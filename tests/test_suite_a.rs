use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

fn unique_temp_repo_dir() -> std::path::PathBuf {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("mergetopus-test-{ts}-{}", std::process::id()))
}

fn run(cmd: &mut Command) -> TestResult<Output> {
    let out = cmd.output()?;
    Ok(out)
}

fn git(repo: &Path, args: &[&str]) -> TestResult<String> {
    let out = run(Command::new("git").args(args).current_dir(repo))?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed:\nstdout:\n{}\nstderr:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
        .into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn mergetopus(repo: &Path, args: &[&str]) -> TestResult<Output> {
    let bin = env!("CARGO_BIN_EXE_mergetopus");
    run(Command::new(bin).args(args).current_dir(repo))
}

fn init_repo() -> TestResult<std::path::PathBuf> {
    let repo = unique_temp_repo_dir();
    fs::create_dir_all(&repo)?;

    git(&repo, &["init"])?;
    git(&repo, &["config", "user.name", "Mergetopus Tests"])?;
    git(&repo, &["config", "user.email", "tests@example.com"])?;
    git(&repo, &["checkout", "-B", "main"])?;

    Ok(repo)
}

fn write_file(repo: &Path, rel: &str, content: &str) -> TestResult<()> {
    let path = repo.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)?;
    Ok(())
}

fn commit_all(repo: &Path, message: &str) -> TestResult<()> {
    git(repo, &["add", "."])?;
    git(repo, &["commit", "-m", message])?;
    Ok(())
}

fn setup_single_conflict_repo() -> TestResult<std::path::PathBuf> {
    let repo = init_repo()?;

    write_file(&repo, "conflict.txt", "base\n")?;
    commit_all(&repo, "base")?;

    git(&repo, &["checkout", "-b", "feature"])?;
    write_file(&repo, "conflict.txt", "feature\n")?;
    commit_all(&repo, "feature change")?;

    git(&repo, &["checkout", "main"])?;
    write_file(&repo, "conflict.txt", "main\n")?;
    commit_all(&repo, "main change")?;

    Ok(repo)
}

fn add_origin_remote_with_feature(repo: &Path) -> TestResult<()> {
    let bare = unique_temp_repo_dir();
    fs::create_dir_all(&bare)?;
    git(
        repo,
        &["init", "--bare", bare.to_str().ok_or("invalid bare path")?],
    )?;

    git(
        repo,
        &[
            "remote",
            "add",
            "origin",
            bare.to_str().ok_or("invalid bare path")?,
        ],
    )?;
    git(repo, &["push", "-u", "origin", "main"])?;
    git(repo, &["push", "-u", "origin", "feature"])?;
    git(repo, &["fetch", "origin"])?;
    Ok(())
}

fn setup_remote_conflict_repo_without_local_feature() -> TestResult<std::path::PathBuf> {
    let repo = setup_single_conflict_repo()?;
    add_origin_remote_with_feature(&repo)?;
    git(&repo, &["checkout", "main"])?;
    git(&repo, &["branch", "-D", "feature"])?;
    Ok(repo)
}

fn setup_two_conflicts_repo() -> TestResult<std::path::PathBuf> {
    let repo = init_repo()?;

    write_file(&repo, "a.txt", "base a\n")?;
    write_file(&repo, "b.txt", "base b\n")?;
    commit_all(&repo, "base")?;

    git(&repo, &["checkout", "-b", "feature"])?;
    write_file(&repo, "a.txt", "feature a\n")?;
    write_file(&repo, "b.txt", "feature b\n")?;
    commit_all(&repo, "feature change")?;

    git(&repo, &["checkout", "main"])?;
    write_file(&repo, "a.txt", "main a\n")?;
    write_file(&repo, "b.txt", "main b\n")?;
    commit_all(&repo, "main change")?;

    Ok(repo)
}

