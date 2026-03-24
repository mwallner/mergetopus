use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::models::PathProvenance;

fn run_git(args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .with_context(|| format!("failed to execute git {}", args.join(" ")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git {} failed: {}", args.join(" "), stderr.trim());
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn run_git_allow_failure(args: &[&str]) -> Result<(bool, String, String)> {
    let output = Command::new("git")
        .args(args)
        .output()
        .with_context(|| format!("failed to execute git {}", args.join(" ")))?;

    let ok = output.status.success();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Ok((ok, stdout, stderr))
}

pub fn ensure_git_context() -> Result<()> {
    ensure_git_worktree()?;

    let status = run_git(&["status", "--porcelain"])?;
    if !status.is_empty() {
        bail!("working tree is not clean; commit or stash changes before running mergetopus");
    }

    Ok(())
}

pub fn ensure_git_worktree() -> Result<()> {
    let inside = run_git(&["rev-parse", "--is-inside-work-tree"])?;
    if inside != "true" {
        bail!("current directory is not inside a Git working tree");
    }

    ensure_longpaths_support()?;

    Ok(())
}

pub fn current_branch() -> Result<String> {
    let (ok, out, _) = run_git_allow_failure(&["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    if ok && !out.is_empty() {
        return Ok(out);
    }

    let head = head_sha()?;
    Ok(format!("detached_{}", &head[..8.min(head.len())]))
}

pub fn head_sha() -> Result<String> {
    run_git(&["rev-parse", "--verify", "HEAD"])
}

pub fn resolve_commit(rev: &str) -> Result<String> {
    run_git(&["rev-parse", "--verify", &format!("{rev}^{{commit}}")])
        .with_context(|| format!("merge source '{rev}' is not a valid commit-ish ref"))
}

pub fn resolve_ref(reference: &str) -> Result<String> {
    run_git(&["rev-parse", "--verify", &format!("{reference}^{{commit}}")])
        .with_context(|| format!("failed to resolve reference '{reference}' to a commit"))
}

pub fn branch_exists(branch: &str) -> Result<bool> {
    let (ok, _, _) = run_git_allow_failure(&[
        "show-ref",
        "--verify",
        "--quiet",
        &format!("refs/heads/{branch}"),
    ])?;
    Ok(ok)
}

pub fn remote_branch_exists(branch: &str) -> Result<bool> {
    let (ok, _, _) = run_git_allow_failure(&[
        "show-ref",
        "--verify",
        "--quiet",
        &format!("refs/remotes/{branch}"),
    ])?;
    Ok(ok)
}

pub fn create_tracking_branch(local_branch: &str, remote_branch: &str) -> Result<()> {
    run_git(&["branch", "--track", local_branch, remote_branch]).map(|_| ())
}

pub fn list_branch_refs() -> Result<Vec<String>> {
    let out = run_git(&[
        "for-each-ref",
        "--format=%(refname:short)",
        "refs/heads",
        "refs/remotes",
    ])?;
    let branches = out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && *l != "origin/HEAD")
        .map(ToOwned::to_owned)
        .collect();
    Ok(branches)
}

pub fn list_local_branches() -> Result<Vec<String>> {
    let out = run_git(&["for-each-ref", "--format=%(refname:short)", "refs/heads"])?;
    let mut branches = out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    branches.sort();
    Ok(branches)
}

pub fn checkout(branch: &str) -> Result<()> {
    run_git(&["checkout", branch]).map(|_| ())
}

pub fn delete_branch(branch: &str) -> Result<()> {
    run_git(&["branch", "-D", branch]).map(|_| ())
}

pub fn checkout_new_or_reset(branch: &str, at: &str) -> Result<()> {
    run_git(&["checkout", "-B", branch, at]).map(|_| ())
}

pub fn merge_no_commit(source: &str) -> Result<()> {
    let (ok, _, stderr) = run_git_allow_failure(&["merge", "--no-ff", "--no-commit", source])?;
    if ok {
        return Ok(());
    }

    // Expected conflict path: merge exits non-zero but leaves MERGE_HEAD.
    if merge_in_progress()? {
        return Ok(());
    }

    bail!(
        "git merge failed before entering conflict resolution: {}\n\
         verify source/history compatibility, then retry (for unrelated histories, merge manually with --allow-unrelated-histories first)",
        stderr
    );
}

pub fn merge_abort() -> Result<()> {
    run_git(&["merge", "--abort"]).map(|_| ())
}

#[cfg(target_os = "windows")]
fn ensure_longpaths_support() -> Result<()> {
    let current = get_git_config("core.longpaths")?.unwrap_or_default();
    if current.eq_ignore_ascii_case("true") {
        return Ok(());
    }

    run_git(&["config", "core.longpaths", "true"]).map(|_| ())
}

#[cfg(not(target_os = "windows"))]
fn ensure_longpaths_support() -> Result<()> {
    Ok(())
}

pub fn conflicted_files() -> Result<Vec<String>> {
    let out = run_git(&["diff", "--name-only", "--diff-filter=U"])?;
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

pub fn restore_ours(path: &str) -> Result<()> {
    run_git(&[
        "restore",
        "--source=HEAD",
        "--staged",
        "--worktree",
        "--",
        path,
    ])
    .map(|_| ())
}

pub fn staged_files() -> Result<Vec<String>> {
    let out = run_git(&["diff", "--cached", "--name-only"])?;
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

pub fn unstaged_files() -> Result<Vec<String>> {
    let out = run_git(&["diff", "--name-only"])?;
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

pub fn merge_in_progress() -> Result<bool> {
    let (ok, _, _) = run_git_allow_failure(&["rev-parse", "-q", "--verify", "MERGE_HEAD"])?;
    Ok(ok)
}

pub fn merge_head_sha() -> Result<String> {
    run_git(&["rev-parse", "--verify", "MERGE_HEAD"])
        .context("failed to resolve MERGE_HEAD for in-progress merge")
}

pub fn commit(message: &str) -> Result<()> {
    run_git(&["commit", "--allow-empty", "-m", message]).map(|_| ())
}

pub fn commit_strict(message: &str) -> Result<()> {
    run_git(&["commit", "-m", message]).map(|_| ())
}

pub fn list_slice_branches_for_integration(integration_branch: &str) -> Result<Vec<String>> {
    let out = run_git(&["for-each-ref", "--format=%(refname:short)", "refs/heads"])?;
    let Some(base) = integration_branch.strip_suffix("/integration") else {
        return Ok(Vec::new());
    };
    let prefix = format!("{base}/slice");
    let mut slices = out
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with(&prefix))
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    slices.sort();
    Ok(slices)
}

pub fn is_ancestor(older: &str, newer: &str) -> Result<bool> {
    let (ok, _, _) = run_git_allow_failure(&["merge-base", "--is-ancestor", older, newer])?;
    Ok(ok)
}

pub fn slice_merge_status(
    integration_branch: &str,
    slice_branches: &[String],
) -> Result<BTreeMap<String, bool>> {
    let mut result = BTreeMap::new();
    for slice in slice_branches {
        result.insert(slice.clone(), is_ancestor(slice, integration_branch)?);
    }
    Ok(result)
}

pub fn path_exists_in_ref(reference: &str, path: &str) -> Result<bool> {
    let (ok, _, _) = run_git_allow_failure(&["cat-file", "-e", &format!("{reference}:{path}")])?;
    Ok(ok)
}

pub fn restore_from_ref(reference: &str, path: &str) -> Result<()> {
    run_git(&[
        "restore",
        &format!("--source={reference}"),
        "--staged",
        "--worktree",
        "--",
        path,
    ])
    .map(|_| ())
}

pub fn rm_path(path: &str) -> Result<()> {
    run_git(&["rm", "--ignore-unmatch", "--", path]).map(|_| ())
}

pub fn staged_has_changes() -> Result<bool> {
    let (ok, _, _) = run_git_allow_failure(&["diff", "--cached", "--quiet"])?;
    Ok(!ok)
}

pub fn path_provenance(source_ref: &str, source_sha: &str, path: &str) -> Result<PathProvenance> {
    let format = "%H%x1f%an%x1f%ae%x1f%aI";
    let (ok, out, _) = run_git_allow_failure(&[
        "log",
        "-n",
        "1",
        &format!("--format={format}"),
        source_sha,
        "--",
        path,
    ])?;

    let mut path_commit = None;
    let mut author_name = None;
    let mut author_email = None;
    let mut author_date = None;

    if ok && !out.trim().is_empty() {
        let parts = out.split('\u{1f}').collect::<Vec<_>>();
        if parts.len() >= 4 {
            path_commit = Some(parts[0].to_string());
            author_name = Some(parts[1].to_string());
            author_email = Some(parts[2].to_string());
            author_date = Some(parts[3].to_string());
        }
    }

    Ok(PathProvenance {
        source_ref: source_ref.to_string(),
        source_commit: source_sha.to_string(),
        path: path.to_string(),
        path_commit,
        author_name,
        author_email,
        author_date,
    })
}

pub fn commit_slice(message: &str, provenance: &PathProvenance) -> Result<()> {
    let mut command = Command::new("git");
    command.args(["commit", "-m", message]);

    if let Some(name) = &provenance.author_name {
        command.env("GIT_AUTHOR_NAME", name);
    }
    if let Some(email) = &provenance.author_email {
        command.env("GIT_AUTHOR_EMAIL", email);
    }
    if let Some(date) = &provenance.author_date {
        command.env("GIT_AUTHOR_DATE", date);
    }

    let output = command
        .output()
        .context("failed to execute git commit for slice")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("slice commit failed: {}", stderr.trim());
    }

    Ok(())
}

pub fn merge_base(a: &str, b: &str) -> Result<String> {
    run_git(&["merge-base", a, b])
}

pub fn show_file_at(reference: &str, path: &str) -> Result<String> {
    let (ok, out, err) = run_git_allow_failure(&["show", &format!("{reference}:{path}")])?;
    if ok {
        Ok(out)
    } else {
        Ok(format!("<unavailable: {err}>"))
    }
}

pub fn consolidated_branch_name(integration_branch: &str) -> String {
    if let Some(base) = integration_branch.strip_suffix("/integration") {
        format!("{base}/kokomeco")
    } else {
        format!("{integration_branch}/kokomeco")
    }
}

pub fn create_consolidated_merge_commit_branch(
    integration_branch: &str,
    source_ref: &str,
    slice_merge_status: &BTreeMap<String, bool>,
) -> Result<String> {
    // Derive remembered head and source commit from the first mergetopus
    // partial-merge commit on the integration branch, not from the oldest
    // reachable ancestor in the repository history.
    let initial_commit = initial_integration_merge_commit(integration_branch)?;

    let remembered_head = run_git(&["rev-parse", "--verify", &format!("{initial_commit}^1")])
        .context("failed to resolve remembered head from initial integration commit")?;
    let source_sha = run_git(&["rev-parse", "--verify", &format!("{initial_commit}^2")]).context(
        "failed to resolve source SHA from initial integration commit (expected a merge commit)",
    )?;

    let merged_slices = slice_merge_status
        .iter()
        .filter(|(_, merged)| **merged)
        .map(|(name, _)| format!("* {name}"))
        .collect::<Vec<_>>()
        .join("\n");

    let message = format!(
        "Mergetopus consolidated merge: '{source_ref}' into '{integration_branch}'\n\nThis commit snapshots the resolved integration tree into one merge commit.\n\nSource-Ref: {source_ref}\nSource-Commit: {source_sha}\nRemembered-Head: {remembered_head}\nMerged-Slices:\n{}",
        if merged_slices.is_empty() {
            "* (none)"
        } else {
            &merged_slices
        }
    );

    let branch = consolidated_branch_name(integration_branch);
    checkout_new_or_reset(&branch, &remembered_head)?;

    // Start the merge to establish the correct merge parents, then replace the
    // index/worktree content with the final integration tree before committing.
    merge_no_commit(&source_sha)?;

    // Replace staged/worktree content with the resolved integration branch content.
    run_git(&[
        "restore",
        &format!("--source={integration_branch}"),
        "--staged",
        "--worktree",
        "--",
        ".",
    ])
    .context("failed to overlay integration branch content onto consolidated branch")?;

    commit(&message)?;
    Ok(branch)
}

fn initial_integration_merge_commit(integration_branch: &str) -> Result<String> {
    let out = run_git(&[
        "log",
        integration_branch,
        "--first-parent",
        "--reverse",
        "--format=%H%x1f%s",
    ])?;

    for line in out.lines() {
        let mut parts = line.splitn(2, '\u{1f}');
        let sha = parts.next().unwrap_or("").trim();
        let subject = parts.next().unwrap_or("").trim();
        if subject.starts_with("Mergetopus: partial merge '") {
            return Ok(sha.to_string());
        }
    }

    bail!(
        "failed to locate initial mergetopus partial-merge commit on integration branch '{}'",
        integration_branch
    )
}

pub fn three_way_diff(path: &str, source_ref: &str) -> Result<String> {
    let base = merge_base("HEAD", source_ref)?;
    let ours = show_file_at("HEAD", path)?;
    let base_txt = show_file_at(&base, path)?;
    let theirs = show_file_at(source_ref, path)?;

    Ok(format!(
        "=== OURS (HEAD) ===\n{ours}\n\n=== BASE ({base}) ===\n{base_txt}\n\n=== THEIRS ({source_ref}) ===\n{theirs}"
    ))
}

pub fn launch_difftool(path: &str, source_ref: &str) -> Result<()> {
    run_git(&["difftool", "--no-prompt", "HEAD", source_ref, "--", path]).with_context(|| {
        format!("failed to launch git difftool for '{path}' against '{source_ref}'")
    })?;
    Ok(())
}

/// Read a single git config value; returns `None` when the key is unset or
/// the value is empty (an empty tool name or command is unusable).
pub fn get_git_config(key: &str) -> Result<Option<String>> {
    let (ok, out, _) = run_git_allow_failure(&["config", "--get", key])?;
    if ok && !out.is_empty() {
        Ok(Some(out))
    } else {
        Ok(None)
    }
}

pub fn refs_pointing_to(commit: &str) -> Result<Vec<String>> {
    let out = run_git(&[
        "for-each-ref",
        "--format=%(refname:short)",
        "--points-at",
        commit,
        "refs/heads",
        "refs/remotes",
    ])?;

    let mut refs = out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && *l != "origin/HEAD")
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    refs.sort();
    Ok(refs)
}

/// Return the full commit message of the tip commit on `branch`.
pub fn branch_tip_commit_message(branch: &str) -> Result<String> {
    run_git(&["log", "-1", "--format=%B", branch])
}

pub fn commit_message(rev: &str) -> Result<String> {
    run_git(&["show", "-s", "--format=%B", rev])
}

pub fn commit_parent_shas(rev: &str) -> Result<Vec<String>> {
    let out = run_git(&["show", "-s", "--format=%P", rev])?;
    Ok(out
        .split_whitespace()
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

pub fn first_parent_oldest_commit(branch: &str) -> Result<String> {
    let out = run_git(&[
        "rev-list",
        "--first-parent",
        "--reverse",
        "--max-count=1",
        branch,
    ])?;
    let commit = out.trim();
    if commit.is_empty() {
        bail!("branch '{}' has no commits", branch);
    }
    Ok(commit.to_string())
}

/// Return the SHA of the first parent of `rev` (i.e. `rev^`).
pub fn parent_sha(rev: &str) -> Result<String> {
    run_git(&["rev-parse", "--verify", &format!("{rev}^")])
        .with_context(|| format!("failed to resolve parent of '{rev}'"))
}

/// List every local branch that looks like a mergetopus slice branch
/// (`_mmm/<original>/<source>/slice<N>` where N is one or more digits).
pub fn list_all_slice_branches() -> Result<Vec<String>> {
    let out = run_git(&["for-each-ref", "--format=%(refname:short)", "refs/heads"])?;
    let mut slices = out
        .lines()
        .map(str::trim)
        .filter(|l| {
            let parts = l.split('/').collect::<Vec<_>>();
            parts.len() == 4
                && parts[0] == "_mmm"
                && !parts[1].is_empty()
                && !parts[2].is_empty()
                && parts[3].starts_with("slice")
                && parts[3]["slice".len()..]
                    .chars()
                    .all(|c| c.is_ascii_digit())
                && parts[3].len() > "slice".len()
        })
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    slices.sort();
    Ok(slices)
}

/// Write the content of `reference:path` from the object store to the file at
/// `dest`.  If the path does not exist at that ref (e.g. new file / deleted
/// file), an empty file is written instead.
pub fn write_blob_to_path(reference: &str, path: &str, dest: &str) -> Result<()> {
    let output = Command::new("git")
        .args(["show", &format!("{reference}:{path}")])
        .output()
        .with_context(|| format!("failed to execute git show {reference}:{path}"))?;

    let content: &[u8] = if output.status.success() {
        &output.stdout
    } else {
        b""
    };

    fs::write(dest, content).with_context(|| format!("failed to write '{dest}'"))
}

/// Stage a single path in the index (`git add -- <path>`).
pub fn stage_path(path: &str) -> Result<()> {
    run_git(&["add", "--", path]).map(|_| ())
}

pub fn select_conflicts_by_list(all_conflicts: &[String], csv: &str) -> Result<Vec<String>> {
    let set = all_conflicts.iter().cloned().collect::<BTreeSet<_>>();
    let requested = csv
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();

    let mut selected = Vec::new();
    for item in requested {
        if !set.contains(&item) {
            bail!("path '{item}' is not in conflicted file list");
        }
        selected.push(item);
    }

    selected.sort();
    selected.dedup();
    Ok(selected)
}
