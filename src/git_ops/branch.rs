use super::{head_sha, run_git, run_git_allow_failure};
use crate::git_ops::{refs, worktree};
use anyhow::{Result, bail};

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

/// Check if a branch exists either locally or as a remote tracking branch
/// (e.g. `origin/<branch>`).
pub fn branch_exists_anywhere(branch: &str) -> Result<bool> {
    if branch_exists(branch)? {
        return Ok(true);
    }
    let refs = remote_refs_for_local_branch(branch)?;
    Ok(!refs.is_empty())
}

pub fn create_tracking_branch(local_branch: &str, remote_branch: &str) -> Result<()> {
    run_git(&["branch", "--track", local_branch, remote_branch]).map(|_| ())
}

pub fn local_branch_name_from_remote_ref(reference: &str) -> Option<String> {
    let (remote, tail) = reference.split_once('/')?;
    if remote.is_empty() || tail.is_empty() {
        return None;
    }
    Some(tail.to_string())
}

pub fn remote_refs_for_local_branch(local_branch: &str) -> Result<Vec<String>> {
    let out = run_git(&["for-each-ref", "--format=%(refname:short)", "refs/remotes"])?;
    let suffix = format!("/{local_branch}");
    let mut refs = out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && *l != "origin/HEAD")
        .filter(|l| l.ends_with(&suffix))
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    refs.sort();
    Ok(refs)
}

pub fn best_ref_for_local_branch(local_branch: &str) -> Result<Option<String>> {
    if branch_exists(local_branch)? {
        return Ok(Some(local_branch.to_string()));
    }

    let refs = remote_refs_for_local_branch(local_branch)?;
    if refs.is_empty() {
        return Ok(None);
    }

    if let Some(origin) = refs.iter().find(|r| r.starts_with("origin/")) {
        return Ok(Some(origin.clone()));
    }

    Ok(refs.first().cloned())
}

pub fn ensure_local_branch_for_operation(branch_or_remote: &str) -> Result<String> {
    if branch_exists(branch_or_remote)? {
        return Ok(branch_or_remote.to_string());
    }

    if remote_branch_exists(branch_or_remote)? {
        let local = local_branch_name_from_remote_ref(branch_or_remote).ok_or_else(|| {
            anyhow::anyhow!(
                "remote ref '{}' cannot be mapped to a local branch name",
                branch_or_remote
            )
        })?;

        if branch_exists(&local)? {
            let local_sha = refs::resolve_ref(&local)?;
            let remote_sha = refs::resolve_ref(branch_or_remote)?;

            if local_sha == remote_sha {
                return Ok(local);
            }

            bail!(
                "remote branch '{}' maps to local branch '{}' which is not up to date \
                 (local: {}, remote: {}); pull the latest changes first:\n  \
                 git checkout {} && git pull",
                branch_or_remote,
                local,
                &local_sha[..8.min(local_sha.len())],
                &remote_sha[..8.min(remote_sha.len())],
                local,
            );
        }

        create_tracking_branch(&local, branch_or_remote)?;
        return Ok(local);
    }

    let remote_refs = remote_refs_for_local_branch(branch_or_remote)?;
    if remote_refs.is_empty() {
        bail!(
            "branch '{}' does not exist locally and no matching remote tracking branch was found",
            branch_or_remote
        );
    }

    let selected = remote_refs
        .iter()
        .find(|r| r.starts_with("origin/"))
        .cloned()
        .unwrap_or_else(|| remote_refs[0].clone());
    create_tracking_branch(branch_or_remote, &selected)?;
    Ok(branch_or_remote.to_string())
}

