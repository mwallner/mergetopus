//! suite B integration tests focused on status reporting:
//! integration metadata, pending slice counts, detected paths, and user guidance output.

mod test_helpers;

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

fn configured_copybase_cmd() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "type \"%BASE%\" > \"%MERGED%\""
    }
    #[cfg(not(target_os = "windows"))]
    {
        "cp \"$BASE\" \"$MERGED\""
    }
}

fn integration_branch() -> &'static str {
    "_mmm/main/feature/integration"
}

fn slice_branch() -> &'static str {
    "_mmm/main/feature/slice1"
}

/// Verifies status output for an in-progress integration: source identity, pending slice count, paths, and resolve hint.
#[test]
fn release_b_status_reports_integration_and_pending_slice() -> TestResult<()> {
    let repo = test_helpers::setup_single_conflict_repo()?;
    test_helpers::assert_single_default_worktree(&repo)?;

    let create = test_helpers::mergetopus(&repo, &["feature", "--quiet"])?;
    assert!(
        create.status.success(),
        "initial mergetopus run failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&create.stdout),
        String::from_utf8_lossy(&create.stderr)
    );

    let status = test_helpers::mergetopus(&repo, &["--quiet", "status", integration_branch()])?;
    assert!(
        status.status.success(),
        "status command failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&status.stdout),
        String::from_utf8_lossy(&status.stderr)
    );

    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(stdout.contains(&format!("Integration branch: {}", integration_branch())));
    assert!(stdout.contains("Source ref: feature"));
    assert!(stdout.contains("Pending slices: 1"));
    assert!(stdout.contains("detected paths: conflict.txt"));
    assert!(stdout.contains(&format!("mergetopus resolve {}", slice_branch())));
    test_helpers::assert_single_default_worktree(&repo)?;

    Ok(())
}

/// Verifies status keeps source ref/SHA after a committed resolve merge.
#[test]
fn release_b_status_uses_initial_partial_merge_metadata_after_resolve_commit() -> TestResult<()> {
    let repo = test_helpers::setup_single_conflict_repo()?;

    let create = test_helpers::mergetopus(&repo, &["feature", "--quiet"])?;
    assert!(
        create.status.success(),
        "initial mergetopus run failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&create.stdout),
        String::from_utf8_lossy(&create.stderr)
    );

    test_helpers::git(&repo, &["config", "merge.tool", "copybase"])?;
    test_helpers::git(
        &repo,
        &["config", "mergetool.copybase.cmd", configured_copybase_cmd()],
    )?;

    let resolve = test_helpers::mergetopus(
        &repo,
        &["--quiet", "resolve", "--commit", slice_branch()],
    )?;
    assert!(
        resolve.status.success(),
        "resolve --commit failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&resolve.stdout),
        String::from_utf8_lossy(&resolve.stderr)
    );

    let expected_source_sha = test_helpers::git(&repo, &["rev-parse", "feature"])?;

    let status = test_helpers::mergetopus(&repo, &["--quiet", "status", "feature"])?;
    assert!(
        status.status.success(),
        "status command failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&status.stdout),
        String::from_utf8_lossy(&status.stderr)
    );

    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(stdout.contains("Source ref: feature"));
    assert!(stdout.contains(&format!("Source SHA: {expected_source_sha}")));
    assert!(stdout.contains("Pending slices: 0"));
    assert!(stdout.contains("mergetopus feature --yes"));

    Ok(())
}
