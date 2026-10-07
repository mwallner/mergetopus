use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::process::Command;

use crate::models::PathProvenance;

mod branch;
mod checkout;
mod commit;
mod diff;
mod merge;
mod refs;
mod worktree;

pub use branch::*;
pub use checkout::*;
pub use commit::*;
pub use diff::*;
pub use merge::*;
pub use refs::*;

/// Run a git command inside `repo_path`, ignoring the process working
/// directory. These `_in` variants are what library consumers (biggit) use;
/// the CWD-based functions below are thin wrappers for the CLI's own
/// behaviour.
pub fn run_git_in(repo_path: &std::path::Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_path)
        .output()
        .with_context(|| format!("failed to execute git {}", args.join(" ")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git {} failed: {}", args.join(" "), stderr.trim());
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn run_git_allow_failure_in(
    repo_path: &std::path::Path,
    args: &[&str],
) -> Result<(bool, String, String)> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_path)
        .output()
        .with_context(|| format!("failed to execute git {}", args.join(" ")))?;

    let ok = output.status.success();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Ok((ok, stdout, stderr))
}

pub fn run_git(args: &[&str]) -> Result<String> {
    run_git_in(&std::env::current_dir()?, args)
}

pub fn run_git_allow_failure(args: &[&str]) -> Result<(bool, String, String)> {
    run_git_allow_failure_in(&std::env::current_dir()?, args)
}

pub fn ensure_git_context() -> Result<()> {
    ensure_git_worktree()?;

    let status = run_git(&["status", "--porcelain"])?;
    if !status.is_empty() {
        bail!("working tree is not clean; commit or stash changes before running mergetopus");
    }

    Ok(())
}

pub fn ensure_git_worktree() -> Result<()> {
    let inside = run_git(&["rev-parse", "--is-inside-work-tree"])?;
    if inside != "true" {
        bail!("current directory is not inside a Git working tree");
    }

    ensure_longpaths_support()?;

    Ok(())
}

