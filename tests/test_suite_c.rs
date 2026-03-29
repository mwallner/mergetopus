//! suite C integration tests for worktree-oriented behavior:
//! when worktree mode is engaged and how target worktree locations are inferred.

use std::fs;
use std::path::{Path, PathBuf};
mod test_helpers;
use test_helpers::{git, mergetopus, setup_single_conflict_repo, unique_temp_repo_dir};

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

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

fn worktree_debug_dump(repo: &Path) -> String {
    git(repo, &["worktree", "list", "--porcelain"])
        .unwrap_or_else(|e| format!("<failed to list worktrees: {e}>"))
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
        "mergetopus run failed:\nrepo: {}\nhelper_path: {}\nstdout:\n{}\nstderr:\n{}\nworktrees:\n{}",
        repo.display(),
        helper_path.display(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
        worktree_debug_dump(&repo)
    );

    let integration_path = branch_worktree_path(&repo, "_mmm/main/feature/integration")?;
    let slice_path = branch_worktree_path(&repo, "_mmm/main/feature/slice1")?;

    assert!(
        integration_path != repo,
        "integration branch should be placed in a dedicated worktree\nrepo: {}\nintegration_path: {}\nworktrees:\n{}",
        repo.display(),
        integration_path.display(),
        worktree_debug_dump(&repo)
    );
    assert!(
        slice_path != repo,
        "slice branch should be placed in a dedicated worktree\nrepo: {}\nslice_path: {}\nworktrees:\n{}",
        repo.display(),
        slice_path.display(),
        worktree_debug_dump(&repo)
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
        "mergetopus run failed:\nrepo: {}\ninferred_base: {}\nwt_a: {}\nwt_b: {}\nstdout:\n{}\nstderr:\n{}\nworktrees:\n{}",
        repo.display(),
        inferred_base.display(),
        wt_a.display(),
        wt_b.display(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
        worktree_debug_dump(&repo)
    );

    let integration_path = branch_worktree_path(&repo, "_mmm/main/feature/integration")?;
    assert!(
        integration_path.starts_with(&inferred_base),
        "expected integration worktree under inferred common base '{}' but found '{}'\nrepo: {}\nwt_a: {}\nwt_b: {}\nworktrees:\n{}",
        inferred_base.display(),
        integration_path.display(),
        repo.display(),
        wt_a.display(),
        wt_b.display(),
        worktree_debug_dump(&repo)
    );

    Ok(())
}
