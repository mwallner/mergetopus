use crate::git_ops::{run_git, ensure_longpaths_support};
use crate::win32_path::to_fs_path;
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
    let paths: Vec<PathBuf> = entries
        .iter()
        .map(|e| normalize_existing_path(&e.path))
        .collect();

    // The first entry from `git worktree list` is always the primary (main) worktree.
    // Exclude it from the anchor-pair search so that the inferred base stays stable even
    // after the CWD moves into a newly-created linked worktree (at which point the primary
    // worktree becomes a non-current entry and would otherwise be included in the pair,
    // yielding a wider common parent such as /tmp).
    let linked = paths.get(1..).unwrap_or_default();

    if linked.len() >= 2 {
        let cwd = normalize_existing_path(
            &env::current_dir().context("failed to read current directory")?,
        );
        let non_current: Vec<&PathBuf> = linked.iter().filter(|p| *p != &cwd).collect();

        // Prefer a pair that excludes the current directory; fall back to any two
        // linked worktrees when there are not enough non-current ones.
        let (a, b) = if non_current.len() >= 2 {
            (non_current[0], non_current[1])
        } else {
            (&linked[0], &linked[1])
        };
        if let Some(common) = nearest_common_parent(a, b) {
            return Ok(common);
        }
    } else if paths.len() == 2 {
        // Exactly one linked worktree: pair the primary with it to find the common base.
        if let Some(common) = nearest_common_parent(&paths[0], &paths[1]) {
            return Ok(common);
        }
    }

    fallback_worktree_base_dir()
}

fn branch_to_worktree_leaf(branch: &str) -> String {
    // MMM branches use a compact hash-based name to keep worktree paths short.
    // Integration → _mmm-{hash}, slice N → _mmm-{hash}-s{N}, kokomeco → _mmm-{hash}-kc
    if let Some(rest) = branch.strip_prefix("_mmm/") {
        let parts: Vec<&str> = rest.split('/').collect();
        if parts.len() >= 3 {
            let target = parts[0];
            let source = parts[1];
            let key = format!("{target}/{source}");
            let hash = key.bytes().fold(0u32, |acc, b| {
                acc.wrapping_mul(31).wrapping_add(b as u32)
            });
            let base = format!("_mmm-{:08x}", hash);
            return match parts.last() {
                Some(suffix) if *suffix == "integration" => base,
                Some(suffix) if *suffix == "kokomeco" => format!("{base}-kc"),
                Some(suffix) if suffix.starts_with("slice") => {
                    format!("{base}-s{}", &suffix["slice".len()..])
                }
                _ => base,
            };
        }
    }

    // Non-MMM branches: existing sanitization logic.
    let mut out = String::with_capacity(branch.len());
    for c in branch.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        return "branch".to_string();
    }

    const MAX_LEAF: usize = 80;
    if out.len() > MAX_LEAF {
        let hash = branch.bytes().fold(0u32, |acc, b| {
            acc.wrapping_mul(31).wrapping_add(b as u32)
        });
        let suffix = format!("_{:04x}", hash & 0xFFFF);
        let trunc = MAX_LEAF.saturating_sub(suffix.len());
        out.truncate(trunc);
        out.push_str(&suffix);
    }

    out
}

