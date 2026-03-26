use super::{head_sha, run_git, run_git_allow_failure};
use crate::git_ops::worktree;
use anyhow::Result;

pub fn current_branch() -> Result<String> {
    let (ok, out, _) = run_git_allow_failure(&["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    if ok && !out.is_empty() {
        return Ok(out);
    }

    let head = head_sha()?;
    Ok(format!("detached_{}", &head[..8.min(head.len())]))
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

pub fn delete_branch(branch: &str) -> Result<()> {
    run_git(&["branch", "-D", branch]).map(|_| ())
}

pub fn checkout_new_or_reset(branch: &str, at: &str) -> Result<()> {
    let entries = worktree::list_worktree_entries()?;
    if worktree::has_existing_linked_worktrees(&entries) {
        let path = worktree::ensure_worktree_for_branch_reset(branch, at, &entries)?;
        worktree::switch_to_dir(&path)?;
    }

    run_git(&["checkout", "-B", branch, at]).map(|_| ())
}