#[cfg(target_os = "windows")]
pub fn ensure_longpaths_support() -> Result<()> {
    let current = get_git_config("core.longpaths")?.unwrap_or_default();
    if current.eq_ignore_ascii_case("true") {
        return Ok(());
    }

    let _ = run_git(&["config", "core.longpaths", "true"]);

    // Best-effort: warn but don't bail if the setting couldn't be verified.
    let verified = get_git_config("core.longpaths")?.unwrap_or_default();
    if !verified.eq_ignore_ascii_case("true") {
        eprintln!(
            "warning: failed to enable core.longpaths; git reports '{verified}'. \
             Long paths on Windows may fail."
        );
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn ensure_longpaths_support() -> Result<()> {
    Ok(())
}

pub fn restore_ours(path: &str) -> Result<()> {
    if path_exists_in_ref("HEAD", path)? {
        return run_git(&[
            "restore",
            "--source=HEAD",
            "--staged",
            "--worktree",
            "--",
            path,
        ])
        .map(|_| ());
    }

    // Ours side has no content at this path (deleted by us, renamed away,
    // added by them, or a dir-rename "file location" suggestion at a path
    // HEAD does not contain). `git restore --source=HEAD` refuses such
    // unmerged paths, so "ours" is expressed as removal from the index plus
    // the worktree copy the merge may have left behind.
    resolve_path_as_deleted(path)
}

/// Settle a conflicted (or vanished) path as a deletion: drop the index
/// entry — forcing it past git's conflict checks if needed — and remove the
/// worktree copy. Absence is the expected end state.
pub fn resolve_path_as_deleted(path: &str) -> Result<()> {
    let (ok, _, stderr) = run_git_allow_failure(&["rm", "--cached", "--", path])?;
    if !ok && !stderr.contains("did not match any files") {
        run_git(&["rm", "-f", "--cached", "--", path])?;
    }

    let fs_path = crate::win32_path::to_fs_path(path);
    // Missing worktree file is the expected state for "absent in ours".
    let _ = std::fs::remove_file(&fs_path);
    Ok(())
}

/// Parse unmerged index entries (`git ls-files -u`) into
/// `path -> (stage -> blob oid)`. Stage 1 = base, 2 = ours, 3 = theirs.
pub fn conflict_stage_map()
-> Result<std::collections::BTreeMap<String, std::collections::BTreeMap<usize, String>>> {
    let out = run_git(&["ls-files", "-u", "-z"])?;
    let mut map = std::collections::BTreeMap::new();
    for record in out.split('\0') {
        let record = record.trim();
        if record.is_empty() {
            continue;
        }
        let Some((meta, path)) = record.split_once('\t') else {
            continue;
        };
        let mut meta_parts = meta.split_whitespace();
        let (Some(_mode), Some(oid), Some(stage)) =
            (meta_parts.next(), meta_parts.next(), meta_parts.next())
        else {
            continue;
        };
        let Ok(stage) = stage.parse::<usize>() else {
            continue;
        };
        if !(1..=3).contains(&stage) {
            continue;
        }
        map.entry(path.to_string())
            .or_insert_with(std::collections::BTreeMap::new)
            .insert(stage, oid.to_string());
    }
    Ok(map)
}

/// Write the blob identified by `oid` to `dest`, creating parent directories
/// and staging the result. Used to materialize stage blobs that no longer
/// exist in any branch tree (e.g. file-location conflict content).
pub fn write_oid_to_staged_path(oid: &str, path: &str) -> Result<()> {
    let (ok, stdout, stderr) = run_git_allow_failure(&["cat-file", "blob", oid])?;
    if !ok {
        bail!("git cat-file blob {oid} failed: {stderr}");
    }
    let fs_path = crate::win32_path::to_fs_path(path);
    if let Some(parent) = std::path::Path::new(&fs_path).parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create parent directory for '{path}'"))?;
    }
    std::fs::write(&fs_path, stdout.as_bytes())
        .with_context(|| format!("failed to write '{path}' from blob {oid}"))?;
    stage_path(path)
}

pub fn list_slice_branches_for_integration(integration_branch: &str) -> Result<Vec<String>> {
    list_slice_branches_for_integration_in(&std::env::current_dir()?, integration_branch)
}

/// Path-taking variant: enumerate the slice branches of `integration_branch`
/// inside `repo_path` regardless of the ambient working directory.
pub fn list_slice_branches_for_integration_in(
    repo_path: &std::path::Path,
    integration_branch: &str,
) -> Result<Vec<String>> {
    let out = run_git_in(
        repo_path,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads",
            "refs/remotes",
        ],
    )?;
    let Some(base) = integration_branch.strip_suffix("/integration") else {
        return Ok(Vec::new());
    };
    let prefix = format!("{base}/slice");
    let mut slices = out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && *l != "origin/HEAD")
        .filter_map(|l| {
            if l.starts_with(&prefix) {
                Some(l.to_string())
            } else if let Some(local) = local_branch_name_from_remote_ref(l) {
                if local.starts_with(&prefix) {
                    Some(local)
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    slices.sort();
    slices.dedup();
    Ok(slices)
}

pub fn is_ancestor(older: &str, newer: &str) -> Result<bool> {
    let (ok, _, _) = run_git_allow_failure(&["merge-base", "--is-ancestor", older, newer])?;
    Ok(ok)
}

pub fn slice_merge_status(
    integration_branch: &str,
    slice_branches: &[String],
) -> Result<BTreeMap<String, bool>> {
    let mut result = BTreeMap::new();
    for slice in slice_branches {
        let probe_ref = best_ref_for_local_branch(slice)?.unwrap_or_else(|| slice.clone());
        result.insert(slice.clone(), is_ancestor(&probe_ref, integration_branch)?);
    }
    Ok(result)
}

pub fn path_exists_in_ref(reference: &str, path: &str) -> Result<bool> {
    let (ok, _, _) = run_git_allow_failure(&["cat-file", "-e", &format!("{reference}:{path}")])?;
    Ok(ok)
}

pub fn restore_from_ref(reference: &str, path: &str) -> Result<()> {
    run_git(&[
        "restore",
        &format!("--source={reference}"),
        "--staged",
        "--worktree",
        "--",
        path,
    ])
    .map(|_| ())
}

pub fn rm_path(path: &str) -> Result<()> {
    run_git(&["rm", "--ignore-unmatch", "--", path]).map(|_| ())
}

pub fn path_provenance(source_ref: &str, source_sha: &str, path: &str) -> Result<PathProvenance> {
    let format = "%H%x1f%an%x1f%ae%x1f%aI";
    let (ok, out, _) = run_git_allow_failure(&[
        "log",
        "-n",
        "1",
        &format!("--format={format}"),
        source_sha,
        "--",
        path,
    ])?;

    let mut path_commit = None;
    let mut author_name = None;
    let mut author_email = None;
    let mut author_date = None;

    if ok && !out.trim().is_empty() {
        let parts = out.split('\u{1f}').collect::<Vec<_>>();
        if parts.len() >= 4 {
            path_commit = Some(parts[0].to_string());
            author_name = Some(parts[1].to_string());
            author_email = Some(parts[2].to_string());
            author_date = Some(parts[3].to_string());
        }
    }

    Ok(PathProvenance {
        source_ref: source_ref.to_string(),
        source_commit: source_sha.to_string(),
        path: path.to_string(),
        path_commit,
        author_name,
        author_email,
        author_date,
    })
}

pub fn commit_slice(message: &str, provenance: &PathProvenance) -> Result<()> {
    let mut command = Command::new("git");
    command.args(["commit", "-F", "-"]);

    if let Some(name) = &provenance.author_name {
        command.env("GIT_AUTHOR_NAME", name);
    }
    if let Some(email) = &provenance.author_email {
        command.env("GIT_AUTHOR_EMAIL", email);
    }
    if let Some(date) = &provenance.author_date {
        command.env("GIT_AUTHOR_DATE", date);
    }

    let mut child = command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("failed to spawn git commit for slice")?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(message.as_bytes())
            .context("failed to write slice commit message to stdin")?;
    }

    let output = child
        .wait_with_output()
        .context("failed to wait for git commit for slice")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("slice commit failed: {}", stderr.trim());
    }

    Ok(())
}

pub fn show_file_at(reference: &str, path: &str) -> Result<String> {
    let output = Command::new("git")
        .args(["show", &format!("{reference}:{path}")])
        .output()
        .with_context(|| format!("failed to execute git show {reference}:{path}"))?;

    if output.status.success() {
        let mut s = String::from_utf8_lossy(&output.stdout).into_owned();
        // git show appends a trailing newline (CRLF on Windows); strip only
        // that, not all trailing whitespace, to preserve meaningful content.
        if s.ends_with('\n') {
            s.truncate(s.len() - 1);
            if s.ends_with('\r') {
                s.truncate(s.len() - 1);
            }
        }
        Ok(s)
    } else {
        let err = String::from_utf8_lossy(&output.stderr);
        Ok(format!("<unavailable: {err}>"))
    }
}

pub fn consolidated_branch_name(integration_branch: &str) -> String {
    if let Some(base) = integration_branch.strip_suffix("/integration") {
        format!("{base}/kokomeco")
    } else {
        format!("{integration_branch}/kokomeco")
    }
}

pub fn three_way_diff(path: &str, source_ref: &str) -> Result<String> {
    let base = merge_base("HEAD", source_ref)?;
    let ours = show_file_at("HEAD", path)?;
    let base_txt = show_file_at(&base, path)?;
    let theirs = show_file_at(source_ref, path)?;

    Ok(format!(
        "=== OURS (HEAD) ===\n{ours}\n\n=== BASE ({base}) ===\n{base_txt}\n\n=== THEIRS ({source_ref}) ===\n{theirs}"
    ))
}

pub fn launch_difftool(path: &str, source_ref: &str) -> Result<()> {
    run_git(&["difftool", "--no-prompt", "HEAD", source_ref, "--", path]).with_context(|| {
        format!("failed to launch git difftool for '{path}' against '{source_ref}'")
    })?;
    Ok(())
}

/// Read a single git config value; returns `None` when the key is unset or
/// the value is empty (an empty tool name or command is unusable).
pub fn get_git_config(key: &str) -> Result<Option<String>> {
    let (ok, out, _) = run_git_allow_failure(&["config", "--get", key])?;
    if ok && !out.is_empty() {
        Ok(Some(out))
    } else {
        Ok(None)
    }
}

fn is_slice_branch_ref(reference: &str) -> bool {
    let parts = reference.split('/').collect::<Vec<_>>();
    let Some(idx) = parts.iter().position(|p| *p == "_mmm") else {
        return false;
    };

    parts.len().saturating_sub(idx) == 4
        && !parts[idx + 1].is_empty()
        && !parts[idx + 2].is_empty()
        && parts[idx + 3].starts_with("slice")
        && parts[idx + 3].len() > "slice".len()
        && parts[idx + 3]["slice".len()..]
            .chars()
            .all(|c| c.is_ascii_digit())
}

/// Return the first Mergetopus partial-merge commit on the integration branch
/// first-parent history.
pub fn first_mergetopus_partial_merge_commit(integration_branch: &str) -> Result<String> {
    let out = run_git(&[
        "log",
        integration_branch,
        "--first-parent",
        "--reverse",
        "--format=%H%x1f%P%x1f%s",
    ])?;

    for line in out.lines() {
        let mut parts = line.splitn(3, '\u{1f}');
        let sha = parts.next().unwrap_or("").trim();
        let parent_list = parts.next().unwrap_or("").trim();
        let subject = parts.next().unwrap_or("").trim();

        // First, find the first merge commit on the integration branch that
        // does not come from a slice branch.
        let parents = parent_list
            .split_whitespace()
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>();
        if parents.len() < 2 {
            continue;
        }

        let merged_parent = parents[1];
        let merged_parent_refs = refs_pointing_to(merged_parent)?;
        let comes_from_slice = merged_parent_refs.iter().any(|r| is_slice_branch_ref(r));
        if comes_from_slice {
            continue;
        }

        if !subject.starts_with("Mergetopus: partial merge '") {
            crate::color::print_error(
                &format!(
                    "warning: skipping merge commit '{}' on '{}' because subject does not match expected Mergetopus prefix",
                    sha, integration_branch
                ),
                None,
            );
            continue;
        }

        return Ok(sha.to_string());
    }

    bail!(
        "failed to locate initial mergetopus partial-merge commit on integration branch '{}'",
        integration_branch
    )
}

/// Return the SHA of the first parent of `rev` (i.e. `rev^`).
pub fn parent_sha(rev: &str) -> Result<String> {
    run_git(&["rev-parse", "--verify", &format!("{rev}^")])
        .with_context(|| format!("failed to resolve parent of '{rev}'"))
}

/// List every local branch that looks like a mergetopus slice branch
/// (`_mmm/<original>/<source>/slice<N>` where N is one or more digits).
pub fn list_all_slice_branches() -> Result<Vec<String>> {
    let out = run_git(&[
        "for-each-ref",
        "--format=%(refname:short)",
        "refs/heads",
        "refs/remotes",
    ])?;
    let mut slices = out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && *l != "origin/HEAD")
        .filter_map(|l| {
            if is_local_slice_branch_name(l) {
                return Some(l.to_string());
            }

            local_branch_name_from_remote_ref(l)
                .filter(|candidate| is_local_slice_branch_name(candidate))
        })
        .collect::<Vec<_>>();
    slices.sort();
    slices.dedup();
    Ok(slices)
}

