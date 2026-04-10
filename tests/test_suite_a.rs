//! suite A integration tests for core merge workflow behavior:
//! branch creation, reruns, slice base ancestry, resolve semantics, and source selection.

use std::process::Command;

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

mod test_helpers;

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

fn kokomeco_branch() -> &'static str {
    "_mmm/main/feature/kokomeco"
}

/// Verifies first-run branch creation and idempotent rerun behavior for a basic conflict scenario.
#[test]
fn release_a_creates_integration_and_slice_and_supports_rerun() -> TestResult<()> {
    let repo = test_helpers::setup_single_conflict_repo()?;
    test_helpers::assert_single_default_worktree(&repo)?;

    let first = test_helpers::mergetopus(&repo, &["feature", "--quiet"])?;
    assert!(
        first.status.success(),
        "first run failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );

    let integration_exists = test_helpers::git(
        &repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            "refs/heads/_mmm/main/feature/integration",
        ],
    );
    assert!(integration_exists.is_ok());

    let slice_exists = test_helpers::git(
        &repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            "refs/heads/_mmm/main/feature/slice1",
        ],
    );
    assert!(slice_exists.is_ok());

    test_helpers::git(&repo, &["checkout", "main"])?;
    let rerun = test_helpers::mergetopus(&repo, &["feature", "--quiet"])?;
    assert!(
        rerun.status.success(),
        "rerun failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&rerun.stdout),
        String::from_utf8_lossy(&rerun.stderr)
    );
    test_helpers::assert_single_default_worktree(&repo)?;

    Ok(())
}

/// Ensures default one-file slices are created from merge-base, not from current branch HEAD.
#[test]
fn release_a_slice_parent_is_merge_base_for_default_slice() -> TestResult<()> {
    let repo = test_helpers::setup_single_conflict_repo()?;

    let run_out = test_helpers::mergetopus(&repo, &["feature", "--quiet"])?;
    assert!(
        run_out.status.success(),
        "mergetopus run failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_out.stdout),
        String::from_utf8_lossy(&run_out.stderr)
    );

    let head_sha_before = test_helpers::git(&repo, &["rev-parse", "main"])?;
    let source_sha = test_helpers::git(&repo, &["rev-parse", "feature"])?;
    let expected_merge_base =
        test_helpers::git(&repo, &["merge-base", &head_sha_before, &source_sha])?;
    let slice_parent = test_helpers::git(&repo, &["rev-parse", &format!("{}^", slice_branch())])?;

    assert_eq!(
        slice_parent, expected_merge_base,
        "default slice parent must be merge-base(main, feature)"
    );

    Ok(())
}

/// Ensures explicitly grouped slice branches also use merge-base as their parent.
#[test]
fn release_a_slice_parent_is_merge_base_for_explicit_slice_group() -> TestResult<()> {
    let repo = test_helpers::setup_single_conflict_repo()?;

    let run_out = test_helpers::mergetopus(
        &repo,
        &["feature", "--quiet", "--select-paths", "conflict.txt"],
    )?;
    assert!(
        run_out.status.success(),
        "mergetopus run with explicit slice failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_out.stdout),
        String::from_utf8_lossy(&run_out.stderr)
    );

    let head_sha_before = test_helpers::git(&repo, &["rev-parse", "main"])?;
    let source_sha = test_helpers::git(&repo, &["rev-parse", "feature"])?;
    let expected_merge_base =
        test_helpers::git(&repo, &["merge-base", &head_sha_before, &source_sha])?;
    let slice_parent = test_helpers::git(&repo, &["rev-parse", &format!("{}^", slice_branch())])?;

    assert_eq!(
        slice_parent, expected_merge_base,
        "explicit slice parent must be merge-base(main, feature)"
    );

    Ok(())
}

