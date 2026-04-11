use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::process::Command;

use crate::models::PathProvenance;

mod branch;
mod checkout;
mod commit;
mod diff;
mod merge;
mod refs;
mod worktree;

pub use branch::*;
pub use checkout::*;
pub use commit::*;
pub use diff::*;
pub use merge::*;
pub use refs::*;

pub fn run_git(args: &[&str]) -> Result<String> {
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

pub fn run_git_allow_failure(args: &[&str]) -> Result<(bool, String, String)> {
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

pub fn list_slice_branches_for_integration(integration_branch: &str) -> Result<Vec<String>> {
    let out = run_git(&[
        "for-each-ref",
        "--format=%(refname:short)",
        "refs/heads",
        "refs/remotes",
    ])?;
    let Some(base) = integration_branch.strip_suffix("/integration") else {
        return Ok(Vec::new());
    };
    let prefix = format!("{base}/slice");
    let mut slices = out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && *l != "origin/HEAD")
        .filter_map(|l| {
            if l.starts_with(&prefix) {
                Some(l.to_string())
            } else if let Some(local) = local_branch_name_from_remote_ref(l) {
                if local.starts_with(&prefix) {
                    Some(local)
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    slices.sort();
    slices.dedup();
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
        let probe_ref = best_ref_for_local_branch(slice)?.unwrap_or_else(|| slice.clone());
        result.insert(slice.clone(), is_ancestor(&probe_ref, integration_branch)?);
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

fn is_slice_branch_ref(reference: &str) -> bool {
    let parts = reference.split('/').collect::<Vec<_>>();
    let Some(idx) = parts.iter().position(|p| *p == "_mmm") else {
        return false;
    };

    parts.len().saturating_sub(idx) == 4
        && !parts[idx + 1].is_empty()
        && !parts[idx + 2].is_empty()
        && parts[idx + 3].starts_with("slice")
        && parts[idx + 3].len() > "slice".len()
        && parts[idx + 3]["slice".len()..]
            .chars()
            .all(|c| c.is_ascii_digit())
}

/// Return the first Mergetopus partial-merge commit on the integration branch
/// first-parent history.
pub fn first_mergetopus_partial_merge_commit(integration_branch: &str) -> Result<String> {
    let out = run_git(&[
        "log",
        integration_branch,
        "--first-parent",
        "--reverse",
        "--format=%H%x1f%P%x1f%s",
    ])?;

    for line in out.lines() {
        let mut parts = line.splitn(3, '\u{1f}');
        let sha = parts.next().unwrap_or("").trim();
        let parent_list = parts.next().unwrap_or("").trim();
        let subject = parts.next().unwrap_or("").trim();

        // First, find the first merge commit on the integration branch that
        // does not come from a slice branch.
        let parents = parent_list
            .split_whitespace()
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>();
        if parents.len() < 2 {
            continue;
        }

        let merged_parent = parents[1];
        let merged_parent_refs = refs_pointing_to(merged_parent)?;
        let comes_from_slice = merged_parent_refs.iter().any(|r| is_slice_branch_ref(r));
        if comes_from_slice {
            continue;
        }

        if !subject.starts_with("Mergetopus: partial merge '") {
            eprintln!(
                "warning: skipping merge commit '{}' on '{}' because subject does not match expected Mergetopus prefix",
                sha, integration_branch
            );
            continue;
        }

        return Ok(sha.to_string());
    }

    bail!(
        "failed to locate initial mergetopus partial-merge commit on integration branch '{}'",
        integration_branch
    )
}

/// Return the SHA of the first parent of `rev` (i.e. `rev^`).
pub fn parent_sha(rev: &str) -> Result<String> {
    run_git(&["rev-parse", "--verify", &format!("{rev}^")])
        .with_context(|| format!("failed to resolve parent of '{rev}'"))
}

/// List every local branch that looks like a mergetopus slice branch
/// (`_mmm/<original>/<source>/slice<N>` where N is one or more digits).
pub fn list_all_slice_branches() -> Result<Vec<String>> {
    let out = run_git(&[
        "for-each-ref",
        "--format=%(refname:short)",
        "refs/heads",
        "refs/remotes",
    ])?;
    let mut slices = out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && *l != "origin/HEAD")
        .filter_map(|l| {
            if is_local_slice_branch_name(l) {
                return Some(l.to_string());
            }

            local_branch_name_from_remote_ref(l)
                .filter(|candidate| is_local_slice_branch_name(candidate))
        })
        .collect::<Vec<_>>();
    slices.sort();
    slices.dedup();
    Ok(slices)
}

fn is_local_slice_branch_name(branch: &str) -> bool {
    let parts = branch.split('/').collect::<Vec<_>>();
    parts.len() == 4
        && parts[0] == "_mmm"
        && !parts[1].is_empty()
        && !parts[2].is_empty()
        && parts[3].starts_with("slice")
        && parts[3]["slice".len()..]
            .chars()
            .all(|c| c.is_ascii_digit())
        && parts[3].len() > "slice".len()
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

#[cfg(test)]
mod tests {
    use super::{is_slice_branch_ref, list_all_slice_branches, list_slice_branches_for_integration};
    use crate::test_support as test_helpers;

    type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

    #[test]
    fn slice_ref_detection_accepts_local_and_remote() {
        assert!(is_slice_branch_ref("_mmm/main/feature/slice1"));
        assert!(is_slice_branch_ref("origin/_mmm/main/feature/slice23"));
    }

    #[test]
    fn slice_ref_detection_rejects_non_slice_refs() {
        assert!(!is_slice_branch_ref("_mmm/main/feature/integration"));
        assert!(!is_slice_branch_ref("feature"));
    }

    #[test]
    fn remote_only_slices_are_listed_by_local_name() -> TestResult<()> {
        let repo = test_helpers::setup_remote_with_feature()?;
        let integration = "_mmm/main/feature/integration";
        let slice = "_mmm/main/feature/slice1";

        test_helpers::git(&repo, &["checkout", "-b", integration])?;
        test_helpers::git(&repo, &["checkout", "-b", slice])?;
        test_helpers::write_file(&repo, "slice.txt", "slice\n")?;
        test_helpers::commit_all(&repo, "slice commit")?;
        test_helpers::git(&repo, &["push", "-u", "origin", integration])?;
        test_helpers::git(&repo, &["push", "-u", "origin", slice])?;
        test_helpers::git(&repo, &["checkout", "main"])?;
        test_helpers::git(&repo, &["branch", "-D", integration])?;
        test_helpers::git(&repo, &["branch", "-D", slice])?;

        let all = test_helpers::with_repo_cwd(&repo, list_all_slice_branches)?;
        assert!(all.iter().any(|b| b == slice));

        let for_integration =
            test_helpers::with_repo_cwd(&repo, || list_slice_branches_for_integration(integration))?;
        assert!(for_integration.iter().any(|b| b == slice));
        Ok(())
    }
}
