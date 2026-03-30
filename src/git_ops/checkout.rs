use crate::git_ops::{run_git, worktree};
use anyhow::Result;

pub fn checkout(branch: &str) -> Result<()> {
    let entries = worktree::list_worktree_entries()?;
    if worktree::has_existing_linked_worktrees(&entries) {
        let path = worktree::ensure_worktree_for_existing_branch(branch, &entries)?;
        worktree::switch_to_dir(&path)?;
    }

    run_git(&["checkout", branch]).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support as test_helpers;

    type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

    #[test]
    fn checkout_switches_branch_in_standard_single_worktree_repo() -> TestResult<()> {
        let repo = test_helpers::init_repo_with_base_file()?;
        test_helpers::git(&repo, &["checkout", "-b", "feature"])?;
        test_helpers::git(&repo, &["checkout", "main"])?;

        test_helpers::with_repo_cwd(&repo, || checkout("feature"))?;

        let current = test_helpers::git(&repo, &["branch", "--show-current"])?;
        assert_eq!(current, "feature");
        Ok(())
    }

    #[test]
    fn checkout_uses_branch_worktree_when_linked_worktrees_exist() -> TestResult<()> {
        let repo = test_helpers::init_repo_with_base_file()?;
        test_helpers::git(&repo, &["checkout", "-b", "feature"])?;
        test_helpers::write_file(&repo, "feature.txt", "feature\n")?;
        test_helpers::commit_all(&repo, "feature commit")?;
        test_helpers::git(&repo, &["checkout", "main"])?;

        let helper_path = test_helpers::unique_temp_repo_dir();
        std::fs::create_dir_all(&helper_path)?;
        test_helpers::git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "wt_helper",
                helper_path.to_str().ok_or("invalid helper path")?,
                "main",
            ],
        )?;

        let (cwd_after_checkout, branch_after_checkout) =
            test_helpers::with_repo_cwd(&repo, || {
                checkout("feature")?;
                let cwd = std::env::current_dir()?.display().to_string();
                let branch = run_git(&["branch", "--show-current"])?;
                Ok((cwd, branch))
            })?;

        let wt_out = test_helpers::git(&repo, &["worktree", "list", "--porcelain"])?;
        let mut feature_worktree_path: Option<String> = None;
        let mut current_path: Option<String> = None;

        for line in wt_out.lines() {
            if let Some(rest) = line.strip_prefix("worktree ") {
                current_path = Some(rest.trim().to_string());
                continue;
            }
            if let Some(rest) = line.strip_prefix("branch refs/heads/")
                && rest.trim() == "feature"
            {
                feature_worktree_path = current_path.clone();
                break;
            }
        }

        let expected_path = feature_worktree_path.ok_or("feature worktree path not found")?;
        assert_eq!(branch_after_checkout, "feature");
        // Normalize path separators: git porcelain uses '/' on Windows while
        // std::env::current_dir() returns the native '\' separator.
        let norm = |s: &str| s.replace('\\', "/");
        assert_eq!(norm(&cwd_after_checkout), norm(&expected_path));

        Ok(())
    }
}