fn is_local_slice_branch_name(branch: &str) -> bool {
    let parts = branch.split('/').collect::<Vec<_>>();
    parts.len() == 4
        && parts[0] == "_mmm"
        && !parts[1].is_empty()
        && !parts[2].is_empty()
        && parts[3].starts_with("slice")
        && parts[3]["slice".len()..]
            .chars()
            .all(|c| c.is_ascii_digit())
        && parts[3].len() > "slice".len()
}

/// Write the content of `reference:path` from the object store to the file at
/// `dest`.  If the path does not exist at that ref (e.g. new file / deleted
/// file), an empty file is written instead.
/// Write the content of `reference:path` from the object store to the file at
/// `dest`. Returns whether the path exists at that ref: callers use this to
/// distinguish "side was deleted/renamed away" from "side is a zero-byte file"
/// when preparing LOCAL/BASE/REMOTE inputs for the merge tool.
pub fn write_blob_to_path(reference: &str, path: &str, dest: &str) -> Result<bool> {
    let output = Command::new("git")
        .args(["show", &format!("{reference}:{path}")])
        .output()
        .with_context(|| format!("failed to execute git show {reference}:{path}"))?;

    let present = output.status.success();
    let content: &[u8] = if present { &output.stdout } else { b"" };

    fs::write(dest, content).with_context(|| format!("failed to write '{dest}'"))?;
    Ok(present)
}

