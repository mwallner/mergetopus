use crate::git_ops::run_git;
use anyhow::{Context, Result};

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

/// Return the SHA of the first parent of `rev` (i.e. `rev^`).
pub fn parent_sha(rev: &str) -> Result<String> {
    run_git(&["rev-parse", "--verify", &format!("{rev}^")])
        .with_context(|| format!("failed to resolve parent of '{rev}'"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support as test_helpers;

    type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

    #[test]
    fn head_sha_matches_git_rev_parse_head() -> TestResult<()> {
        let repo = test_helpers::init_repo_with_base_file()?;
        let expected = test_helpers::git(&repo, &["rev-parse", "--verify", "HEAD"])?;

        let actual = test_helpers::with_repo_cwd(&repo, head_sha)?;
        assert_eq!(actual, expected);
        Ok(())
    }

    #[test]
    fn resolve_commit_resolves_branch_to_commit_sha() -> TestResult<()> {
        let repo = test_helpers::init_repo_with_base_file()?;
        test_helpers::git(&repo, &["checkout", "-b", "feature"])?;
        test_helpers::write_file(&repo, "feature.txt", "feature\n")?;
        test_helpers::commit_all(&repo, "feature commit")?;

        let expected = test_helpers::git(&repo, &["rev-parse", "--verify", "feature^{commit}"])?;
        let actual = test_helpers::with_repo_cwd(&repo, || resolve_commit("feature"))?;
        assert_eq!(actual, expected);
        Ok(())
    }

    #[test]
    fn resolve_ref_resolves_reference_to_commit_sha() -> TestResult<()> {
        let repo = test_helpers::init_repo_with_base_file()?;
        let expected = test_helpers::git(&repo, &["rev-parse", "--verify", "main^{commit}"])?;

        let actual = test_helpers::with_repo_cwd(&repo, || resolve_ref("main"))?;
        assert_eq!(actual, expected);
        Ok(())
    }

    #[test]
    fn refs_pointing_to_lists_local_and_remote_refs_sorted_without_origin_head() -> TestResult<()> {
        let repo = test_helpers::setup_remote_with_feature()?;
        let feature_sha = test_helpers::git(&repo, &["rev-parse", "feature"])?;

        let refs = test_helpers::with_repo_cwd(&repo, || refs_pointing_to(&feature_sha))?;

        assert_eq!(refs, vec!["feature", "origin/feature"]);
        assert!(!refs.iter().any(|r| r == "origin/HEAD"));
        Ok(())
    }

    #[test]
    fn parent_sha_returns_first_parent_of_commit() -> TestResult<()> {
        let repo = test_helpers::init_repo_with_base_file()?;

        test_helpers::git(&repo, &["checkout", "-b", "feature"])?;
        test_helpers::write_file(&repo, "feature.txt", "feature\n")?;
        test_helpers::commit_all(&repo, "feature commit")?;
        test_helpers::git(&repo, &["checkout", "main"])?;
        test_helpers::write_file(&repo, "main.txt", "main\n")?;
        test_helpers::commit_all(&repo, "main commit")?;

        let main_before_merge = test_helpers::git(&repo, &["rev-parse", "HEAD"])?;
        test_helpers::git(
            &repo,
            &["merge", "--no-ff", "-m", "merge feature", "feature"],
        )?;

        let merge_sha = test_helpers::git(&repo, &["rev-parse", "HEAD"])?;
        let actual_parent = test_helpers::with_repo_cwd(&repo, || parent_sha(&merge_sha))?;
        assert_eq!(actual_parent, main_before_merge);
        Ok(())
    }
}