pub fn list_branch_refs() -> Result<Vec<String>> {
    let out = run_git(&[
        "for-each-ref",
        "--format=%(refname:short)",
        "refs/heads",
        "refs/remotes",
    ])?;
    let remote_names = list_remote_names()?;
    let branches = out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        // Exclude <remote>/HEAD symbolic refs and bare remote names (not real branches).
        .filter(|l| !l.ends_with("/HEAD") && !remote_names.iter().any(|r| r.as_str() == *l))
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

pub fn list_remote_names() -> Result<Vec<String>> {
    let out = run_git_allow_failure(&["remote"])?;
    if !out.0 {
        return Ok(Vec::new());
    }
    Ok(out
        .1
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(ToOwned::to_owned)
        .collect())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support as test_helpers;

    type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

    #[test]
    fn current_branch_returns_checked_out_branch_name() -> TestResult<()> {
        let repo = test_helpers::init_repo_with_base_file()?;
        let branch = test_helpers::with_repo_cwd(&repo, current_branch)?;
        assert_eq!(branch, "main");
        Ok(())
    }

    #[test]
    fn branch_exists_reports_local_branch_presence() -> TestResult<()> {
        let repo = test_helpers::init_repo_with_base_file()?;
        test_helpers::git(&repo, &["branch", "feature"])?;

        let exists = test_helpers::with_repo_cwd(&repo, || branch_exists("feature"))?;
        let missing = test_helpers::with_repo_cwd(&repo, || branch_exists("missing"))?;

        assert!(exists);
        assert!(!missing);
        Ok(())
    }

    #[test]
    fn remote_branch_exists_reports_remote_tracking_branch_presence() -> TestResult<()> {
        let repo = test_helpers::setup_remote_with_feature()?;

        let exists = test_helpers::with_repo_cwd(&repo, || remote_branch_exists("origin/feature"))?;
        let missing =
            test_helpers::with_repo_cwd(&repo, || remote_branch_exists("origin/missing"))?;

        assert!(exists);
        assert!(!missing);
        Ok(())
    }

    #[test]
    fn create_tracking_branch_creates_local_branch_from_remote() -> TestResult<()> {
        let repo = test_helpers::setup_remote_with_feature()?;
        test_helpers::git(&repo, &["branch", "-D", "feature"])?;

        test_helpers::with_repo_cwd(&repo, || {
            create_tracking_branch("feature", "origin/feature")
        })?;

        let current =
            test_helpers::git(&repo, &["rev-parse", "--abbrev-ref", "feature@{upstream}"])?;
        assert_eq!(current, "origin/feature");
        Ok(())
    }

    #[test]
    fn list_branch_refs_returns_local_and_remote_refs_without_origin_head() -> TestResult<()> {
        let repo = test_helpers::setup_remote_with_feature()?;

        let refs = test_helpers::with_repo_cwd(&repo, list_branch_refs)?;

        assert!(refs.iter().any(|r| r == "main"));
        assert!(refs.iter().any(|r| r == "feature"));
        assert!(refs.iter().any(|r| r == "origin/main"));
        assert!(refs.iter().any(|r| r == "origin/feature"));
        assert!(!refs.iter().any(|r| r == "origin/HEAD"));
        // Bare remote names should be excluded – they are not branches.
        assert!(!refs.iter().any(|r| r == "origin"));
        Ok(())
    }

    #[test]
    fn list_local_branches_returns_sorted_local_branches() -> TestResult<()> {
        let repo = test_helpers::init_repo_with_base_file()?;
        test_helpers::git(&repo, &["branch", "zeta"])?;
        test_helpers::git(&repo, &["branch", "alpha"])?;

        let branches = test_helpers::with_repo_cwd(&repo, list_local_branches)?;

        assert_eq!(branches, vec!["alpha", "main", "zeta"]);
        Ok(())
    }

    #[test]
    fn delete_branch_removes_existing_branch() -> TestResult<()> {
        let repo = test_helpers::init_repo_with_base_file()?;
        test_helpers::git(&repo, &["branch", "trash"])?;

        test_helpers::with_repo_cwd(&repo, || delete_branch("trash"))?;

        let exists = test_helpers::git(&repo, &["show-ref", "--verify", "refs/heads/trash"]);
        assert!(exists.is_err());
        Ok(())
    }

    #[test]
    fn checkout_new_or_reset_creates_or_resets_branch_at_target_commit() -> TestResult<()> {
        let repo = test_helpers::init_repo_with_base_file()?;
        let base_sha = test_helpers::git(&repo, &["rev-parse", "HEAD"])?;

        test_helpers::write_file(&repo, "later.txt", "later\n")?;
        test_helpers::commit_all(&repo, "later")?;

        test_helpers::with_repo_cwd(&repo, || checkout_new_or_reset("feature", &base_sha))?;

        let current = test_helpers::git(&repo, &["branch", "--show-current"])?;
        let head = test_helpers::git(&repo, &["rev-parse", "HEAD"])?;
        assert_eq!(current, "feature");
        assert_eq!(head, base_sha);
        Ok(())
    }

    #[test]
    fn ensure_local_branch_for_operation_creates_tracking_branch_for_remote_only_slice()
    -> TestResult<()> {
        let repo = test_helpers::setup_remote_with_feature()?;
        let slice = "_mmm/main/feature/slice1";
        let remote_slice = format!("origin/{slice}");

        test_helpers::git(&repo, &["checkout", "-b", slice])?;
        test_helpers::write_file(&repo, "slice.txt", "slice\n")?;
        test_helpers::commit_all(&repo, "slice commit")?;
        test_helpers::git(&repo, &["push", "-u", "origin", slice])?;
        test_helpers::git(&repo, &["checkout", "main"])?;
        test_helpers::git(&repo, &["branch", "-D", slice])?;

        let materialized = test_helpers::with_repo_cwd(&repo, || {
            ensure_local_branch_for_operation(&remote_slice)
        })?;
        assert_eq!(materialized, slice);

        let exists = test_helpers::with_repo_cwd(&repo, || branch_exists(slice))?;
        assert!(exists);
        Ok(())
    }

    #[test]
    fn branch_exists_anywhere_finds_remote_only_branch() -> TestResult<()> {
        let repo = test_helpers::setup_remote_with_feature()?;
        let branch = "_mmm/main/feature/integration";

        test_helpers::git(&repo, &["checkout", "-b", branch])?;
        test_helpers::write_file(&repo, "int.txt", "int\n")?;
        test_helpers::commit_all(&repo, "integration commit")?;
        test_helpers::git(&repo, &["push", "-u", "origin", branch])?;
        test_helpers::git(&repo, &["checkout", "main"])?;
        test_helpers::git(&repo, &["branch", "-D", branch])?;

        // Not locally present.
        let local = test_helpers::with_repo_cwd(&repo, || branch_exists(branch))?;
        assert!(!local);

        // But branch_exists_anywhere should find it via remote.
        let anywhere = test_helpers::with_repo_cwd(&repo, || branch_exists_anywhere(branch))?;
        assert!(anywhere);

        Ok(())
    }

    #[test]
    fn branch_exists_anywhere_returns_false_for_truly_missing_branch() -> TestResult<()> {
        let repo = test_helpers::setup_remote_with_feature()?;

        let result =
            test_helpers::with_repo_cwd(&repo, || branch_exists_anywhere("no_such_branch"))?;
        assert!(!result);
        Ok(())
    }
}
