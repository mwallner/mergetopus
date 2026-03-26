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
    std::env::temp_dir().join(format!(
        "mergetopus-test-status-{ts}-{}",
        std::process::id()
    ))
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

fn assert_single_default_worktree(repo: &Path) -> TestResult<()> {
    let out = git(repo, &["worktree", "list", "--porcelain"])?;
    let count = out.lines().filter(|l| l.starts_with("worktree ")).count();
    if count != 1 {
        return Err(format!("expected exactly one worktree, found {count}\n{out}").into());
    }
    Ok(())
}

fn init_repo() -> TestResult<std::path::PathBuf> {
    let repo = unique_temp_repo_dir();
    fs::create_dir_all(&repo)?;

    git(&repo, &["init"])?;
    git(&repo, &["config", "commit.gpgsign", "false"])?;
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

fn integration_branch() -> &'static str {
    "_mmm/main/feature/integration"
}

fn slice_branch() -> &'static str {
    "_mmm/main/feature/slice1"
}

/// Verifies status output for an in-progress integration: source identity, pending slice count, paths, and resolve hint.
#[test]
fn release_b_status_reports_integration_and_pending_slice() -> TestResult<()> {
    let repo = setup_single_conflict_repo()?;
    assert_single_default_worktree(&repo)?;

    let create = mergetopus(&repo, &["feature", "--quiet"])?;
    assert!(
        create.status.success(),
        "initial mergetopus run failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&create.stdout),
        String::from_utf8_lossy(&create.stderr)
    );

    let status = mergetopus(&repo, &["--quiet", "status", integration_branch()])?;
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
    assert_single_default_worktree(&repo)?;

    Ok(())
}