/// Validates resolve behavior: stage-only resolve does not commit, and --commit writes one integration merge commit.
#[test]
fn release_a_resolve_stages_by_default_and_commits_with_flag() -> TestResult<()> {
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
        &[
            "config",
            "mergetool.copybase.cmd",
            configured_copybase_cmd(),
        ],
    )?;

    let slice = slice_branch();
    let slice_before = test_helpers::git(&repo, &["rev-list", "--first-parent", "--count", slice])?;
    let integration_before = test_helpers::git(
        &repo,
        &[
            "rev-list",
            "--first-parent",
            "--count",
            integration_branch(),
        ],
    )?;

    let resolve_stage_only = test_helpers::mergetopus(&repo, &["--quiet", "resolve", slice])?;
    assert!(
        resolve_stage_only.status.success(),
        "resolve (stage-only) failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&resolve_stage_only.stdout),
        String::from_utf8_lossy(&resolve_stage_only.stderr)
    );

    let after_stage_only = test_helpers::git(
        &repo,
        &[
            "rev-list",
            "--first-parent",
            "--count",
            integration_branch(),
        ],
    )?;
    assert_eq!(
        integration_before, after_stage_only,
        "stage-only resolve must not commit"
    );

    let current_branch = test_helpers::git(&repo, &["branch", "--show-current"])?;
    assert_eq!(
        current_branch,
        integration_branch(),
        "resolve must operate on the integration branch"
    );

    let merge_head = test_helpers::git(&repo, &["rev-parse", "--verify", "MERGE_HEAD"])?;
    let slice_sha = test_helpers::git(&repo, &["rev-parse", slice])?;
    assert_eq!(
        merge_head, slice_sha,
        "MERGE_HEAD must point at the slice tip"
    );

    let staged = test_helpers::git(&repo, &["diff", "--cached", "--name-only"])?;
    assert!(
        staged.contains("conflict.txt"),
        "expected conflict.txt to be staged, got: {staged}"
    );

    let resolve_commit =
        test_helpers::mergetopus(&repo, &["--quiet", "resolve", "--commit", slice])?;
    assert!(
        resolve_commit.status.success(),
        "resolve (--commit) failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&resolve_commit.stdout),
        String::from_utf8_lossy(&resolve_commit.stderr)
    );

    let integration_after_commit = test_helpers::git(
        &repo,
        &[
            "rev-list",
            "--first-parent",
            "--count",
            integration_branch(),
        ],
    )?;
    let before_count: i64 = integration_before.parse()?;
    let after_count: i64 = integration_after_commit.parse()?;
    assert_eq!(
        after_count,
        before_count + 1,
        "--commit must create one merge commit on the integration branch"
    );

    let slice_after = test_helpers::git(&repo, &["rev-list", "--count", slice])?;
    assert_eq!(
        slice_before, slice_after,
        "resolve must not create commits on the slice branch"
    );

    let staged_after = test_helpers::git(&repo, &["diff", "--cached", "--name-only"])?;
    assert!(
        staged_after.is_empty(),
        "index must be clean after --commit"
    );

    Ok(())
}

/// Confirms resolve still works when slice commit metadata does not include path trailers.
#[test]
fn release_a_resolve_works_without_slice_path_metadata() -> TestResult<()> {
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
        &[
            "config",
            "mergetool.copybase.cmd",
            configured_copybase_cmd(),
        ],
    )?;

    test_helpers::git(&repo, &["checkout", slice_branch()])?;
    test_helpers::git(
        &repo,
        &[
            "commit",
            "--amend",
            "-m",
            "Mergetopus - slice1 from feature (theirs)\n\nNo path trailers.",
        ],
    )?;

    let resolve_stage_only =
        test_helpers::mergetopus(&repo, &["--quiet", "resolve", slice_branch()])?;
    assert!(
        resolve_stage_only.status.success(),
        "resolve without metadata failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&resolve_stage_only.stdout),
        String::from_utf8_lossy(&resolve_stage_only.stderr)
    );

    let current_branch = test_helpers::git(&repo, &["branch", "--show-current"])?;
    assert_eq!(current_branch, integration_branch());

    let staged = test_helpers::git(&repo, &["diff", "--cached", "--name-only"])?;
    assert!(
        staged.contains("conflict.txt"),
        "expected conflict.txt to be staged, got: {staged}"
    );

    Ok(())
}

