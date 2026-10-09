use crate::git_ops::{run_git, run_git_allow_failure};
use anyhow::Result;

/// Source-side deletions (`D`) and renames (`R...`) between the merge base and
/// the source, excluding paths that ended up conflicted. These are the merge
/// decisions git applies silently — reporting them makes an audit possible
/// (e.g. a file both sides deleted never surfaces as a conflict).
pub fn auto_applied_entries(
    merge_base: &str,
    source_sha: &str,
    conflicted: &[String],
) -> Result<Vec<(String, String)>> {
    let (ok, out, _) =
        run_git_allow_failure(&["diff", "--name-status", "-M", merge_base, source_sha])?;
    if !ok {
        return Ok(Vec::new());
    }

    let conflicted_set = conflicted
        .iter()
        .map(|p| p.as_str())
        .collect::<std::collections::BTreeSet<_>>();

    let mut entries = Vec::new();
    for line in out.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 2 {
            continue;
        }
        let (kind, detail) = if fields[0].starts_with('D') {
            ("D", fields[1].to_string())
        } else if fields[0].starts_with('R') {
            let second = *fields.get(2).unwrap_or(&fields[1]);
            ("R", format!("{} -> {second}", fields[1]))
        } else {
            continue;
        };

        if fields[1..].iter().any(|p| conflicted_set.contains(*p)) {
            continue;
        }
        entries.push((kind.to_string(), detail));
    }

    entries.sort();
    Ok(entries)
}

pub fn conflicted_files() -> Result<Vec<String>> {
    let out = run_git(&["diff", "--name-only", "--diff-filter=U"])?;
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

pub fn staged_files() -> Result<Vec<String>> {
    let out = run_git(&["diff", "--cached", "--name-only"])?;
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

pub fn unstaged_files() -> Result<Vec<String>> {
    let out = run_git(&["diff", "--name-only"])?;
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

pub fn staged_has_changes() -> Result<bool> {
    let (ok, _, _) = run_git_allow_failure(&["diff", "--cached", "--quiet"])?;
    Ok(!ok)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support as test_helpers;

    type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

    #[test]
    fn conflicted_files_lists_unmerged_paths() -> TestResult<()> {
        let repo = test_helpers::setup_single_conflict_repo()?;

        let merge = test_helpers::run(
            std::process::Command::new("git")
                .args(["merge", "feature"])
                .current_dir(&repo),
        )?;
        assert!(!merge.status.success(), "expected merge conflict setup");

        let files = test_helpers::with_repo_cwd(&repo, conflicted_files)?;
        assert_eq!(files, vec!["conflict.txt"]);
        Ok(())
    }

    #[test]
    fn staged_files_lists_cached_paths() -> TestResult<()> {
        let repo = test_helpers::init_repo_with_base_file()?;
        test_helpers::write_file(&repo, "staged.txt", "staged\n")?;
        test_helpers::git(&repo, &["add", "staged.txt"])?;

        let files = test_helpers::with_repo_cwd(&repo, staged_files)?;
        assert_eq!(files, vec!["staged.txt"]);
        Ok(())
    }

    #[test]
    fn unstaged_files_lists_worktree_only_paths() -> TestResult<()> {
        let repo = test_helpers::init_repo_with_base_file()?;
        test_helpers::write_file(&repo, "base.txt", "base\nmodified\n")?;

        let files = test_helpers::with_repo_cwd(&repo, unstaged_files)?;
        assert_eq!(files, vec!["base.txt"]);
        Ok(())
    }

    #[test]
    fn staged_has_changes_reports_index_state() -> TestResult<()> {
        let repo = test_helpers::init_repo_with_base_file()?;

        let initially = test_helpers::with_repo_cwd(&repo, staged_has_changes)?;
        assert!(!initially, "fresh repo should have no staged changes");

        test_helpers::write_file(&repo, "index.txt", "index\n")?;
        test_helpers::git(&repo, &["add", "index.txt"])?;

        let after_add = test_helpers::with_repo_cwd(&repo, staged_has_changes)?;
        assert!(after_add, "staged file should be detected");

        Ok(())
    }
}
