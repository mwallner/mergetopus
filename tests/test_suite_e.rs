//! suite E integration tests for `mergetopus push`:
//! happy path, source/target missing on remote, multiple remotes, partial pushes,
//! and name conflicts from previous merge attempts.

use std::fs;
type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

mod test_helpers;

// ── helpers ──────────────────────────────────────────────────────────────────

fn integration_branch() -> &'static str {
    "_mmm/main/feature/integration"
}

fn slice_branch() -> &'static str {
    "_mmm/main/feature/slice1"
}

fn kokomeco_branch() -> &'static str {
    "_mmm/main/feature/kokomeco"
}

/// Create a bare repo directory that acts as a remote; returns its path.
fn create_bare_remote(_name: &str) -> TestResult<std::path::PathBuf> {
    // Use unique_temp_repo_dir directly so every call gets a unique path
    // (the name is only for debugging context; uniqueness comes from the timestamp).
    let path = test_helpers::unique_temp_repo_dir();
    fs::create_dir_all(&path)?;
    test_helpers::git(&path, &["init", "--bare"])?;
    Ok(path)
}

/// Add a named remote pointing at `remote_path` in `repo`.
fn add_remote(repo: &std::path::Path, name: &str, remote_path: &std::path::Path) -> TestResult<()> {
    test_helpers::git(repo, &["remote", "add", name, remote_path.to_str().unwrap()])?;
    Ok(())
}

/// Push a branch from `repo` to `remote` by name.
fn push(repo: &std::path::Path, remote: &str, branch: &str) -> TestResult<()> {
    test_helpers::git(repo, &["push", remote, branch])?;
    Ok(())
}

/// Returns true when `refs/heads/<branch>` exists in the bare remote.
fn remote_has_branch(remote_path: &std::path::Path, branch: &str) -> TestResult<bool> {
    let result = test_helpers::git(
        remote_path,
        &["show-ref", "--verify", "--quiet", &format!("refs/heads/{branch}")],
    );
    Ok(result.is_ok())
}