/// Checks that merges between unrelated histories fail early with actionable guidance.
#[test]
fn release_a_fails_fast_on_unrelated_histories() -> TestResult<()> {
    let repo = test_helpers::init_repo()?;

    test_helpers::write_file(&repo, "main.txt", "main\n")?;
    test_helpers::commit_all(&repo, "main root")?;

    test_helpers::git(&repo, &["checkout", "--orphan", "other"])?;
    let _ = test_helpers::git(&repo, &["rm", "-rf", "."]);
    test_helpers::write_file(&repo, "other.txt", "other\n")?;
    test_helpers::commit_all(&repo, "other root")?;

    test_helpers::git(&repo, &["checkout", "main"])?;

    let out = test_helpers::mergetopus(&repo, &["other", "--quiet"])?;
    assert!(
        !out.status.success(),
        "expected command to fail on unrelated histories"
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("failed before entering conflict resolution"),
        "stderr did not include merge failure guidance:\n{stderr}"
    );

    Ok(())
}

/// Verifies kokomeco commit topology: parent order and tree content must match expected integration state.
#[test]
fn consolidation_uses_original_and_source_as_parents_and_integration_tree() -> TestResult<()> {
    let repo = test_helpers::setup_single_conflict_repo()?;

    let source_sha = test_helpers::git(&repo, &["rev-parse", "feature"])?;
    let original_sha = test_helpers::git(&repo, &["rev-parse", "main"])?;

    let create = test_helpers::mergetopus(&repo, &["feature", "--quiet"])?;
    assert!(
        create.status.success(),
        "initial mergetopus run failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&create.stdout),
        String::from_utf8_lossy(&create.stderr)
    );

    test_helpers::git(&repo, &["checkout", integration_branch()])?;
    test_helpers::git(
        &repo,
        &[
            "merge",
            "--no-ff",
            "-s",
            "ours",
            "-m",
            "merge resolved slice",
            slice_branch(),
        ],
    )?;

    test_helpers::git(&repo, &["checkout", "main"])?;
    let consolidate = test_helpers::mergetopus(&repo, &["feature", "--quiet", "--yes"])?;
    assert!(
        consolidate.status.success(),
        "consolidation run failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&consolidate.stdout),
        String::from_utf8_lossy(&consolidate.stderr)
    );

    let consolidated_branch = kokomeco_branch();
    let consolidated_parents =
        test_helpers::git(&repo, &["show", "-s", "--format=%P", consolidated_branch])?;
    let parent_list = consolidated_parents
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert_eq!(
        parent_list.len(),
        2,
        "consolidated commit must be a merge commit"
    );
    assert_eq!(
        parent_list[0], original_sha,
        "first parent must be original branch head"
    );
    assert_eq!(
        parent_list[1], source_sha,
        "second parent must be source branch head used for integration"
    );

    let consolidated_tree = test_helpers::git(
        &repo,
        &["rev-parse", &format!("{consolidated_branch}^{{tree}}")],
    )?;
    let integration_tree = test_helpers::git(
        &repo,
        &["rev-parse", &format!("{}^{{tree}}", integration_branch())],
    )?;
    assert_eq!(
        consolidated_tree, integration_tree,
        "consolidated commit tree must match final integration branch state"
    );

    Ok(())
}

/// Verifies selecting a remote-only source creates a local tracking branch and proceeds.
#[test]
fn remote_source_creates_local_tracking_branch_when_missing() -> TestResult<()> {
    let repo = test_helpers::setup_remote_conflict_repo_without_local_feature()?;

    let out = test_helpers::mergetopus(&repo, &["origin/feature", "--quiet"])?;
    assert!(
        out.status.success(),
        "run with remote source failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(
            "Using remote source 'origin/feature' via new local tracking branch 'feature'"
        ),
        "expected remote normalization note, got:\n{stdout}"
    );

    let local_feature_exists = test_helpers::git(
        &repo,
        &["show-ref", "--verify", "--quiet", "refs/heads/feature"],
    );
    assert!(
        local_feature_exists.is_ok(),
        "expected local tracking branch 'feature' to exist"
    );

    let integration_exists = test_helpers::git(
        &repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            "refs/heads/_mmm/main/feature/integration",
        ],
    );
    assert!(integration_exists.is_ok());

    Ok(())
}

