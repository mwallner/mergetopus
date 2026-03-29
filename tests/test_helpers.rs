use anyhow::Context;
use std::env;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

static CWD_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

pub fn run(cmd: &mut Command) -> TestResult<Output> {
    let out = cmd.output()?;
    Ok(out)
}

pub fn git(repo: &Path, args: &[&str]) -> TestResult<String> {
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

pub fn mergetopus(repo: &Path, args: &[&str]) -> TestResult<Output> {
    let bin = option_env!("CARGO_BIN_EXE_mergetopus")
        .ok_or("CARGO_BIN_EXE_mergetopus is not available in this test context")?;
    run(Command::new(bin).args(args).current_dir(repo))
}

pub fn unique_temp_repo_dir() -> std::path::PathBuf {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("mergetopus-test-{ts}-{}", std::process::id()))
}

pub fn init_repo() -> TestResult<std::path::PathBuf> {
    let repo = unique_temp_repo_dir();
    fs::create_dir_all(&repo)?;

    git(&repo, &["init"])?;
    git(&repo, &["config", "commit.gpgsign", "false"])?;
    git(&repo, &["config", "user.name", "Mergetopus Tests"])?;
    git(&repo, &["config", "user.email", "tests@example.com"])?;
    git(&repo, &["checkout", "-B", "main"])?;

    Ok(repo)
}

pub fn write_file(repo: &Path, rel: &str, content: &str) -> TestResult<()> {
    let path = repo.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)?;
    Ok(())
}

pub fn commit_all(repo: &Path, message: &str) -> TestResult<()> {
    git(repo, &["add", "."])?;
    git(repo, &["commit", "-m", message])?;
    Ok(())
}

pub fn setup_single_conflict_repo() -> TestResult<std::path::PathBuf> {
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

pub fn assert_single_default_worktree(repo: &Path) -> TestResult<()> {
    let out = git(repo, &["worktree", "list", "--porcelain"])?;
    let count = out.lines().filter(|l| l.starts_with("worktree ")).count();
    if count != 1 {
        return Err(format!("expected exactly one worktree, found {count}\n{out}").into());
    }
    Ok(())
}

pub fn add_origin_remote_with_feature(repo: &Path) -> TestResult<()> {
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

pub fn setup_remote_conflict_repo_without_local_feature() -> TestResult<std::path::PathBuf> {
    let repo = setup_single_conflict_repo()?;
    add_origin_remote_with_feature(&repo)?;
    git(&repo, &["checkout", "main"])?;
    git(&repo, &["branch", "-D", "feature"])?;
    Ok(repo)
}

pub fn setup_two_conflicts_repo() -> TestResult<std::path::PathBuf> {
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

pub fn setup_single_conflict_repo_with_named_source(
    source_branch: &str,
) -> TestResult<std::path::PathBuf> {
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

pub fn cwd_lock() -> &'static Mutex<()> {
    CWD_LOCK.get_or_init(|| Mutex::new(()))
}

pub fn with_repo_cwd<T>(repo: &Path, f: impl FnOnce() -> anyhow::Result<T>) -> TestResult<T> {
    let _guard = cwd_lock().lock().map_err(|_| "failed to lock cwd mutex")?;
    let previous = env::current_dir().context("failed to read current working directory")?;
    env::set_current_dir(repo)
        .with_context(|| format!("failed to switch to '{}'", repo.display()))?;

    let result = f().map_err(|err| -> Box<dyn std::error::Error> { err.into() });
    env::set_current_dir(&previous)
        .with_context(|| format!("failed to restore cwd to '{}'", previous.display()))?;
    result
}

pub fn init_repo_with_base_file() -> TestResult<std::path::PathBuf> {
    let repo = init_repo()?;
    write_file(&repo, "base.txt", "base\n")?;
    commit_all(&repo, "base")?;
    Ok(repo)
}

pub fn setup_remote_with_feature() -> TestResult<std::path::PathBuf> {
    let repo = init_repo_with_base_file()?;
    git(&repo, &["checkout", "-b", "feature"])?;
    write_file(&repo, "feature.txt", "feature\n")?;
    commit_all(&repo, "feature commit")?;
    git(&repo, &["checkout", "main"])?;
    add_origin_remote_with_feature(&repo)?;
    Ok(repo)
}