fn setup_single_conflict_repo_with_named_source(source_branch: &str) -> TestResult<std::path::PathBuf> {
    let repo = init_repo()?;

    write_file(&repo, "conflict.txt", "base\n")?;
    commit_all(&repo, "base")?;

    git(&repo, &["checkout", "-b", source_branch])?;
    write_file(&repo, "conflict.txt", "feature\n")?;
    commit_all(&repo, "feature change")?;

    git(&repo, &["checkout", "main"])?;
    write_file(&repo, "conflict.txt", "main\n")?;
    commit_all(&repo, "main change")?;

    Ok(repo)
}

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
    let repo = setup_single_conflict_repo()?;

    let first = mergetopus(&repo, &["feature", "--quiet"])?;
    assert!(
        first.status.success(),
        "first run failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );

    let integration_exists = git(
        &repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            "refs/heads/_mmm/main/feature/integration",
        ],
    );
    assert!(integration_exists.is_ok());

    let slice_exists = git(
        &repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            "refs/heads/_mmm/main/feature/slice1",
        ],
    );
    assert!(slice_exists.is_ok());

    git(&repo, &["checkout", "main"])?;
    let rerun = mergetopus(&repo, &["feature", "--quiet"])?;
    assert!(
        rerun.status.success(),
        "rerun failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&rerun.stdout),
        String::from_utf8_lossy(&rerun.stderr)
    );

    Ok(())
}

/// Ensures default one-file slices are created from merge-base, not from current branch HEAD.
#[test]
fn release_a_slice_parent_is_merge_base_for_default_slice() -> TestResult<()> {
    let repo = setup_single_conflict_repo()?;

    let run_out = mergetopus(&repo, &["feature", "--quiet"])?;
    assert!(
        run_out.status.success(),
        "mergetopus run failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_out.stdout),
        String::from_utf8_lossy(&run_out.stderr)
    );

    let head_sha_before = git(&repo, &["rev-parse", "main"])?;
    let source_sha = git(&repo, &["rev-parse", "feature"])?;
    let expected_merge_base = git(&repo, &["merge-base", &head_sha_before, &source_sha])?;
    let slice_parent = git(&repo, &["rev-parse", &format!("{}^", slice_branch())])?;

    assert_eq!(
        slice_parent, expected_merge_base,
        "default slice parent must be merge-base(main, feature)"
    );

    Ok(())
}

/// Ensures explicitly grouped slice branches also use merge-base as their parent.
#[test]
fn release_a_slice_parent_is_merge_base_for_explicit_slice_group() -> TestResult<()> {
    let repo = setup_single_conflict_repo()?;

    let run_out = mergetopus(
        &repo,
        &["feature", "--quiet", "--select-paths", "conflict.txt"],
    )?;
    assert!(
        run_out.status.success(),
        "mergetopus run with explicit slice failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_out.stdout),
        String::from_utf8_lossy(&run_out.stderr)
    );

    let head_sha_before = git(&repo, &["rev-parse", "main"])?;
    let source_sha = git(&repo, &["rev-parse", "feature"])?;
    let expected_merge_base = git(&repo, &["merge-base", &head_sha_before, &source_sha])?;
    let slice_parent = git(&repo, &["rev-parse", &format!("{}^", slice_branch())])?;

    assert_eq!(
        slice_parent, expected_merge_base,
        "explicit slice parent must be merge-base(main, feature)"
    );

    Ok(())
}