/// Verifies selecting a remote source uses the existing local branch when local and remote are in sync.
#[test]
fn remote_source_uses_local_when_in_sync() -> TestResult<()> {
    let repo = test_helpers::setup_single_conflict_repo()?;
    test_helpers::add_origin_remote_with_feature(&repo)?;

    let out = test_helpers::mergetopus(&repo, &["origin/feature", "--quiet"])?;
    assert!(
        out.status.success(),
        "run with in-sync local+remote should succeed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Using existing local branch 'feature' (in sync with 'origin/feature')"),
        "expected in-sync message, got:\n{stdout}"
    );

    let integration_exists = test_helpers::git(
        &repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            "refs/heads/_mmm/main/feature/integration",
        ],
    );
    assert!(integration_exists.is_ok());

    Ok(())
}

/// Verifies selecting a remote source fails when the matching local branch has diverged.
#[test]
fn remote_source_stops_when_local_diverged() -> TestResult<()> {
    let repo = test_helpers::setup_single_conflict_repo()?;
    test_helpers::add_origin_remote_with_feature(&repo)?;

    test_helpers::git(&repo, &["checkout", "feature"])?;
    test_helpers::write_file(&repo, "newfile.txt", "local divergence\n")?;
    test_helpers::commit_all(&repo, "local commit ahead of remote")?;

    let out = test_helpers::mergetopus(&repo, &["origin/feature", "--quiet"])?;
    assert!(
        !out.status.success(),
        "expected run to stop when local diverged from remote"
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("has diverged from its remote counterpart"),
        "expected divergence warning, got:\n{stderr}"
    );

    Ok(())
}

/// Ensures reruns stop early when a kokomeco branch already exists and do not mutate integration/kokomeco state.
#[test]
fn merge_stops_when_kokomeco_already_exists() -> TestResult<()> {
    let repo = test_helpers::setup_single_conflict_repo()?;

    let create = test_helpers::mergetopus(&repo, &["feature", "--quiet"])?;
    assert!(
        create.status.success(),
        "initial mergetopus run failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&create.stdout),
        String::from_utf8_lossy(&create.stderr)
    );

    test_helpers::git(&repo, &["checkout", integration_branch()])?;
    test_helpers::git(
        &repo,
        &[
            "merge",
            "--no-ff",
            "-s",
            "ours",
            "-m",
            "merge resolved slice",
            slice_branch(),
        ],
    )?;

    test_helpers::git(&repo, &["checkout", "main"])?;
    let consolidate = test_helpers::mergetopus(&repo, &["feature", "--quiet", "--yes"])?;
    assert!(
        consolidate.status.success(),
        "consolidation run failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&consolidate.stdout),
        String::from_utf8_lossy(&consolidate.stderr)
    );

    let integration_before = test_helpers::git(
        &repo,
        &[
            "rev-list",
            "--first-parent",
            "--count",
            integration_branch(),
        ],
    )?;
    let kokomeco_sha_before = test_helpers::git(&repo, &["rev-parse", kokomeco_branch()])?;

    test_helpers::git(&repo, &["checkout", "main"])?;

    let rerun = test_helpers::mergetopus(&repo, &["feature", "--quiet"])?;
    assert!(
        rerun.status.success(),
        "rerun failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&rerun.stdout),
        String::from_utf8_lossy(&rerun.stderr)
    );

    let rerun_stdout = String::from_utf8_lossy(&rerun.stdout);
    assert!(
        rerun_stdout.contains("Kokomeco branch already exists"),
        "expected kokomeco early-exit message, got:\n{}",
        rerun_stdout
    );

    let integration_after = test_helpers::git(
        &repo,
        &[
            "rev-list",
            "--first-parent",
            "--count",
            integration_branch(),
        ],
    )?;
    let kokomeco_sha_after = test_helpers::git(&repo, &["rev-parse", kokomeco_branch()])?;

    assert_eq!(
        integration_before, integration_after,
        "integration branch should not change when kokomeco exists"
    );
    assert_eq!(
        kokomeco_sha_before, kokomeco_sha_after,
        "kokomeco branch should not change on blocked rerun"
    );

    Ok(())
}

