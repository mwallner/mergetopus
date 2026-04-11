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

/// Helper: creates integration + slice branches for a single-conflict repo,
/// pushes everything to a bare origin remote, and deletes the local integration
/// and slice branches — simulating a second workstation that only has remote refs.
fn setup_remote_only_integration_repo() -> TestResult<std::path::PathBuf> {
    let repo = test_helpers::setup_single_conflict_repo()?;
    test_helpers::add_origin_remote_with_feature(&repo)?;

    let create = test_helpers::mergetopus(&repo, &["feature", "--quiet"])?;
    assert!(
        create.status.success(),
        "initial mergetopus run failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&create.stdout),
        String::from_utf8_lossy(&create.stderr)
    );

    // Push integration and slice branches to remote.
    test_helpers::git(&repo, &["push", "-u", "origin", integration_branch()])?;
    test_helpers::git(&repo, &["push", "-u", "origin", slice_branch()])?;

    // Switch to main and delete local integration/slice branches.
    test_helpers::git(&repo, &["checkout", "main"])?;
    test_helpers::git(&repo, &["branch", "-D", integration_branch()])?;
    test_helpers::git(&repo, &["branch", "-D", slice_branch()])?;

    Ok(repo)
}

/// Person B (remote-only) can discover and report status for an integration
/// branch that only exists on the remote.
#[test]
fn release_b_status_discovers_remote_only_integration_branch() -> TestResult<()> {
    let repo = setup_remote_only_integration_repo()?;

    let status = test_helpers::mergetopus(&repo, &["--quiet", "status"])?;
    assert!(
        status.status.success(),
        "status command failed with remote-only branches:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&status.stdout),
        String::from_utf8_lossy(&status.stderr)
    );

    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(
        stdout.contains(&format!("Integration branch: {}", integration_branch())),
        "expected integration branch in output:\n{stdout}"
    );
    assert!(
        stdout.contains("Pending slices: 1"),
        "expected 1 pending slice:\n{stdout}"
    );
    assert!(
        stdout.contains("detected paths: conflict.txt"),
        "expected conflict.txt in detected paths:\n{stdout}"
    );

    Ok(())
}

/// Person B can discover a remote-only integration branch via explicit source arg.
#[test]
fn release_b_status_with_source_arg_finds_remote_only_integration() -> TestResult<()> {
    let repo = setup_remote_only_integration_repo()?;

    let status = test_helpers::mergetopus(&repo, &["--quiet", "status", "feature"])?;
    assert!(
        status.status.success(),
        "status command failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&status.stdout),
        String::from_utf8_lossy(&status.stderr)
    );

    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(
        stdout.contains(&format!("Integration branch: {}", integration_branch())),
        "expected integration branch in output:\n{stdout}"
    );

    Ok(())
}

/// Person B can resolve a remote-only slice branch.
#[test]
fn release_b_resolve_works_with_remote_only_slice() -> TestResult<()> {
    let repo = setup_remote_only_integration_repo()?;

    test_helpers::git(&repo, &["config", "merge.tool", "copybase"])?;
    test_helpers::git(
        &repo,
        &[
            "config",
            "mergetool.copybase.cmd",
            configured_copybase_cmd(),
        ],
    )?;

    let resolve = test_helpers::mergetopus(
        &repo,
        &["--quiet", "resolve", "--commit", slice_branch()],
    )?;
    assert!(
        resolve.status.success(),
        "resolve failed with remote-only slice:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&resolve.stdout),
        String::from_utf8_lossy(&resolve.stderr)
    );

    // After resolve, the integration and slice branches should exist locally.
    let int_exists = test_helpers::git(
        &repo,
        &[
            "show-ref",
            "--verify",
            &format!("refs/heads/{}", integration_branch()),
        ],
    );
    assert!(
        int_exists.is_ok(),
        "integration branch should be materialized locally after resolve"
    );

    let slice_exists = test_helpers::git(
        &repo,
        &[
            "show-ref",
            "--verify",
            &format!("refs/heads/{}", slice_branch()),
        ],
    );
    assert!(
        slice_exists.is_ok(),
        "slice branch should be materialized locally after resolve"
    );

    Ok(())
}

/// Person B can run merge workflow to resume an existing remote-only
/// integration branch.
#[test]
fn release_b_merge_workflow_resumes_remote_only_integration() -> TestResult<()> {
    let repo = setup_remote_only_integration_repo()?;

    // Running merge workflow with the source should detect the existing
    // remote-only integration branch and resume (not create a new one).
    let result = test_helpers::mergetopus(&repo, &["--quiet", "feature"])?;
    assert!(
        result.status.success(),
        "merge workflow failed with remote-only integration:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );

    let stdout = String::from_utf8_lossy(&result.stdout);
    // Should detect the existing integration and report pending slices,
    // not create a brand new integration branch.
    assert!(
        stdout.contains("pending") || stdout.contains("Existing slice merge status"),
        "expected to resume existing integration:\n{stdout}"
    );

    Ok(())
}
