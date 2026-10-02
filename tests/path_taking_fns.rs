//! `_in` variants must resolve against the given repo, never the ambient
//! CWD (overlay-plan task 0b acceptance). These tests run with the process
//! CWD on the *mergetopus* repository itself, so any CWD leakage would show
//! up as the wrong branch/ref set.

use std::path::{Path, PathBuf};
use std::process::Command;

fn git_in(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test Committer")
        .env("GIT_COMMITTER_EMAIL", "committer@example.com")
        .output()
        .expect("git is required");
    assert!(out.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
}

fn seeded_repo() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git_in(&repo, &["init", "-b", "main", "."]);
    git_in(&repo, &["config", "user.email", "t@t.com"]);
    git_in(&repo, &["config", "user.name", "T"]);
    std::fs::write(repo.join("a.txt"), "one").unwrap();
    git_in(&repo, &["add", "."]);
    git_in(&repo, &["-c", "commit.gpgsign=false", "commit", "-m", "init"]);
    git_in(&repo, &["branch", "_mmm/main/feature_x/integration"]);
    git_in(&repo, &["branch", "_mmm/main/feature_x/slice1"]);
    git_in(&repo, &["branch", "_mmm/main/feature_x/slice2"]);
    (dir, repo)
}

#[test]
fn run_git_in_ignores_ambient_cwd() {
    let (_tmp, repo) = seeded_repo();
    let toplevel = mergetopus::git_ops::run_git_in(&repo, &["rev-parse", "--show-toplevel"])
        .unwrap();
    let toplevel = Path::new(&toplevel);
    assert!(
        toplevel.ends_with("repo"),
        "expected the temp repo, got {toplevel:?} (ambient CWD leaked)"
    );
}

#[test]
fn current_branch_in_ignores_ambient_cwd() {
    let (_tmp, repo) = seeded_repo();
    assert_eq!(mergetopus::git_ops::current_branch_in(&repo).unwrap(), "main");
}

#[test]
fn list_local_branches_in_ignores_ambient_cwd() {
    let (_tmp, repo) = seeded_repo();
    let branches = mergetopus::git_ops::list_local_branches_in(&repo).unwrap();
    assert_eq!(
        branches,
        vec![
            "_mmm/main/feature_x/integration",
            "_mmm/main/feature_x/slice1",
            "_mmm/main/feature_x/slice2",
            "main",
        ]
    );
}

#[test]
fn list_slice_branches_in_ignores_ambient_cwd() {
    let (_tmp, repo) = seeded_repo();
    let slices = mergetopus::git_ops::list_slice_branches_for_integration_in(
        &repo,
        "_mmm/main/feature_x/integration",
    )
    .unwrap();
    assert_eq!(
        slices,
        vec!["_mmm/main/feature_x/slice1", "_mmm/main/feature_x/slice2"]
    );
}