/// Ensures failed conflict-path selection triggers cleanup: restore target checkout and delete fresh integration branch.
#[test]
fn conflict_selection_cancellation_cleans_up_integration_branch() -> TestResult<()> {
    let repo = test_helpers::setup_single_conflict_repo()?;

    let out = test_helpers::mergetopus(
        &repo,
        &["feature", "--quiet", "--select-paths", "nonexistent.txt"],
    )?;
    assert!(
        !out.status.success(),
        "expected failure when --select-paths specifies a non-conflicted path"
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("is not in conflicted file list"),
        "expected path validation error, got:\n{stderr}"
    );

    let integration_exists = test_helpers::git(
        &repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            "refs/heads/_mmm/main/feature/integration",
        ],
    );
    assert!(
        integration_exists.is_err(),
        "integration branch should be deleted after conflict selection cancellation"
    );

    let current_branch = test_helpers::git(&repo, &["branch", "--show-current"])?;
    assert_eq!(
        current_branch, "main",
        "should be back on main branch after cleanup"
    );

    Ok(())
}

/// Ensures HERE fails fast when no merge is currently in progress.
#[test]
fn here_requires_in_progress_merge() -> TestResult<()> {
    let repo = test_helpers::setup_single_conflict_repo()?;

    let out = test_helpers::mergetopus(&repo, &["--quiet", "HERE"])?;
    assert!(
        !out.status.success(),
        "expected HERE to fail without in-progress merge"
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("HERE requires an in-progress merge"),
        "expected precondition error, got:\n{stderr}"
    );

    Ok(())
}

/// Ensures HERE takes over an in-progress merge, preserves already-resolved paths,
/// and creates slices only for remaining unresolved conflicts.
#[test]
fn here_takes_over_and_slices_only_remaining_conflicts() -> TestResult<()> {
    let repo = test_helpers::setup_two_conflicts_repo()?;

    let merge = test_helpers::run(
        Command::new("git")
            .args(["merge", "feature"])
            .current_dir(&repo),
    )?;
    assert!(
        !merge.status.success(),
        "expected manual merge to stop on conflicts"
    );

    test_helpers::write_file(&repo, "a.txt", "manually resolved a\n")?;
    test_helpers::git(&repo, &["add", "a.txt"])?;

    let here = test_helpers::mergetopus(&repo, &["--quiet", "--select-paths", "b.txt", "HERE"])?;
    assert!(
        here.status.success(),
        "HERE takeover failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&here.stdout),
        String::from_utf8_lossy(&here.stderr)
    );

    let current_branch = test_helpers::git(&repo, &["branch", "--show-current"])?;
    assert_eq!(current_branch, integration_branch());

    let a_on_integration =
        test_helpers::git(&repo, &["show", &format!("{}:a.txt", integration_branch())])?;
    assert_eq!(a_on_integration, "manually resolved a");

    let slice_exists = test_helpers::git(
        &repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            "refs/heads/_mmm/main/feature/slice1",
        ],
    );
    assert!(
        slice_exists.is_ok(),
        "expected slice branch for remaining conflict"
    );

    let slice_msg = test_helpers::git(&repo, &["show", "-s", "--format=%B", slice_branch()])?;
    assert!(
        slice_msg.contains("* b.txt"),
        "slice should target remaining unresolved path b.txt:\n{slice_msg}"
    );
    assert!(
        !slice_msg.contains("* a.txt"),
        "slice should not include already resolved path a.txt:\n{slice_msg}"
    );

    Ok(())
}