/// Validates resolve behavior: stage-only resolve does not commit, and --commit writes one integration merge commit.
#[test]
fn release_a_resolve_stages_by_default_and_commits_with_flag() -> TestResult<()> {
    let repo = setup_single_conflict_repo()?;

    let create = mergetopus(&repo, &["feature", "--quiet"])?;
    assert!(
        create.status.success(),
        "initial mergetopus run failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&create.stdout),
        String::from_utf8_lossy(&create.stderr)
    );

    git(&repo, &["config", "merge.tool", "copybase"])?;
    git(
        &repo,
        &[
            "config",
            "mergetool.copybase.cmd",
            configured_copybase_cmd(),
        ],
    )?;

    let slice = slice_branch();
    let slice_before = git(&repo, &["rev-list", "--first-parent", "--count", slice])?;
    let integration_before = git(
        &repo,
        &[
            "rev-list",
            "--first-parent",
            "--count",
            integration_branch(),
        ],
    )?;

    let resolve_stage_only = mergetopus(&repo, &["--quiet", "resolve", slice])?;
    assert!(
        resolve_stage_only.status.success(),
        "resolve (stage-only) failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&resolve_stage_only.stdout),
        String::from_utf8_lossy(&resolve_stage_only.stderr)
    );

    let after_stage_only = git(
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

    let current_branch = git(&repo, &["branch", "--show-current"])?;
    assert_eq!(
        current_branch,
        integration_branch(),
        "resolve must operate on the integration branch"
    );

    let merge_head = git(&repo, &["rev-parse", "--verify", "MERGE_HEAD"])?;
    let slice_sha = git(&repo, &["rev-parse", slice])?;
    assert_eq!(
        merge_head, slice_sha,
        "MERGE_HEAD must point at the slice tip"
    );

    let staged = git(&repo, &["diff", "--cached", "--name-only"])?;
    assert!(
        staged.contains("conflict.txt"),
        "expected conflict.txt to be staged, got: {staged}"
    );

    let resolve_commit = mergetopus(&repo, &["--quiet", "resolve", "--commit", slice])?;
    assert!(
        resolve_commit.status.success(),
        "resolve (--commit) failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&resolve_commit.stdout),
        String::from_utf8_lossy(&resolve_commit.stderr)
    );

    let integration_after_commit = git(
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

    let slice_after = git(&repo, &["rev-list", "--count", slice])?;
    assert_eq!(
        slice_before, slice_after,
        "resolve must not create commits on the slice branch"
    );

    let staged_after = git(&repo, &["diff", "--cached", "--name-only"])?;
    assert!(
        staged_after.is_empty(),
        "index must be clean after --commit"
    );

    Ok(())
}

/// Confirms resolve still works when slice commit metadata does not include path trailers.
#[test]
fn release_a_resolve_works_without_slice_path_metadata() -> TestResult<()> {
    let repo = setup_single_conflict_repo()?;

    let create = mergetopus(&repo, &["feature", "--quiet"])?;
    assert!(
        create.status.success(),
        "initial mergetopus run failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&create.stdout),
        String::from_utf8_lossy(&create.stderr)
    );

    git(&repo, &["config", "merge.tool", "copybase"])?;
    git(
        &repo,
        &[
            "config",
            "mergetool.copybase.cmd",
            configured_copybase_cmd(),
        ],
    )?;

    git(&repo, &["checkout", slice_branch()])?;
    git(
        &repo,
        &[
            "commit",
            "--amend",
            "-m",
            "Mergetopus - slice1 from feature (theirs)\n\nNo path trailers.",
        ],
    )?;

    let resolve_stage_only = mergetopus(&repo, &["--quiet", "resolve", slice_branch()])?;
    assert!(
        resolve_stage_only.status.success(),
        "resolve without metadata failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&resolve_stage_only.stdout),
        String::from_utf8_lossy(&resolve_stage_only.stderr)
    );

    let current_branch = git(&repo, &["branch", "--show-current"])?;
    assert_eq!(current_branch, integration_branch());

    let staged = git(&repo, &["diff", "--cached", "--name-only"])?;
    assert!(
        staged.contains("conflict.txt"),
        "expected conflict.txt to be staged, got: {staged}"
    );

    Ok(())
}

/// Checks that merges between unrelated histories fail early with actionable guidance.
#[test]
fn release_a_fails_fast_on_unrelated_histories() -> TestResult<()> {
    let repo = init_repo()?;

    write_file(&repo, "main.txt", "main\n")?;
    commit_all(&repo, "main root")?;

    git(&repo, &["checkout", "--orphan", "other"])?;
    let _ = git(&repo, &["rm", "-rf", "."]);
    write_file(&repo, "other.txt", "other\n")?;
    commit_all(&repo, "other root")?;

    git(&repo, &["checkout", "main"])?;

    let out = mergetopus(&repo, &["other", "--quiet"])?;
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
    let repo = setup_single_conflict_repo()?;

    let source_sha = git(&repo, &["rev-parse", "feature"])?;
    let original_sha = git(&repo, &["rev-parse", "main"])?;

    let create = mergetopus(&repo, &["feature", "--quiet"])?;
    assert!(
        create.status.success(),
        "initial mergetopus run failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&create.stdout),
        String::from_utf8_lossy(&create.stderr)
    );

    git(&repo, &["checkout", integration_branch()])?;
    git(
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

    git(&repo, &["checkout", "main"])?;
    let consolidate = mergetopus(&repo, &["feature", "--quiet", "--yes"])?;
    assert!(
        consolidate.status.success(),
        "consolidation run failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&consolidate.stdout),
        String::from_utf8_lossy(&consolidate.stderr)
    );

    let consolidated_branch = kokomeco_branch();
    let consolidated_parents = git(&repo, &["show", "-s", "--format=%P", consolidated_branch])?;
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

    let consolidated_tree = git(
        &repo,
        &["rev-parse", &format!("{consolidated_branch}^{{tree}}")],
    )?;
    let integration_tree = git(
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
    let repo = setup_remote_conflict_repo_without_local_feature()?;

    let out = mergetopus(&repo, &["origin/feature", "--quiet"])?;
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

    let local_feature_exists = git(
        &repo,
        &["show-ref", "--verify", "--quiet", "refs/heads/feature"],
    );
    assert!(
        local_feature_exists.is_ok(),
        "expected local tracking branch 'feature' to exist"
    );

    let integration_exists = git(
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
    let repo = setup_single_conflict_repo()?;
    add_origin_remote_with_feature(&repo)?;

    let out = mergetopus(&repo, &["origin/feature", "--quiet"])?;
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

    let integration_exists = git(
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
    let repo = setup_single_conflict_repo()?;
    add_origin_remote_with_feature(&repo)?;

    git(&repo, &["checkout", "feature"])?;
    write_file(&repo, "newfile.txt", "local divergence\n")?;
    commit_all(&repo, "local commit ahead of remote")?;

    let out = mergetopus(&repo, &["origin/feature", "--quiet"])?;
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
    let repo = setup_single_conflict_repo()?;

    let create = mergetopus(&repo, &["feature", "--quiet"])?;
    assert!(
        create.status.success(),
        "initial mergetopus run failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&create.stdout),
        String::from_utf8_lossy(&create.stderr)
    );

    git(&repo, &["checkout", integration_branch()])?;
    git(
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

    git(&repo, &["checkout", "main"])?;
    let consolidate = mergetopus(&repo, &["feature", "--quiet", "--yes"])?;
    assert!(
        consolidate.status.success(),
        "consolidation run failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&consolidate.stdout),
        String::from_utf8_lossy(&consolidate.stderr)
    );

    let integration_before = git(
        &repo,
        &[
            "rev-list",
            "--first-parent",
            "--count",
            integration_branch(),
        ],
    )?;
    let kokomeco_sha_before = git(&repo, &["rev-parse", kokomeco_branch()])?;

    git(&repo, &["checkout", "main"])?;

    let rerun = mergetopus(&repo, &["feature", "--quiet"])?;
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

    let integration_after = git(
        &repo,
        &[
            "rev-list",
            "--first-parent",
            "--count",
            integration_branch(),
        ],
    )?;
    let kokomeco_sha_after = git(&repo, &["rev-parse", kokomeco_branch()])?;

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
    let repo = setup_single_conflict_repo()?;

    let out = mergetopus(
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

    let integration_exists = git(
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

    let current_branch = git(&repo, &["branch", "--show-current"])?;
    assert_eq!(
        current_branch, "main",
        "should be back on main branch after cleanup"
    );

    Ok(())
}

/// Ensures HERE fails fast when no merge is currently in progress.
#[test]
fn here_requires_in_progress_merge() -> TestResult<()> {
    let repo = setup_single_conflict_repo()?;

    let out = mergetopus(&repo, &["--quiet", "HERE"])?;
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
    let repo = setup_two_conflicts_repo()?;

    let merge = run(Command::new("git")
        .args(["merge", "feature"])
        .current_dir(&repo))?;
    assert!(
        !merge.status.success(),
        "expected manual merge to stop on conflicts"
    );

    write_file(&repo, "a.txt", "manually resolved a\n")?;
    git(&repo, &["add", "a.txt"])?;

    let here = mergetopus(&repo, &["--quiet", "--select-paths", "b.txt", "HERE"])?;
    assert!(
        here.status.success(),
        "HERE takeover failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&here.stdout),
        String::from_utf8_lossy(&here.stderr)
    );

    let current_branch = git(&repo, &["branch", "--show-current"])?;
    assert_eq!(current_branch, integration_branch());

    let a_on_integration = git(&repo, &["show", &format!("{}:a.txt", integration_branch())])?;
    assert_eq!(a_on_integration, "manually resolved a");

    let slice_exists = git(
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

    let slice_msg = git(&repo, &["show", "-s", "--format=%B", slice_branch()])?;
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
    let repo = setup_single_conflict_repo_with_named_source("resolve")?;

    let out = mergetopus(&repo, &["--source", "resolve", "--quiet"])?;
    assert!(
        out.status.success(),
        "run with --source resolve failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let integration_exists = git(
        &repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            "refs/heads/_mmm/main/resolve/integration",
        ],
    );
    assert!(integration_exists.is_ok());

    let slice_exists = git(
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
    let repo = setup_single_conflict_repo_with_named_source("feature_x")?;

    let out = mergetopus(&repo, &["feature_x", "--quiet"])?;
    assert!(
        out.status.success(),
        "run with positional feature_x failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let integration_exists = git(
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
