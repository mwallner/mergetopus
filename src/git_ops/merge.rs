use crate::git_ops::{run_git, run_git_allow_failure};
use anyhow::{Context, Result, bail};

pub fn merge_in_progress() -> Result<bool> {
    let (ok, _, _) = run_git_allow_failure(&["rev-parse", "-q", "--verify", "MERGE_HEAD"])?;
    Ok(ok)
}

pub fn merge_head_sha() -> Result<String> {
    run_git(&["rev-parse", "--verify", "MERGE_HEAD"])
        .context("failed to resolve MERGE_HEAD for in-progress merge")
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

pub fn merge_base(a: &str, b: &str) -> Result<String> {
    run_git(&["merge-base", a, b])
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

    fn setup_clean_non_conflicting_repo() -> TestResult<std::path::PathBuf> {
        let repo = test_helpers::init_repo_with_base_file()?;

        test_helpers::git(&repo, &["checkout", "-b", "feature"])?;
        test_helpers::write_file(&repo, "feature.txt", "feature\n")?;
        test_helpers::commit_all(&repo, "feature change")?;

        test_helpers::git(&repo, &["checkout", "main"])?;
        test_helpers::write_file(&repo, "main.txt", "main\n")?;
        test_helpers::commit_all(&repo, "main change")?;

        Ok(repo)
    }

    #[test]
    fn merge_in_progress_reports_false_then_true_for_conflicted_merge() -> TestResult<()> {
        let repo = test_helpers::setup_single_conflict_repo()?;

        let before = test_helpers::with_repo_cwd(&repo, merge_in_progress)?;
        assert!(!before);

        test_helpers::with_repo_cwd(&repo, || merge_no_commit("feature"))?;
        let after = test_helpers::with_repo_cwd(&repo, merge_in_progress)?;
        assert!(after);

        test_helpers::with_repo_cwd(&repo, merge_abort)?;
        Ok(())
    }

    #[test]
    fn merge_head_sha_matches_feature_tip_during_conflicted_merge() -> TestResult<()> {
        let repo = test_helpers::setup_single_conflict_repo()?;
        let feature_tip = test_helpers::git(&repo, &["rev-parse", "feature"])?;

        test_helpers::with_repo_cwd(&repo, || merge_no_commit("feature"))?;
        let merge_head = test_helpers::with_repo_cwd(&repo, merge_head_sha)?;

        assert_eq!(merge_head, feature_tip);
        test_helpers::with_repo_cwd(&repo, merge_abort)?;
        Ok(())
    }

    #[test]
    fn merge_no_commit_succeeds_for_clean_non_conflicting_merge() -> TestResult<()> {
        let repo = setup_clean_non_conflicting_repo()?;

        test_helpers::with_repo_cwd(&repo, || merge_no_commit("feature"))?;

        let in_progress = test_helpers::with_repo_cwd(&repo, merge_in_progress)?;
        assert!(
            in_progress,
            "--no-commit merge should leave MERGE_HEAD present"
        );

        test_helpers::with_repo_cwd(&repo, merge_abort)?;
        Ok(())
    }

    #[test]
    fn merge_no_commit_returns_error_for_unrelated_histories() -> TestResult<()> {
        let repo = test_helpers::init_repo_with_base_file()?;

        test_helpers::git(&repo, &["checkout", "--orphan", "other"])?;
        let _ = test_helpers::git(&repo, &["rm", "-rf", "."]);
        test_helpers::write_file(&repo, "other.txt", "other\n")?;
        test_helpers::commit_all(&repo, "other root")?;
        test_helpers::git(&repo, &["checkout", "main"])?;

        let err = test_helpers::with_repo_cwd(&repo, || merge_no_commit("other"))
            .expect_err("expected unrelated-histories merge to fail");
        let msg = err.to_string();
        assert!(
            msg.contains("failed before entering conflict resolution"),
            "unexpected error: {msg}"
        );
        Ok(())
    }

    #[test]
    fn merge_abort_clears_in_progress_merge_state() -> TestResult<()> {
        let repo = test_helpers::setup_single_conflict_repo()?;
        test_helpers::with_repo_cwd(&repo, || merge_no_commit("feature"))?;

        test_helpers::with_repo_cwd(&repo, merge_abort)?;

        let in_progress = test_helpers::with_repo_cwd(&repo, merge_in_progress)?;
        assert!(!in_progress);
        Ok(())
    }

    #[test]
    fn merge_base_matches_git_merge_base_result() -> TestResult<()> {
        let repo = test_helpers::setup_single_conflict_repo()?;
        let expected = test_helpers::git(&repo, &["merge-base", "main", "feature"])?;

        let actual = test_helpers::with_repo_cwd(&repo, || merge_base("main", "feature"))?;
        assert_eq!(actual, expected);
        Ok(())
    }
}