/// Verifies command-like source branch names can be merged with --source.
#[test]
fn source_option_disambiguates_branch_named_resolve() -> TestResult<()> {
    let repo = test_helpers::setup_single_conflict_repo_with_named_source("resolve")?;

    let out = test_helpers::mergetopus(&repo, &["--source", "resolve", "--quiet"])?;
    assert!(
        out.status.success(),
        "run with --source resolve failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let integration_exists = test_helpers::git(
        &repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            "refs/heads/_mmm/main/resolve/integration",
        ],
    );
    assert!(integration_exists.is_ok());

    let slice_exists = test_helpers::git(
        &repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            "refs/heads/_mmm/main/resolve/slice1",
        ],
    );
    assert!(slice_exists.is_ok());

    Ok(())
}

/// Verifies positional SOURCE still works for regular non-conflicting names.
#[test]
fn positional_source_still_works_for_non_command_name() -> TestResult<()> {
    let repo = test_helpers::setup_single_conflict_repo_with_named_source("feature_x")?;

    let out = test_helpers::mergetopus(&repo, &["feature_x", "--quiet"])?;
    assert!(
        out.status.success(),
        "run with positional feature_x failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let integration_exists = test_helpers::git(
        &repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            "refs/heads/_mmm/main/feature_x/integration",
        ],
    );
    assert!(integration_exists.is_ok());

    Ok(())
}

