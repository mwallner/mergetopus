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

/// Run `git merge --no-ff --no-commit <source>` and return its combined
/// stdout/stderr so callers can parse `CONFLICT (...)` descriptions.
pub fn merge_no_commit(source: &str) -> Result<String> {
    let (ok, stdout, stderr) = run_git_allow_failure(&["merge", "--no-ff", "--no-commit", source])?;
    let combined = if stderr.is_empty() {
        stdout
    } else if stdout.is_empty() {
        stderr.clone()
    } else {
        format!("{stdout}\n{stderr}")
    };

    if ok {
        return Ok(combined);
    }

    // Expected conflict path: merge exits non-zero but leaves MERGE_HEAD.
    if merge_in_progress()? {
        return Ok(combined);
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

/// Reconstruct the `CONFLICT (...)` description lines for a merge between
/// two commits without touching the index or worktree, via
/// `git merge-tree --write-tree`. Used to recover conflict topology when
/// resuming an already in-progress slice merge whose original merge output
/// was never captured. When `chosen_base` is given, the merge is pinned to
/// that single base (`--merge-base`, git >= 2.42) so the reconstruction
/// agrees with Mergetopus's base decision instead of git's virtual base;
/// falls back to git's own base selection on older versions. Returns an
/// empty string when the probe fails; callers then fall back to single-path
/// grouping.
pub fn merge_conflict_messages(
    commit_a: &str,
    commit_b: &str,
    chosen_base: Option<&str>,
) -> Result<String> {
    let run = |base_arg: Option<&String>| -> Result<(bool, String)> {
        let mut args: Vec<&str> = vec!["merge-tree", "--write-tree"];
        if let Some(b) = base_arg {
            args.push(b);
        }
        args.push(commit_a);
        args.push(commit_b);
        let (ok, stdout, _stderr) = run_git_allow_failure(&args)?;
        Ok((ok, stdout))
    };

    let pinned = chosen_base.map(|b| format!("--merge-base={b}"));
    let (mut _ok, mut stdout) = run(pinned.as_ref())?;
    if stdout.is_empty() && pinned.is_some() {
        // Older git without --merge-base: retry with git's own base choice.
        let (_, stdout2) = run(None)?;
        stdout = stdout2;
    }
    let messages = stdout
        .lines()
        .filter(|l| l.starts_with("CONFLICT"))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(messages)
}

pub fn merge_base(a: &str, b: &str) -> Result<String> {
    run_git(&["merge-base", a, b])
}

/// All best common ancestors of `a` and `b`. Criss-cross histories can have
/// more than one; `merge_base` then reports only a single (arbitrary) pick
/// while git's own merge builds a virtual base from all of them.
pub fn merge_bases(a: &str, b: &str) -> Result<Vec<String>> {
    let out = run_git(&["merge-base", "--all", a, b])?;
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support as test_helpers;

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
    fn merge_bases_single_and_criss_cross() -> TestResult<()> {
        let repo = test_helpers::setup_single_conflict_repo()?;

        let single = test_helpers::with_repo_cwd(&repo, || merge_bases("main", "feature"))?;
        assert_eq!(
            single.len(),
            1,
            "linear divergence has one base: {single:?}"
        );

        // Build a criss-cross: both tips merge the other's original commit.
        let b1 = test_helpers::git(&repo, &["rev-parse", "main"])?;
        let b2 = test_helpers::git(&repo, &["rev-parse", "feature"])?;
        test_helpers::git(&repo, &["checkout", "main"])?;
        test_helpers::git(&repo, &["merge", "--no-commit", b2.trim()]).ok();
        test_helpers::write_file(&repo, "conflict.txt", "x resolved\n")?;
        test_helpers::git(&repo, &["add", "conflict.txt"])?;
        test_helpers::git(&repo, &["commit", "-m", "X"])?;
        test_helpers::git(&repo, &["checkout", "feature"])?;
        test_helpers::git(&repo, &["merge", "--no-commit", b1.trim()]).ok();
        test_helpers::write_file(&repo, "conflict.txt", "y resolved\n")?;
        test_helpers::git(&repo, &["add", "conflict.txt"])?;
        test_helpers::git(&repo, &["commit", "-m", "Y"])?;

        let x = test_helpers::git(&repo, &["rev-parse", "main"])?;
        let y = test_helpers::git(&repo, &["rev-parse", "feature"])?;
        let bases = test_helpers::with_repo_cwd(&repo, || merge_bases(x.trim(), y.trim()))?;
        assert_eq!(
            bases.len(),
            2,
            "criss-cross must list both bases: {bases:?}"
        );
        let default = test_helpers::with_repo_cwd(&repo, || merge_base(x.trim(), y.trim()))?;
        assert!(
            bases.contains(&default),
            "the merge-base default must be one of --all"
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
