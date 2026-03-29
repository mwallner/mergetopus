use crate::git_ops::run_git;
use anyhow::Result;

pub fn commit(message: &str) -> Result<()> {
    run_git(&["commit", "--allow-empty", "-m", message]).map(|_| ())
}

pub fn commit_strict(message: &str) -> Result<()> {
    run_git(&["commit", "-m", message]).map(|_| ())
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

#[cfg(test)]
mod tests {
    use super::*;
    mod test_helpers {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/test_helpers.rs"
        ));
    }

    type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

    #[test]
    fn commit_creates_allow_empty_commit() -> TestResult<()> {
        let repo = test_helpers::init_repo_with_base_file()?;

        test_helpers::with_repo_cwd(&repo, || commit("allow-empty from test"))?;

        let msg = test_helpers::git(&repo, &["log", "-1", "--format=%s"])?;
        assert_eq!(msg, "allow-empty from test");
        Ok(())
    }

    #[test]
    fn commit_strict_commits_staged_changes() -> TestResult<()> {
        let repo = test_helpers::init_repo_with_base_file()?;
        test_helpers::write_file(&repo, "strict.txt", "strict\n")?;
        test_helpers::git(&repo, &["add", "strict.txt"])?;

        test_helpers::with_repo_cwd(&repo, || commit_strict("strict commit"))?;

        let msg = test_helpers::git(&repo, &["log", "-1", "--format=%s"])?;
        assert_eq!(msg, "strict commit");
        Ok(())
    }

    #[test]
    fn branch_tip_commit_message_returns_tip_message_for_branch() -> TestResult<()> {
        let repo = test_helpers::init_repo_with_base_file()?;
        test_helpers::git(&repo, &["checkout", "-b", "feature"])?;
        test_helpers::write_file(&repo, "feature.txt", "feature\n")?;
        test_helpers::commit_all(&repo, "feature tip message")?;

        let message = test_helpers::with_repo_cwd(&repo, || branch_tip_commit_message("feature"))?;
        assert_eq!(message.trim(), "feature tip message");
        Ok(())
    }

    #[test]
    fn commit_message_returns_full_message_for_revision() -> TestResult<()> {
        let repo = test_helpers::init_repo_with_base_file()?;
        test_helpers::write_file(&repo, "full.txt", "full\n")?;
        test_helpers::commit_all(&repo, "subject line\n\nbody line")?;

        let head = test_helpers::git(&repo, &["rev-parse", "HEAD"])?;
        let message = test_helpers::with_repo_cwd(&repo, || commit_message(&head))?;

        assert!(message.contains("subject line"));
        assert!(message.contains("body line"));
        Ok(())
    }

    #[test]
    fn commit_parent_shas_returns_both_parents_for_merge_commit() -> TestResult<()> {
        let repo = test_helpers::init_repo_with_base_file()?;

        test_helpers::git(&repo, &["checkout", "-b", "feature"])?;
        test_helpers::write_file(&repo, "feature.txt", "feature\n")?;
        test_helpers::commit_all(&repo, "feature commit")?;
        let feature_tip = test_helpers::git(&repo, &["rev-parse", "HEAD"])?;

        test_helpers::git(&repo, &["checkout", "main"])?;
        let main_before_merge = test_helpers::git(&repo, &["rev-parse", "HEAD"])?;

        test_helpers::git(
            &repo,
            &["merge", "--no-ff", "-m", "merge feature", "feature"],
        )?;
        let merge_sha = test_helpers::git(&repo, &["rev-parse", "HEAD"])?;

        let parents = test_helpers::with_repo_cwd(&repo, || commit_parent_shas(&merge_sha))?;
        assert_eq!(parents.len(), 2);
        assert_eq!(parents[0], main_before_merge);
        assert_eq!(parents[1], feature_tip);
        Ok(())
    }
}
