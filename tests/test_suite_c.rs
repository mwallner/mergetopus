use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

fn unique_temp_repo_dir() -> PathBuf {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("mergetopus-test-worktree-{ts}-{}", std::process::id()))
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

fn init_repo() -> TestResult<PathBuf> {
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

fn setup_single_conflict_repo() -> TestResult<PathBuf> {
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

fn parse_worktree_branch_map(repo: &Path) -> TestResult<Vec<(PathBuf, Option<String>)>> {
    let out = git(repo, &["worktree", "list", "--porcelain"])?;
    let mut rows = Vec::new();

    let mut current_path: Option<PathBuf> = None;
    let mut current_branch: Option<String> = None;

    for line in out.lines() {
        if let Some(rest) = line.strip_prefix("worktree ") {
            if let Some(path) = current_path.take() {
                rows.push((path, current_branch.take()));
            }
            current_path = Some(PathBuf::from(rest));
            current_branch = None;
            continue;
        }

        if let Some(rest) = line.strip_prefix("branch ") {
            let value = rest.trim();
            if let Some(name) = value.strip_prefix("refs/heads/") {
                current_branch = Some(name.to_string());
            }
        }
    }

    if let Some(path) = current_path.take() {
        rows.push((path, current_branch));
    }

    Ok(rows)
}

fn branch_worktree_path(repo: &Path, branch: &str) -> TestResult<PathBuf> {
    let map = parse_worktree_branch_map(repo)?;
    let Some((path, _)) = map.into_iter().find(|(_, b)| b.as_deref() == Some(branch)) else {
        return Err(format!("branch '{branch}' is not checked out in any worktree").into());
    };
    Ok(path)
}

#[test]
fn release_c_uses_worktree_mode_only_when_worktrees_already_exist() -> TestResult<()> {
    let repo = setup_single_conflict_repo()?;

    let helper_path = unique_temp_repo_dir();
    fs::create_dir_all(&helper_path)?;
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-b",
            "wt_helper",
            helper_path.to_str().ok_or("invalid helper path")?,
            "main",
        ],
    )?;

    let out = mergetopus(&repo, &["feature", "--quiet"])?;
    assert!(
        out.status.success(),
        "mergetopus run failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let integration_path = branch_worktree_path(&repo, "_mmm/main/feature/integration")?;
    let slice_path = branch_worktree_path(&repo, "_mmm/main/feature/slice1")?;

    assert!(
        integration_path != repo,
        "integration branch should be placed in a dedicated worktree"
    );
    assert!(
        slice_path != repo,
        "slice branch should be placed in a dedicated worktree"
    );

    Ok(())
}

#[test]
fn release_c_infers_common_base_for_new_worktrees() -> TestResult<()> {
    let repo = setup_single_conflict_repo()?;

    let inferred_base = unique_temp_repo_dir();
    let wt_a = inferred_base.join("wta");
    let wt_b = inferred_base.join("wtb");
    fs::create_dir_all(&inferred_base)?;

    git(
        &repo,
        &[
            "worktree",
            "add",
            "-b",
            "wt_a",
            wt_a.to_str().ok_or("invalid wt_a path")?,
            "main",
        ],
    )?;
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-b",
            "wt_b",
            wt_b.to_str().ok_or("invalid wt_b path")?,
            "main",
        ],
    )?;

    let out = mergetopus(&repo, &["feature", "--quiet"])?;
    assert!(
        out.status.success(),
        "mergetopus run failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let integration_path = branch_worktree_path(&repo, "_mmm/main/feature/integration")?;
    assert!(
        integration_path.starts_with(&inferred_base),
        "expected integration worktree under inferred common base '{}' but found '{}'",
        inferred_base.display(),
        integration_path.display()
    );

    Ok(())
}