/// Ensures initial partial-merge detection is stable when a non-slice task
/// branch merge appears between two slice merges on the integration branch.
#[test]
fn initial_partial_merge_detection_ignores_task_branch_merge_between_slice_merges() -> TestResult<()>
{
    let repo = test_helpers::setup_two_conflicts_repo()?;

    let create = test_helpers::mergetopus(&repo, &["feature", "--quiet"])?;
    assert!(
        create.status.success(),
        "initial mergetopus run failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&create.stdout),
        String::from_utf8_lossy(&create.stderr)
    );

    let source_sha = test_helpers::git(&repo, &["rev-parse", "feature"])?;

    // Merge first slice.
    test_helpers::git(&repo, &["checkout", integration_branch()])?;
    test_helpers::git(
        &repo,
        &[
            "merge",
            "--no-ff",
            "-s",
            "ours",
            "-m",
            "merge resolved slice1",
            slice_branch(),
        ],
    )?;

    // Create and merge a non-slice task branch between the slice merges.
    test_helpers::git(&repo, &["checkout", "main"])?;
    test_helpers::git(&repo, &["checkout", "-b", "Task-FeatureX"])?;
    test_helpers::write_file(&repo, "task.txt", "task branch payload\n")?;
    test_helpers::commit_all(&repo, "Task-FeatureX change")?;

    test_helpers::git(&repo, &["checkout", integration_branch()])?;
    test_helpers::git(
        &repo,
        &[
            "merge",
            "--no-ff",
            "-m",
            "Merge branch 'Task-FeatureX' into integration",
            "Task-FeatureX",
        ],
    )?;

    // Merge second slice after the task branch merge.
    test_helpers::git(
        &repo,
        &[
            "merge",
            "--no-ff",
            "-s",
            "ours",
            "-m",
            "merge resolved slice2",
            "_mmm/main/feature/slice2",
        ],
    )?;

    test_helpers::git(&repo, &["checkout", "main"])?;

    // status uses first_mergetopus_partial_merge_commit internally.
    let status = test_helpers::mergetopus(&repo, &["--quiet", "status", "feature"])?;
    assert!(
        status.status.success(),
        "status command failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&status.stdout),
        String::from_utf8_lossy(&status.stderr)
    );

    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(stdout.contains("Source ref: feature"));
    assert!(stdout.contains(&format!("Source SHA: {source_sha}")));
    assert!(
        !stdout.contains("Source ref: (unknown)"),
        "status must keep source metadata from initial partial merge:\n{stdout}"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// resolve: trustExitCode / conflict marker detection
// ---------------------------------------------------------------------------

/// Helper: configure a merge tool that copies BASE to MERGED (resolves the conflict) but exits
/// with the given exit code.
fn configure_resolve_tool(repo: &std::path::Path, exit_code: i32) -> TestResult<()> {
    test_helpers::git(repo, &["config", "merge.tool", "testmerge"])?;
    let cmd = if exit_code == 0 {
        configured_copybase_cmd().to_string()
    } else {
        #[cfg(target_os = "windows")]
        {
            format!("type \"%BASE%\" > \"%MERGED%\" & exit /b {exit_code}")
        }
        #[cfg(not(target_os = "windows"))]
        {
            format!("cp \"$BASE\" \"$MERGED\"; exit {exit_code}")
        }
    };
    test_helpers::git(repo, &["config", "mergetool.testmerge.cmd", &cmd])?;
    Ok(())
}

/// Helper: configure a merge tool that does NOT resolve the file (just exits).
fn configure_noop_tool(repo: &std::path::Path, exit_code: i32) -> TestResult<()> {
    test_helpers::git(repo, &["config", "merge.tool", "testmerge"])?;
    #[cfg(target_os = "windows")]
    let cmd = format!("exit /b {exit_code}");
    #[cfg(not(target_os = "windows"))]
    let cmd = format!("exit {exit_code}");
    test_helpers::git(repo, &["config", "mergetool.testmerge.cmd", &cmd])?;
    Ok(())
}

/// Helper: configure a merge tool that writes conflict markers into MERGED and exits 0.
fn configure_marker_tool(repo: &std::path::Path) -> TestResult<()> {
    test_helpers::git(repo, &["config", "merge.tool", "testmerge"])?;
    #[cfg(target_os = "windows")]
    let cmd = r#"(echo ^^^^^^^ HEAD & echo ours & echo ======= & echo theirs & echo ^^^^^^^ branch) > "%MERGED%""#;
    #[cfg(not(target_os = "windows"))]
    let cmd =
        r#"printf '<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> branch\n' > "$MERGED""#;
    test_helpers::git(repo, &["config", "mergetool.testmerge.cmd", cmd])?;
    Ok(())
}

/// Helper: set up a repo with a conflict and run mergetopus to create integration + slices.
fn setup_resolve_scenario() -> TestResult<(std::path::PathBuf, String)> {
    let repo = test_helpers::setup_single_conflict_repo()?;
    let create = test_helpers::mergetopus(&repo, &["feature", "--quiet"])?;
    assert!(
        create.status.success(),
        "initial mergetopus run failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&create.stdout),
        String::from_utf8_lossy(&create.stderr)
    );
    Ok((repo, slice_branch().to_string()))
}

/// trustExitCode=true, tool exits 0 → file is staged.
#[test]
fn resolve_trust_exit_code_true_exit_zero_stages_file() -> TestResult<()> {
    let (repo, slice) = setup_resolve_scenario()?;
    configure_resolve_tool(&repo, 0)?;
    test_helpers::git(&repo, &["config", "mergetool.testmerge.trustExitCode", "true"])?;

    let out = test_helpers::mergetopus(&repo, &["--quiet", "resolve", &slice])?;
    assert!(
        out.status.success(),
        "resolve failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Staged 1 file(s)"),
        "expected file to be staged:\n{stdout}"
    );
    assert!(
        !stdout.contains("Skipped"),
        "should not skip when tool exits 0 with trustExitCode=true:\n{stdout}"
    );

    Ok(())
}

/// trustExitCode=true, tool exits non-zero → file is NOT staged.
#[test]
fn resolve_trust_exit_code_true_exit_nonzero_skips_file() -> TestResult<()> {
    let (repo, slice) = setup_resolve_scenario()?;
    configure_resolve_tool(&repo, 1)?;
    test_helpers::git(&repo, &["config", "mergetool.testmerge.trustExitCode", "true"])?;

    let out = test_helpers::mergetopus(&repo, &["--quiet", "resolve", &slice])?;
    assert!(
        out.status.success(),
        "resolve command itself should succeed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("Skipped 1 file(s)"),
        "expected file to be skipped:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("skipping staging (trustExitCode is enabled)"),
        "expected trustExitCode warning in stderr:\n{stderr}"
    );

    Ok(())
}

/// trustExitCode=false, tool exits non-zero → file is staged regardless.
#[test]
fn resolve_trust_exit_code_false_always_stages() -> TestResult<()> {
    let (repo, slice) = setup_resolve_scenario()?;
    configure_resolve_tool(&repo, 1)?;
    test_helpers::git(&repo, &["config", "mergetool.trustExitCode", "false"])?;

    let out = test_helpers::mergetopus(&repo, &["--quiet", "resolve", &slice])?;
    assert!(
        out.status.success(),
        "resolve failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Staged 1 file(s)"),
        "trustExitCode=false should always stage:\n{stdout}"
    );
    assert!(
        !stdout.contains("Skipped"),
        "trustExitCode=false should not skip:\n{stdout}"
    );

    Ok(())
}

