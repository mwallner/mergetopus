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
            "refs/heads/main_mw_int_feature",
        ],
    );
    assert!(integration_exists.is_ok());

    let slice_exists = git(
        &repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            "refs/heads/main_mw_int_feature_slice1",
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

    let slice = "main_mw_int_feature_slice1";
    let before = git(&repo, &["rev-list", "--count", slice])?;

    let resolve_stage_only = mergetopus(&repo, &["--quiet", "resolve", slice])?;
    assert!(
        resolve_stage_only.status.success(),
        "resolve (stage-only) failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&resolve_stage_only.stdout),
        String::from_utf8_lossy(&resolve_stage_only.stderr)
    );

    let after_stage_only = git(&repo, &["rev-list", "--count", slice])?;
    assert_eq!(
        before, after_stage_only,
        "stage-only resolve must not commit"
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

    let after_commit = git(&repo, &["rev-list", "--count", slice])?;
    let before_count: i64 = before.parse()?;
    let after_count: i64 = after_commit.parse()?;
    assert_eq!(
        after_count,
        before_count + 1,
        "--commit must create one commit"
    );

    let staged_after = git(&repo, &["diff", "--cached", "--name-only"])?;
    assert!(
        staged_after.is_empty(),
        "index must be clean after --commit"
    );

    Ok(())
}

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

    git(&repo, &["checkout", "main_mw_int_feature"])?;
    git(
        &repo,
        &[
            "merge",
            "--no-ff",
            "-m",
            "merge resolved slice",
            "main_mw_int_feature_slice1",
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

    let consolidated_branch = "main_mw_int_feature_consolidated";
    let consolidated_parents = git(&repo, &["show", "-s", "--format=%P", consolidated_branch])?;
    let parent_list = consolidated_parents
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert_eq!(parent_list.len(), 2, "consolidated commit must be a merge commit");
    assert_eq!(parent_list[0], original_sha, "first parent must be original branch head");
    assert_eq!(parent_list[1], source_sha, "second parent must be source branch head used for integration");

    let consolidated_tree = git(&repo, &["rev-parse", &format!("{consolidated_branch}^{{tree}}")])?;
    let integration_tree = git(&repo, &["rev-parse", "main_mw_int_feature^{tree}"])?;
    assert_eq!(
        consolidated_tree, integration_tree,
        "consolidated commit tree must match final integration branch state"
    );

    Ok(())
}
