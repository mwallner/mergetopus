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
    let inside = run_git(&["rev-parse", "--is-inside-work-tree"])?;
    if inside != "true" {
        bail!("current directory is not inside a Git working tree");
    }

    let status = run_git(&["status", "--porcelain"])?;
    if !status.is_empty() {
        bail!("working tree is not clean; commit or stash changes before running mergetopus");
    }

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

pub fn branch_exists(branch: &str) -> Result<bool> {
    let (ok, _, _) = run_git_allow_failure(&[
        "show-ref",
        "--verify",
        "--quiet",
        &format!("refs/heads/{branch}"),
    ])?;
    Ok(ok)
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

pub fn checkout(branch: &str) -> Result<()> {
    run_git(&["checkout", branch]).map(|_| ())
}

pub fn checkout_new_or_reset(branch: &str, at: &str) -> Result<()> {
    run_git(&["checkout", "-B", branch, at]).map(|_| ())
}

pub fn merge_no_commit(source: &str) -> Result<()> {
    let _ = run_git_allow_failure(&["merge", "--no-ff", "--no-commit", source])?;
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

pub fn merge_in_progress() -> Result<bool> {
    let (ok, _, _) = run_git_allow_failure(&["rev-parse", "-q", "--verify", "MERGE_HEAD"])?;
    Ok(ok)
}

pub fn commit(message: &str) -> Result<()> {
    run_git(&["commit", "--allow-empty", "-m", message]).map(|_| ())
}

pub fn list_slice_branches_for_integration(integration_branch: &str) -> Result<Vec<String>> {
    let out = run_git(&["for-each-ref", "--format=%(refname:short)", "refs/heads"])?;
    let prefix = format!("{integration_branch}_slice");
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
    format!("{integration_branch}_consolidated")
}

pub fn create_consolidated_merge_commit_branch(
    integration_branch: &str,
    source_ref: &str,
    slice_merge_status: &BTreeMap<String, bool>,
) -> Result<String> {
    // Derive remembered head and source commit from the initial first-parent
    // commit on the integration branch (the first partial-merge commit).
    let initial_commit = run_git(&[
        "rev-list",
        "--first-parent",
        "--reverse",
        "--max-count=1",
        integration_branch,
    ])?;
    let initial_commit = initial_commit.trim();
    if initial_commit.is_empty() {
        bail!("integration branch '{}' has no commits", integration_branch);
    }

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

    // Start the merge to establish the merge parents on consolidated branch.
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

pub fn three_way_diff(path: &str, source_ref: &str) -> Result<String> {
    let base = merge_base("HEAD", source_ref)?;
    let ours = show_file_at("HEAD", path)?;
    let base_txt = show_file_at(&base, path)?;
    let theirs = show_file_at(source_ref, path)?;

    Ok(format!(
        "=== OURS (HEAD) ===\n{ours}\n\n=== BASE ({base}) ===\n{base_txt}\n\n=== THEIRS ({source_ref}) ===\n{theirs}"
    ))
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

/// Return the full commit message of the tip commit on `branch`.
pub fn branch_tip_commit_message(branch: &str) -> Result<String> {
    run_git(&["log", "-1", "--format=%B", branch])
}

/// Return the SHA of the first parent of `rev` (i.e. `rev^`).
pub fn parent_sha(rev: &str) -> Result<String> {
    run_git(&["rev-parse", "--verify", &format!("{rev}^")])
        .with_context(|| format!("failed to resolve parent of '{rev}'"))
}

/// List every local branch that looks like a mergetopus slice branch
/// (`*_mw_int_*_slice<N>` where N is one or more digits).
pub fn list_all_slice_branches() -> Result<Vec<String>> {
    let out = run_git(&["for-each-ref", "--format=%(refname:short)", "refs/heads"])?;
    let mut slices = out
        .lines()
        .map(str::trim)
        .filter(|l| {
            l.contains("_mw_int_") && {
                // The suffix after the last "_slice" must be non-empty digits.
                const SLICE_SUFFIX: &str = "_slice";
                if let Some(idx) = l.rfind(SLICE_SUFFIX) {
                    let after = &l[idx + SLICE_SUFFIX.len()..];
                    !after.is_empty() && after.chars().all(|c| c.is_ascii_digit())
                } else {
                    false
                }
            }
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
