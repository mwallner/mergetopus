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
}
