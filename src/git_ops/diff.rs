use crate::git_ops::{run_git, run_git_allow_failure};
use anyhow::Result;

pub fn conflicted_files() -> Result<Vec<String>> {
    let out = run_git(&["diff", "--name-only", "--diff-filter=U"])?;
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(ToOwned::to_owned)
        .collect())
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

pub fn staged_has_changes() -> Result<bool> {
    let (ok, _, _) = run_git_allow_failure(&["diff", "--cached", "--quiet"])?;
    Ok(!ok)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support as test_helpers;

    type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

    #[test]
    fn conflicted_files_lists_unmerged_paths() -> TestResult<()> {
        let repo = test_helpers::setup_single_conflict_repo()?;

        let merge = test_helpers::run(
            std::process::Command::new("git")
                .args(["merge", "feature"])
                .current_dir(&repo),
        )?;
        assert!(!merge.status.success(), "expected merge conflict setup");

        let files = test_helpers::with_repo_cwd(&repo, conflicted_files)?;
        assert_eq!(files, vec!["conflict.txt"]);
        Ok(())
    }

    #[test]
    fn staged_files_lists_cached_paths() -> TestResult<()> {
        let repo = test_helpers::init_repo_with_base_file()?;
        test_helpers::write_file(&repo, "staged.txt", "staged\n")?;
        test_helpers::git(&repo, &["add", "staged.txt"])?;

        let files = test_helpers::with_repo_cwd(&repo, staged_files)?;
        assert_eq!(files, vec!["staged.txt"]);
        Ok(())
    }

    #[test]
    fn unstaged_files_lists_worktree_only_paths() -> TestResult<()> {
        let repo = test_helpers::init_repo_with_base_file()?;
        test_helpers::write_file(&repo, "base.txt", "base\nmodified\n")?;

        let files = test_helpers::with_repo_cwd(&repo, unstaged_files)?;
        assert_eq!(files, vec!["base.txt"]);
        Ok(())
    }

    #[test]
    fn staged_has_changes_reports_index_state() -> TestResult<()> {
        let repo = test_helpers::init_repo_with_base_file()?;

        let initially = test_helpers::with_repo_cwd(&repo, staged_has_changes)?;
        assert!(!initially, "fresh repo should have no staged changes");

        test_helpers::write_file(&repo, "index.txt", "index\n")?;
        test_helpers::git(&repo, &["add", "index.txt"])?;

        let after_add = test_helpers::with_repo_cwd(&repo, staged_has_changes)?;
        assert!(after_add, "staged file should be detected");

        Ok(())
    }
}
