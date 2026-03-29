use crate::git_ops::run_git;
use anyhow::{Context, Result, bail};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct WorktreeEntry {
    path: PathBuf,
    branch: Option<String>,
}

fn parse_worktree_entries(porcelain: &str) -> Vec<WorktreeEntry> {
    let mut entries = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_branch: Option<String> = None;

    for line in porcelain.lines() {
        if let Some(rest) = line.strip_prefix("worktree ") {
            if let Some(path) = current_path.take() {
                entries.push(WorktreeEntry {
                    path,
                    branch: current_branch.take(),
                });
            }
            current_path = Some(PathBuf::from(rest.trim()));
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
        entries.push(WorktreeEntry {
            path,
            branch: current_branch,
        });
    }

    entries
}

pub fn list_worktree_entries() -> Result<Vec<WorktreeEntry>> {
    let out = run_git(&["worktree", "list", "--porcelain"])?;
    Ok(parse_worktree_entries(&out))
}

pub fn has_existing_linked_worktrees(entries: &[WorktreeEntry]) -> bool {
    // A repo always has one worktree (the current/main checkout).
    // Enable worktree-specific logic only when additional worktrees exist.
    entries.len() > 1
}

pub fn find_worktree_for_branch(entries: &[WorktreeEntry], branch: &str) -> Option<PathBuf> {
    entries
        .iter()
        .find(|e| e.branch.as_deref() == Some(branch))
        .map(|e| e.path.clone())
}

fn nearest_common_parent(a: &Path, b: &Path) -> Option<PathBuf> {
    let a_parts = a.components().collect::<Vec<_>>();
    let b_parts = b.components().collect::<Vec<_>>();
    let mut common = PathBuf::new();

    for (left, right) in a_parts.iter().zip(b_parts.iter()) {
        if left != right {
            break;
        }
        common.push(left.as_os_str());
    }

    if common.as_os_str().is_empty() {
        None
    } else {
        Some(common)
    }
}

fn normalize_existing_path(path: &Path) -> PathBuf {
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

    #[cfg(target_os = "windows")]
    {
        let canonical_str = canonical.to_string_lossy();
        if let Some(rest) = canonical_str.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = canonical_str.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }

    canonical
}

fn repository_base_dir() -> Result<PathBuf> {
    let common_dir_raw = run_git(&["rev-parse", "--git-common-dir"])?;
    let common_dir_path = PathBuf::from(common_dir_raw);
    let abs_common_dir = if common_dir_path.is_absolute() {
        common_dir_path
    } else {
        env::current_dir()
            .context("failed to read current directory")?
            .join(common_dir_path)
    };

    let Some(repo_base) = abs_common_dir.parent() else {
        bail!(
            "failed to infer repository base from git common dir '{}'",
            abs_common_dir.display()
        );
    };

    Ok(repo_base.to_path_buf())
}

fn fallback_worktree_base_dir() -> Result<PathBuf> {
    let repo_base = repository_base_dir()?;
    Ok(repo_base
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or(repo_base))
}

fn infer_worktree_base_dir(entries: &[WorktreeEntry]) -> Result<PathBuf> {
    let paths = entries
        .iter()
        .map(|e| normalize_existing_path(&e.path))
        .collect::<Vec<_>>();

    if paths.len() >= 2 {
        // Prefer two existing linked worktree paths (excluding the current directory) when possible.
        let cwd = normalize_existing_path(
            &env::current_dir().context("failed to read current directory")?,
        );
        let non_current = paths
            .iter()
            .filter(|p| **p != cwd)
            .cloned()
            .collect::<Vec<_>>();

        if non_current.len() >= 2 {
            if let Some(common) = nearest_common_parent(&non_current[0], &non_current[1]) {
                return Ok(common);
            }
        } else if let Some(common) = nearest_common_parent(&paths[0], &paths[1]) {
            return Ok(common);
        }
    }

    fallback_worktree_base_dir()
}

fn branch_to_worktree_leaf(branch: &str) -> String {
    let mut out = String::with_capacity(branch.len());
    for c in branch.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "branch".to_string()
    } else {
        out
    }
}

fn pick_new_worktree_path(base: &Path, branch: &str, entries: &[WorktreeEntry]) -> PathBuf {
    let base_name = format!("mergetopus-{}", branch_to_worktree_leaf(branch));
    let known = entries.iter().map(|e| e.path.clone()).collect::<Vec<_>>();

    for idx in 0..1000usize {
        let leaf = if idx == 0 {
            base_name.clone()
        } else {
            format!("{base_name}-{idx}")
        };
        let candidate = base.join(leaf);
        if known.iter().any(|p| p == &candidate) {
            continue;
        }
        if !candidate.exists() {
            return candidate;
        }
    }

    base.join(format!("{}-fallback", base_name))
}

pub fn switch_to_dir(path: &Path) -> Result<()> {
    env::set_current_dir(path).with_context(|| {
        format!(
            "failed to switch to worktree directory '{}'",
            path.display()
        )
    })
}

pub fn ensure_worktree_for_existing_branch(
    branch: &str,
    entries: &[WorktreeEntry],
) -> Result<PathBuf> {
    if let Some(path) = find_worktree_for_branch(entries, branch) {
        return Ok(path);
    }

    let base = infer_worktree_base_dir(entries)?;
    fs::create_dir_all(&base).with_context(|| {
        format!(
            "failed to create inferred worktree base '{}'",
            base.display()
        )
    })?;

    let new_path = pick_new_worktree_path(&base, branch, entries);
    run_git(&["worktree", "add", &new_path.to_string_lossy(), branch]).with_context(|| {
        format!(
            "failed to create worktree '{}' for branch '{}'",
            new_path.display(),
            branch
        )
    })?;

    Ok(new_path)
}

pub fn ensure_worktree_for_branch_reset(
    branch: &str,
    at: &str,
    entries: &[WorktreeEntry],
) -> Result<PathBuf> {
    if let Some(path) = find_worktree_for_branch(entries, branch) {
        return Ok(path);
    }

    let base = infer_worktree_base_dir(entries)?;
    fs::create_dir_all(&base).with_context(|| {
        format!(
            "failed to create inferred worktree base '{}'",
            base.display()
        )
    })?;

    let new_path = pick_new_worktree_path(&base, branch, entries);
    run_git(&[
        "worktree",
        "add",
        "-B",
        branch,
        &new_path.to_string_lossy(),
        at,
    ])
    .with_context(|| {
        format!(
            "failed to create/reset worktree '{}' for branch '{}' at '{}'",
            new_path.display(),
            branch,
            at
        )
    })?;

    Ok(new_path)
}