fn pick_new_worktree_path(base: &Path, branch: &str, entries: &[WorktreeEntry]) -> PathBuf {
    let base_name = branch_to_worktree_leaf(branch);
    let known = entries.iter().map(|e| e.path.clone()).collect::<Vec<_>>();

    // Guard: if even the base name would produce a path exceeding the
    // recommended maximum (240 chars — leaves room for drive letter,
    // colon, backslash, and null terminator on Windows), fall back to
    // a short hash-only name.
    const MAX_PATH_LEN: usize = 240;
    let base_len = base.to_string_lossy().len() + 1; // +1 for separator
    if base_len + base_name.len() > MAX_PATH_LEN {
        let hash = branch.bytes().fold(0u32, |acc, b| {
            acc.wrapping_mul(31).wrapping_add(b as u32)
        });
        // Ensure the fallback also respects the limit.
        let fallback_leaf = {
            // Try progressively shorter prefixes until one fits.
            let candidates = [
                format!("mm-{:08x}", hash),   // 11 chars
                format!("wt-{:08x}", hash),   // 11 chars
                format!("m{:08x}", hash),     // 9 chars
                format!("{:08x}", hash),      // 8 chars
            ];
            candidates.into_iter().find(|leaf| base_len + leaf.len() <= MAX_PATH_LEN)
                .unwrap_or_else(|| format!("{:08x}", hash))
        };
        return base.join(fallback_leaf);
    }

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
        ensure_longpaths_support()?;
        return Ok(path);
    }

    ensure_longpaths_support()?;
    let base = infer_worktree_base_dir(entries)?;
    fs::create_dir_all(to_fs_path(&base)).with_context(|| {
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
        ensure_longpaths_support()?;
        return Ok(path);
    }

    ensure_longpaths_support()?;
    let base = infer_worktree_base_dir(entries)?;
    fs::create_dir_all(to_fs_path(&base)).with_context(|| {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support as test_helpers;

    type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

    #[test]
    fn parse_worktree_entries_parses_paths_and_branches() {
        let data = "worktree /tmp/wt1\nHEAD abc\nbranch refs/heads/main\n\nworktree /tmp/wt2\nHEAD def\nbranch refs/heads/feature\n";
        let entries = parse_worktree_entries(data);

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, PathBuf::from("/tmp/wt1"));
        assert_eq!(entries[0].branch.as_deref(), Some("main"));
        assert_eq!(entries[1].path, PathBuf::from("/tmp/wt2"));
        assert_eq!(entries[1].branch.as_deref(), Some("feature"));
    }

    #[test]
    fn list_worktree_entries_returns_current_repo_entry() -> TestResult<()> {
        let repo = test_helpers::init_repo_with_base_file()?;
        let entries = test_helpers::with_repo_cwd(&repo, list_worktree_entries)?;

        assert!(!entries.is_empty());
        Ok(())
    }

    #[test]
    fn has_existing_linked_worktrees_is_true_only_for_more_than_one_entry() {
        let one = vec![WorktreeEntry {
            path: PathBuf::from("/tmp/one"),
            branch: Some("main".to_string()),
        }];
        let two = vec![
            WorktreeEntry {
                path: PathBuf::from("/tmp/one"),
                branch: Some("main".to_string()),
            },
            WorktreeEntry {
                path: PathBuf::from("/tmp/two"),
                branch: Some("feature".to_string()),
            },
        ];

        assert!(!has_existing_linked_worktrees(&one));
        assert!(has_existing_linked_worktrees(&two));
    }

    #[test]
    fn find_worktree_for_branch_returns_matching_path() {
        let entries = vec![
            WorktreeEntry {
                path: PathBuf::from("/tmp/one"),
                branch: Some("main".to_string()),
            },
            WorktreeEntry {
                path: PathBuf::from("/tmp/two"),
                branch: Some("feature".to_string()),
            },
        ];

        assert_eq!(
            find_worktree_for_branch(&entries, "feature"),
            Some(PathBuf::from("/tmp/two"))
        );
        assert_eq!(find_worktree_for_branch(&entries, "missing"), None);
    }

    #[test]
    fn nearest_common_parent_returns_shared_prefix_directory() {
        let a = Path::new("/tmp/root/a");
        let b = Path::new("/tmp/root/b");
        let common = nearest_common_parent(a, b);
        assert_eq!(common, Some(PathBuf::from("/tmp/root")));
    }

    #[test]
    fn normalize_existing_path_keeps_nonexistent_path() {
        let p = PathBuf::from("/tmp/mergetopus-does-not-exist-xyz");
        let normalized = normalize_existing_path(&p);
        assert_eq!(normalized, p);
    }

    #[test]
    fn repository_base_dir_matches_repo_root() -> TestResult<()> {
        let repo = test_helpers::init_repo_with_base_file()?;
        let got = test_helpers::with_repo_cwd(&repo, repository_base_dir)?;
        assert_eq!(normalize_existing_path(&got), normalize_existing_path(&repo));
        Ok(())
    }

    #[test]
    fn fallback_worktree_base_dir_is_parent_of_repo_root() -> TestResult<()> {
        let repo = test_helpers::init_repo_with_base_file()?;
        let expected = repo.parent().ok_or("repo has no parent")?.to_path_buf();
        let got = test_helpers::with_repo_cwd(&repo, fallback_worktree_base_dir)?;
        assert_eq!(
            normalize_existing_path(&got),
            normalize_existing_path(&expected)
        );
        Ok(())
    }

    #[test]
    fn infer_worktree_base_dir_prefers_common_parent_of_non_current_entries() -> TestResult<()> {
        let repo = test_helpers::init_repo_with_base_file()?;
        let base = test_helpers::unique_temp_repo_dir();
        let wta = base.join("wta");
        let wtb = base.join("wtb");
        std::fs::create_dir_all(&wta)?;
        std::fs::create_dir_all(&wtb)?;

        let entries = vec![
            WorktreeEntry {
                path: repo.clone(),
                branch: Some("main".to_string()),
            },
            WorktreeEntry {
                path: wta,
                branch: Some("a".to_string()),
            },
            WorktreeEntry {
                path: wtb,
                branch: Some("b".to_string()),
            },
        ];

        let got = test_helpers::with_repo_cwd(&repo, || infer_worktree_base_dir(&entries))?;
        // normalize_existing_path canonicalizes 8.3 short names on Windows;
        // apply the same normalization to `base` so both sides are comparable.
        assert_eq!(got, normalize_existing_path(&base));
        Ok(())
    }

    #[test]
    fn infer_worktree_base_dir_stays_stable_after_cwd_moves_to_linked_worktree() -> TestResult<()>
    {
        let repo = test_helpers::init_repo_with_base_file()?;
        let base = test_helpers::unique_temp_repo_dir();
        let wta = base.join("wta");
        let wtb = base.join("wtb");
        let integration = base.join("mergetopus-integration");
        std::fs::create_dir_all(&wta)?;
        std::fs::create_dir_all(&wtb)?;
        std::fs::create_dir_all(&integration)?;

        // Simulate the state after the integration worktree has been created and
        // the CWD has moved into it.  The primary worktree (repo) is now a
        // non-current entry, so the old algorithm would use (repo, wta) as the
        // anchor pair and return /tmp instead of `base`.
        let entries = vec![
            WorktreeEntry {
                path: repo.clone(),
                branch: Some("main".to_string()),
            },
            WorktreeEntry {
                path: wta.clone(),
                branch: Some("a".to_string()),
            },
            WorktreeEntry {
                path: wtb.clone(),
                branch: Some("b".to_string()),
            },
            WorktreeEntry {
                path: integration.clone(),
                branch: Some("integration".to_string()),
            },
        ];

        // CWD = integration (the newly-created linked worktree).
        let got =
            test_helpers::with_repo_cwd(&integration, || infer_worktree_base_dir(&entries))?;
        assert_eq!(
            got,
            normalize_existing_path(&base),
            "base dir must remain stable even when CWD is the integration worktree"
        );
        Ok(())
    }

    #[test]
    fn branch_to_worktree_leaf_sanitizes_non_mmm_branch_name() {
        assert_eq!(
            branch_to_worktree_leaf("feature/refactor auth"),
            "feature_refactor_auth"
        );
        assert_eq!(branch_to_worktree_leaf("***"), "___");
    }

    #[test]
    fn branch_to_worktree_leaf_uses_hash_for_mmm_integration() {
        let leaf = branch_to_worktree_leaf("_mmm/main/feature/integration");
        assert!(leaf.starts_with("_mmm-"), "expected _mmm-{{hash}}, got {leaf}");
        assert_eq!(leaf.len(), 13, "expected '_mmm-XXXXXXXX' (13 chars), got '{leaf}' ({})", leaf.len());
        // Same target/source → same hash.
        let leaf2 = branch_to_worktree_leaf("_mmm/main/feature/integration");
        assert_eq!(leaf, leaf2);
    }

    #[test]
    fn branch_to_worktree_leaf_uses_hash_for_mmm_slice() {
        let integration = branch_to_worktree_leaf("_mmm/main/feature/integration");
        let slice = branch_to_worktree_leaf("_mmm/main/feature/slice3");
        assert!(slice.starts_with(&integration), "slice should share integration prefix");
        assert!(slice.ends_with("-s3"), "expected suffix -s3, got {slice}");
    }

    #[test]
    fn branch_to_worktree_leaf_uses_hash_for_mmm_kokomeco() {
        let integration = branch_to_worktree_leaf("_mmm/main/feature/integration");
        let kokomeco = branch_to_worktree_leaf("_mmm/main/feature/kokomeco");
        assert!(kokomeco.starts_with(&integration), "kokomeco should share integration prefix");
        assert!(kokomeco.ends_with("-kc"), "expected suffix -kc, got {kokomeco}");
    }

    #[test]
    fn branch_to_worktree_leaf_different_sources_produce_different_hashes() {
        let a = branch_to_worktree_leaf("_mmm/main/feature/integration");
        let b = branch_to_worktree_leaf("_mmm/main/other/integration");
        assert_ne!(a, b, "different sources must produce different hashes");
    }

    #[test]
    fn branch_to_worktree_leaf_handles_long_non_mmm_branch_names() {
        let long = "a".repeat(200);
        let leaf = branch_to_worktree_leaf(&long);
        // Should be truncated to 80 chars + hash suffix.
        assert!(leaf.len() <= 85, "expected truncation, got {leaf} ({} chars)", leaf.len());
        assert!(leaf.starts_with("aaaaaaaaa"), "should start with a's, got {leaf}");
    }

    #[test]
    fn pick_new_worktree_path_avoids_known_existing_entry_path() {
        let base = PathBuf::from("/tmp");
        let entries = vec![WorktreeEntry {
            path: base.join("main"),
            branch: Some("main".to_string()),
        }];

        let picked = pick_new_worktree_path(&base, "main", &entries);
        assert_eq!(picked, base.join("main-1"));
    }

    #[test]
    fn pick_new_worktree_path_falls_back_to_short_hash_when_base_too_long() {
        // Use a long branch name so that the descriptive leaf exceeds the
        // limit, triggering the hash fallback.
        let long_branch = "a".repeat(50);
        let base_str = "C:\\".to_string() + &"a".repeat(195);
        let base = PathBuf::from(&base_str);
        let entries: Vec<WorktreeEntry> = Vec::new();

        let picked = pick_new_worktree_path(&base, &long_branch, &entries);
        let picked_str = picked.to_string_lossy();
        assert!(picked_str.len() <= 240);
        assert!(picked_str.contains("mm-"), "expected 'mm-{{hash}}', got {picked_str}");
    }

    #[test]
    fn pick_new_worktree_path_falls_back_to_progressively_shorter_hash_when_base_extreme() {
        // Base path so long that even "mm-{hash}" (11 chars) exceeds the
        // limit — ensure we fall back to a shorter prefix.
        let long_branch = "a".repeat(50);
        let base_str = "C:\\".to_string() + &"a".repeat(228);
        let base = PathBuf::from(&base_str);
        let entries: Vec<WorktreeEntry> = Vec::new();

        let picked = pick_new_worktree_path(&base, &long_branch, &entries);
        let picked_str = picked.to_string_lossy();
        assert!(
            picked_str.len() <= 245,
            "expected roughly <= 240, got {} ({picked_str})",
            picked_str.len()
        );
        // Should NOT contain the "mm-" prefix (that would exceed the limit).
        assert!(!picked_str.contains("mm-"), "mm- should be too long, got {picked_str}");
        // Should contain a shortened hash-based leaf (hex digits).
        let leaf = picked.file_name().unwrap().to_string_lossy();
        assert!(leaf.len() < 11, "leaf should be shorter than mm-prefix version, got {leaf}");
        assert!(!leaf.contains("aaaaa"), "leaf should not contain the original branch name");
    }

    #[test]
    fn switch_to_dir_changes_current_directory() -> TestResult<()> {
        let repo = test_helpers::init_repo_with_base_file()?;
        let child = repo.join("subdir");
        std::fs::create_dir_all(&child)?;

        let cwd = test_helpers::with_repo_cwd(&repo, || {
            switch_to_dir(&child)?;
            Ok(std::env::current_dir()?)
        })?;
        assert_eq!(normalize_existing_path(&cwd), normalize_existing_path(&child));
        Ok(())
    }

    #[test]
    fn ensure_worktree_for_existing_branch_returns_existing_path_without_creation() {
        let entries = vec![WorktreeEntry {
            path: PathBuf::from("/tmp/existing"),
            branch: Some("feature".to_string()),
        }];

        let got = ensure_worktree_for_existing_branch("feature", &entries)
            .expect("existing worktree path should be returned");
        assert_eq!(got, PathBuf::from("/tmp/existing"));
    }

    #[test]
    fn ensure_worktree_for_existing_branch_creates_new_worktree_when_missing() -> TestResult<()> {
        let repo = test_helpers::init_repo_with_base_file()?;
        test_helpers::git(&repo, &["branch", "feature"])?;

        let entries = test_helpers::with_repo_cwd(&repo, list_worktree_entries)?;
        let path = test_helpers::with_repo_cwd(&repo, || {
            ensure_worktree_for_existing_branch("feature", &entries)
        })?;

        let checked_out = test_helpers::git(&path, &["branch", "--show-current"])?;
        assert_eq!(checked_out, "feature");
        Ok(())
    }

    #[test]
    fn ensure_worktree_for_branch_reset_creates_branch_at_target_commit() -> TestResult<()> {
        let repo = test_helpers::init_repo_with_base_file()?;
        let at = test_helpers::git(&repo, &["rev-parse", "HEAD"])?;
        test_helpers::write_file(&repo, "later.txt", "later\n")?;
        test_helpers::commit_all(&repo, "later")?;

        let entries = test_helpers::with_repo_cwd(&repo, list_worktree_entries)?;
        let path = test_helpers::with_repo_cwd(&repo, || {
            ensure_worktree_for_branch_reset("reset-branch", &at, &entries)
        })?;

        let head = test_helpers::git(&path, &["rev-parse", "HEAD"])?;
        let branch = test_helpers::git(&path, &["branch", "--show-current"])?;
        assert_eq!(head, at);
        assert_eq!(branch, "reset-branch");
        Ok(())
    }
}