/// trustExitCode unset, tool exits 0, no markers → file is staged (happy path).
#[test]
fn resolve_unset_trust_exit_code_clean_resolve_stages() -> TestResult<()> {
    let (repo, slice) = setup_resolve_scenario()?;
    configure_resolve_tool(&repo, 0)?;

    let out = test_helpers::mergetopus(&repo, &["--quiet", "resolve", &slice])?;
    assert!(
        out.status.success(),
        "resolve failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Staged 1 file(s)"),
        "clean resolve should stage:\n{stdout}"
    );
    assert!(
        !stdout.contains("Skipped"),
        "clean resolve should not skip:\n{stdout}"
    );

    Ok(())
}

/// trustExitCode unset, tool exits non-zero, --quiet → file is skipped.
#[test]
fn resolve_unset_trust_nonzero_exit_quiet_skips() -> TestResult<()> {
    let (repo, slice) = setup_resolve_scenario()?;
    configure_noop_tool(&repo, 1)?;

    let out = test_helpers::mergetopus(&repo, &["--quiet", "resolve", &slice])?;
    assert!(
        out.status.success(),
        "resolve command itself should succeed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("Skipped 1 file(s)"),
        "non-zero exit with unset trustExitCode in quiet mode should skip:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("skipping staging in --quiet mode"),
        "expected quiet-mode skip warning:\n{stderr}"
    );

    Ok(())
}

/// trustExitCode unset, tool exits 0 but conflict markers remain, --quiet → file is skipped.
#[test]
fn resolve_unset_trust_markers_remain_quiet_skips() -> TestResult<()> {
    let (repo, slice) = setup_resolve_scenario()?;
    configure_marker_tool(&repo)?;

    let out = test_helpers::mergetopus(&repo, &["--quiet", "resolve", &slice])?;
    assert!(
        out.status.success(),
        "resolve command itself should succeed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("Skipped 1 file(s)"),
        "conflict markers remaining with unset trustExitCode in quiet mode should skip:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("conflict markers remain"),
        "expected conflict markers warning:\n{stderr}"
    );

    Ok(())
}

/// Tool-specific trustExitCode overrides global setting.
#[test]
fn resolve_tool_specific_trust_overrides_global() -> TestResult<()> {
    let (repo, slice) = setup_resolve_scenario()?;
    configure_resolve_tool(&repo, 1)?;
    // Global says don't trust, tool-specific says trust
    test_helpers::git(&repo, &["config", "mergetool.trustExitCode", "false"])?;
    test_helpers::git(&repo, &["config", "mergetool.testmerge.trustExitCode", "true"])?;

    let out = test_helpers::mergetopus(&repo, &["--quiet", "resolve", &slice])?;
    assert!(
        out.status.success(),
        "resolve command itself should succeed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Tool-specific trustExitCode=true should win over global=false,
    // so non-zero exit → skip
    assert!(
        stdout.contains("Skipped 1 file(s)"),
        "tool-specific trust should override global:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    Ok(())
}