/// Sentinel name used for a LOCAL/BASE/REMOTE input whose side lacks the path
/// (deleted or renamed away). Tools reading the file see zero bytes like
/// before, but the name — and the per-side info line printed by resolve —
/// no longer confuses "absent" with "genuinely empty".
pub fn absent_sentinel_path(dest: &str) -> String {
    format!("{dest}.DELETED")
}

/// Materialize the input file for a side that is absent at the given ref: an
/// empty sentinel whose name signals deletion to the merge tool (and to the
/// user). The sentinel path, not the original dest, must be used everywhere
/// the tool input is passed or re-read, so tools that write their output to
/// `$BASE` keep working for absent bases too.
pub fn prepare_absent_side_file(dest: &str) -> Result<String> {
    let sentinel = absent_sentinel_path(dest);
    fs::write(&sentinel, b"").with_context(|| format!("failed to write sentinel '{sentinel}'"))?;
    Ok(sentinel)
}

/// Stage a single path in the index (`git add -- <path>`).
pub fn stage_path(path: &str) -> Result<()> {
    run_git(&["add", "--", path]).map(|_| ())
}

pub fn select_conflicts_by_list(all_conflicts: &[String], csv: &str) -> Result<Vec<String>> {
    let set = all_conflicts.iter().cloned().collect::<BTreeSet<_>>();
    let requested = csv
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();

    let mut selected = Vec::new();
    for item in requested {
        if !set.contains(&item) {
            bail!("path '{item}' is not in conflicted file list");
        }
        selected.push(item);
    }

    selected.sort();
    selected.dedup();
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::{
        is_slice_branch_ref, list_all_slice_branches, list_slice_branches_for_integration,
        prepare_absent_side_file, restore_ours, write_blob_to_path,
    };
    use crate::test_support as test_helpers;

    type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

    #[test]
    fn slice_ref_detection_accepts_local_and_remote() {
        assert!(is_slice_branch_ref("_mmm/main/feature/slice1"));
        assert!(is_slice_branch_ref("origin/_mmm/main/feature/slice23"));
    }

    #[test]
    fn slice_ref_detection_rejects_non_slice_refs() {
        assert!(!is_slice_branch_ref("_mmm/main/feature/integration"));
        assert!(!is_slice_branch_ref("feature"));
    }

    #[test]
    fn remote_only_slices_are_listed_by_local_name() -> TestResult<()> {
        let repo = test_helpers::setup_remote_with_feature()?;
        let integration = "_mmm/main/feature/integration";
        let slice = "_mmm/main/feature/slice1";

        test_helpers::git(&repo, &["checkout", "-b", integration])?;
        test_helpers::git(&repo, &["checkout", "-b", slice])?;
        test_helpers::write_file(&repo, "slice.txt", "slice\n")?;
        test_helpers::commit_all(&repo, "slice commit")?;
        test_helpers::git(&repo, &["push", "-u", "origin", integration])?;
        test_helpers::git(&repo, &["push", "-u", "origin", slice])?;
        test_helpers::git(&repo, &["checkout", "main"])?;
        test_helpers::git(&repo, &["branch", "-D", integration])?;
        test_helpers::git(&repo, &["branch", "-D", slice])?;

        let all = test_helpers::with_repo_cwd(&repo, list_all_slice_branches)?;
        assert!(all.iter().any(|b| b == slice));

        let for_integration = test_helpers::with_repo_cwd(&repo, || {
            list_slice_branches_for_integration(integration)
        })?;
        assert!(for_integration.iter().any(|b| b == slice));
        Ok(())
    }

    /// A path deleted on one side must NOT be handed to the merge tool as a
    /// plain empty file: `write_blob_to_path` reports presence, and resolve
    /// swaps the input for a `.DELETED` sentinel, keeping "absent" and
    /// "zero-byte content" distinguishable.
    #[test]
    fn deleted_and_empty_paths_are_distinguishable() -> TestResult<()> {
        let repo = test_helpers::init_repo_with_base_file()?;
        test_helpers::write_file(&repo, "empty.txt", "")?;
        test_helpers::commit_all(&repo, "add empty.txt")?;

        test_helpers::git(&repo, &["checkout", "-b", "deleted-side"])?;
        test_helpers::git(&repo, &["rm", "-q", "base.txt"])?;
        test_helpers::git(&repo, &["commit", "-m", "delete base.txt"])?;

        let out_dir = test_helpers::unique_temp_repo_dir();
        std::fs::create_dir_all(&out_dir)?;
        let from_deleted = out_dir.join("blob-of-deleted");
        let from_empty = out_dir.join("blob-of-empty");

        let (deleted_present, empty_present) = test_helpers::with_repo_cwd(&repo, || {
            let deleted_present =
                write_blob_to_path("deleted-side", "base.txt", &from_deleted.to_string_lossy())?;
            let empty_present =
                write_blob_to_path("deleted-side", "empty.txt", &from_empty.to_string_lossy())?;
            Ok((deleted_present, empty_present))
        })?;

        assert!(
            !deleted_present,
            "a path missing at the ref must be reported as absent"
        );
        assert!(
            empty_present,
            "a genuine zero-byte file must be reported as present"
        );

        let sentinel = test_helpers::with_repo_cwd(&repo, || {
            prepare_absent_side_file(&from_deleted.to_string_lossy())
        })?;
        assert!(sentinel.ends_with(".DELETED"));
        assert!(std::fs::metadata(&sentinel)?.len() == 0);
        Ok(())
    }

    /// restore_ours must be total over unmerged index shapes: paths present
    /// in HEAD are restored; paths absent in HEAD (deleted by us, added by
    /// them, both-deleted, file-location suggestions) are removed from the
    /// index instead of aborting with "path is unmerged".
    #[test]
    fn restore_ours_handles_paths_absent_in_head() -> TestResult<()> {
        let repo = test_helpers::init_repo()?;
        test_helpers::write_file(&repo, "src/a.txt", "a\n")?;
        test_helpers::commit_all(&repo, "base")?;

        test_helpers::git(&repo, &["checkout", "-b", "feature"])?;
        test_helpers::git(&repo, &["mv", "src/a.txt", "renamed_feature.txt"])?;
        test_helpers::git(&repo, &["commit", "-m", "feature renames a"])?;

        test_helpers::git(&repo, &["checkout", "main"])?;
        test_helpers::git(&repo, &["mv", "src/a.txt", "renamed_main.txt"])?;
        test_helpers::git(&repo, &["commit", "-m", "main renames a"])?;

        test_helpers::git(&repo, &["merge", "--no-ff", "--no-commit", "feature"]).ok();

        test_helpers::with_repo_cwd(&repo, || {
            // DD: old path, stage 1 only, absent in HEAD.
            restore_ours("src/a.txt")?;
            // UA: their new path, absent in HEAD.
            restore_ours("renamed_feature.txt")?;
            // AU: our new path, present in HEAD.
            restore_ours("renamed_main.txt")?;
            Ok(())
        })?;

        let unmerged = test_helpers::with_repo_cwd(&repo, super::conflicted_files)?;
        assert!(
            unmerged.is_empty(),
            "restore_ours should leave no unmerged entries:\n{unmerged:?}"
        );

        // Ours-side state fully restored: our rename target present, theirs
        // (and the shared old path) gone.
        assert!(repo.join("renamed_main.txt").exists());
        assert!(!repo.join("renamed_feature.txt").exists());
        assert!(!repo.join("src/a.txt").exists());
        Ok(())
    }
}