/// Set up a conflict repo, run mergetopus to create integration+slice, then
/// add a bare remote with `main` and `feature` already pushed.
fn setup_pushed_source_target(
    remote_name: &str,
) -> TestResult<(std::path::PathBuf, std::path::PathBuf)> {
    let repo = test_helpers::setup_single_conflict_repo()?;

    // Create the integration branch
    let result = test_helpers::mergetopus(&repo, &["feature", "--quiet"])?;
    assert!(
        result.status.success(),
        "mergetopus setup failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );

    test_helpers::git(&repo, &["checkout", "main"])?;

    let bare = create_bare_remote(remote_name)?;
    add_remote(&repo, remote_name, &bare)?;
    push(&repo, remote_name, "main")?;

    // feature branch was created by setup_single_conflict_repo; push it directly.
    push(&repo, remote_name, "feature")?;
    test_helpers::git(&repo, &["fetch", remote_name])?;

    Ok((repo, bare))
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// Happy path: push integration + slice to a single remote from integration branch.
#[test]
fn push_integration_and_slice_to_single_remote() -> TestResult<()> {
    let (repo, bare) = setup_pushed_source_target("origin")?;

    // Checkout the integration branch so push uses it directly
    test_helpers::git(&repo, &["checkout", integration_branch()])?;

    let result = test_helpers::mergetopus(&repo, &["--quiet", "push"])?;
    assert!(
        result.status.success(),
        "push failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );

    assert!(
        remote_has_branch(&bare, integration_branch())?,
        "integration branch missing on remote"
    );
    assert!(
        remote_has_branch(&bare, slice_branch())?,
        "slice branch missing on remote"
    );
    // no kokomeco yet
    assert!(
        !remote_has_branch(&bare, kokomeco_branch())?,
        "kokomeco should not be on remote yet"
    );

    Ok(())
}

/// Error: source branch ('feature') not on remote.
#[test]
fn push_fails_when_source_not_on_remote() -> TestResult<()> {
    let repo = test_helpers::setup_single_conflict_repo()?;

    let result = test_helpers::mergetopus(&repo, &["feature", "--quiet"])?;
    assert!(result.status.success(), "mergetopus setup failed");

    test_helpers::git(&repo, &["checkout", "main"])?;

    let bare = create_bare_remote("origin-no-src")?;
    add_remote(&repo, "origin", &bare)?;
    push(&repo, "origin", "main")?;
    // deliberately do NOT push feature
    test_helpers::git(&repo, &["fetch", "origin"])?;

    test_helpers::git(&repo, &["checkout", integration_branch()])?;

    let result = test_helpers::mergetopus(&repo, &["--quiet", "push", "origin"])?;
    assert!(
        !result.status.success(),
        "expected push to fail when source missing on remote"
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("feature") && stderr.contains("not found on remote"),
        "unexpected error message: {stderr}"
    );

    Ok(())
}

/// Error: target branch ('main') not on remote.
#[test]
fn push_fails_when_target_not_on_remote() -> TestResult<()> {
    let repo = test_helpers::setup_single_conflict_repo()?;

    let result = test_helpers::mergetopus(&repo, &["feature", "--quiet"])?;
    assert!(result.status.success(), "mergetopus setup failed");

    // push feature but NOT main
    let bare = create_bare_remote("origin-no-tgt")?;
    add_remote(&repo, "origin", &bare)?;
    // push feature branch directly without switching to it
    push(&repo, "origin", "feature")?;
    test_helpers::git(&repo, &["fetch", "origin"])?;

    test_helpers::git(&repo, &["checkout", integration_branch()])?;

    let result = test_helpers::mergetopus(&repo, &["--quiet", "push", "origin"])?;
    assert!(
        !result.status.success(),
        "expected push to fail when target missing on remote"
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("main") && stderr.contains("not found on remote"),
        "unexpected error message: {stderr}"
    );

    Ok(())
}

/// Multiple remotes + explicit remote arg → succeeds in quiet mode.
#[test]
fn push_with_explicit_remote_when_multiple_configured() -> TestResult<()> {
    let (repo, bare) = setup_pushed_source_target("upstream")?;

    // Add a second remote that does NOT have main/feature
    let bare2 = create_bare_remote("other")?;
    add_remote(&repo, "other", &bare2)?;

    test_helpers::git(&repo, &["checkout", integration_branch()])?;

    let result = test_helpers::mergetopus(&repo, &["--quiet", "push", "upstream"])?;
    assert!(
        result.status.success(),
        "push failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );

    assert!(remote_has_branch(&bare, integration_branch())?);
    assert!(remote_has_branch(&bare, slice_branch())?);
    // second remote untouched
    assert!(!remote_has_branch(&bare2, integration_branch())?);

    Ok(())
}

/// Multiple remotes + no explicit remote + --quiet → error.
#[test]
fn push_quiet_with_multiple_remotes_and_no_arg_errors() -> TestResult<()> {
    let (repo, _bare) = setup_pushed_source_target("origin")?;

    let bare2 = create_bare_remote("backup")?;
    add_remote(&repo, "backup", &bare2)?;

    test_helpers::git(&repo, &["checkout", integration_branch()])?;

    let result = test_helpers::mergetopus(&repo, &["--quiet", "push"])?;
    assert!(
        !result.status.success(),
        "expected error when multiple remotes and no explicit remote"
    );

    Ok(())
}

/// Partial push: integration already on remote — force-with-lease updates it.
#[test]
fn push_force_updates_partially_pushed_branches() -> TestResult<()> {
    let (repo, bare) = setup_pushed_source_target("origin")?;

    // Pre-push only the integration branch
    test_helpers::git(&repo, &["checkout", "main"])?;
    push(&repo, "origin", integration_branch())?;
    test_helpers::git(&repo, &["fetch", "origin"])?;

    test_helpers::git(&repo, &["checkout", integration_branch()])?;

    let result = test_helpers::mergetopus(&repo, &["--quiet", "push", "origin"])?;
    assert!(
        result.status.success(),
        "push failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );

    assert!(remote_has_branch(&bare, integration_branch())?);
    assert!(remote_has_branch(&bare, slice_branch())?);

    Ok(())
}

/// Name conflict: leftover MMM branches from a previous failed attempt are
/// overwritten by force-with-lease.
#[test]
fn push_overwrites_leftover_branches_from_previous_attempt() -> TestResult<()> {
    let (repo, bare) = setup_pushed_source_target("origin")?;

    // Simulate a previous partial push: push integration+slice to remote
    push(&repo, "origin", integration_branch())?;
    push(&repo, "origin", slice_branch())?;
    test_helpers::git(&repo, &["fetch", "origin"])?;

    // Make a new commit on the integration branch (e.g. after a resolve step)
    test_helpers::git(&repo, &["checkout", integration_branch()])?;
    test_helpers::write_file(&repo, "extra.txt", "extra\n")?;
    test_helpers::commit_all(&repo, "post-resolve commit")?;

    let result = test_helpers::mergetopus(&repo, &["--quiet", "push", "origin"])?;
    assert!(
        result.status.success(),
        "push failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );

    // Verify the remote tip was updated (local sha == remote sha)
    let local_sha =
        test_helpers::git(&repo, &["rev-parse", integration_branch()])?;
    let remote_sha =
        test_helpers::git(&bare, &["rev-parse", integration_branch()])?;
    assert_eq!(
        local_sha, remote_sha,
        "remote was not updated to latest commit"
    );

    Ok(())
}

/// Kokomeco is pushed when it exists.
#[test]
fn push_includes_kokomeco_when_present() -> TestResult<()> {
    let (repo, bare) = setup_pushed_source_target("origin")?;

    // Manually create a kokomeco branch to simulate a completed consolidation
    test_helpers::git(&repo, &["checkout", integration_branch()])?;
    test_helpers::git(
        &repo,
        &["branch", kokomeco_branch(), integration_branch()],
    )?;

    let result = test_helpers::mergetopus(&repo, &["--quiet", "push", "origin"])?;
    assert!(
        result.status.success(),
        "push failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );

    assert!(
        remote_has_branch(&bare, kokomeco_branch())?,
        "kokomeco should have been pushed"
    );

    Ok(())
}
