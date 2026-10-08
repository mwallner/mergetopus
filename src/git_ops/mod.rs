use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufRead, Write};
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

/// Result of a contentless merge of two commits pinned to one explicit base.
pub struct PinnedBaseMerge {
    /// OID of the resulting tree.
    pub tree: String,
    /// Paths left unmerged by that single-base merge.
    pub conflicts: BTreeSet<String>,
}

/// Run `git merge-tree --write-tree --merge-base=<base> <ours> <theirs>`:
/// a full ort merge (rename detection included) against exactly one base,
/// without touching index or worktree. Returns `None` when the git version
/// lacks the support needed (unparsable output), so callers can fall back.
pub fn merge_tree_with_base(
    ours: &str,
    theirs: &str,
    base: &str,
) -> Result<Option<PinnedBaseMerge>> {
    let (ok, stdout, _stderr) = run_git_allow_failure(&[
        "merge-tree",
        "--write-tree",
        &format!("--merge-base={base}"),
        ours,
        theirs,
    ])?;
    let _ = ok; // merge-tree exits 1 on conflicts; that is a valid answer
    let mut lines = stdout.lines();
    let Some(tree) = lines.next().map(str::trim) else {
        return Ok(None);
    };
    if tree.is_empty() || tree.len() < 40 || !tree.chars().all(|c| c.is_ascii_hexdigit()) {
        // Old git: --merge-base unknown → usage error, no tree OID printed.
        return Ok(None);
    }

    let mut conflicts = BTreeSet::new();
    for line in lines {
        // conflicted file info: "<mode> <oid> <stage>\t<path>"
        let Some((meta, path)) = line.split_once('\t') else {
            continue;
        };
        let unmerged = meta
            .split_whitespace()
            .nth(2)
            .and_then(|s| s.parse::<u8>().ok())
            .is_some_and(|stage| (1..=3).contains(&stage));
        if unmerged {
            conflicts.insert(path.to_string());
        }
    }
    Ok(Some(PinnedBaseMerge {
        tree: tree.to_string(),
        conflicts,
    }))
}

/// Stage many blobs in one index update and materialize them in one
/// checkout. Both lists stream over stdin (`update-index --add -z
/// --index-info` + `checkout-index -f -z --stdin`), so there is no
/// command-line limit at all; mode and OID are carried verbatim, making
/// this byte-identical to `write_stage_to_staged_path` per entry. Duplicate
/// paths stage the last entry.
pub fn write_stages_to_staged_paths(entries: &[(String, String, String)]) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    // last occurrence of a path wins; iteration order preserved
    let by_path: BTreeMap<&str, (&str, &str)> = entries
        .iter()
        .map(|(mode, oid, path)| (path.as_str(), (mode.as_str(), oid.as_str())))
        .collect();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut ordered: Vec<&str> = entries
        .iter()
        .rev()
        .map(|(_, _, path)| path.as_str())
        .filter(|p| seen.insert(p))
        .collect();
    ordered.reverse();

    if supports_pathspec_from_file() {
        let mut payload = Vec::new();
        for path in &ordered {
            let (mode, oid) = by_path[path];
            payload.extend_from_slice(mode.as_bytes());
            payload.push(b' ');
            payload.extend_from_slice(oid.as_bytes());
            payload.push(b'\t');
            payload.extend_from_slice(path.as_bytes());
            payload.push(0);
        }
        run_git_stdin(&["update-index", "--add", "-z", "--index-info"], &payload)?;
        return run_git_stdin(
            &["checkout-index", "-f", "-z", "--stdin"],
            &nul_lines(ordered.iter().copied()),
        );
    }

    // Old git: batched --cacheinfo argv + pathspec checkout.
    let mut chunk: Vec<&str> = Vec::new();
    let mut chunk_bytes = 0usize;
    let mut batches: Vec<Vec<&str>> = Vec::new();
    for path in &ordered {
        let est = path.len() * 2 + 80;
        if !chunk.is_empty() && chunk_bytes + est > 20_000 {
            batches.push(std::mem::take(&mut chunk));
            chunk_bytes = 0;
        }
        chunk_bytes += est;
        chunk.push(path);
    }
    if !chunk.is_empty() {
        batches.push(chunk);
    }
    for batch in batches {
        let mut args: Vec<String> = vec!["update-index".to_string(), "--add".to_string()];
        for path in &batch {
            let (mode, oid) = by_path[path];
            args.push("--cacheinfo".to_string());
            args.push(format!("{mode},{oid},{path}"));
        }
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        run_git(&arg_refs)?;
        let paths: Vec<String> = batch.iter().map(|p| (*p).to_string()).collect();
        restore_worktree_from_index(&paths)?;
    }
    Ok(())
}

/// Fetch many `<ref>:<path>` blobs into destination files using ONE
/// `git cat-file --batch` process, responding sequentially per spec so the
/// pipes never deadlock. Returns presence keyed by (reference, path);
/// absent paths leave their dest untouched (callers apply their own
/// sentinel/empty-file semantics). Requests whose path contains a newline
/// bypass the line-based protocol and use `write_blob_to_path` individually.
/// The returned map is keyed by the spec string `<reference>:<path>`.
pub fn write_blobs_batch(requests: &[(String, String, String)]) -> Result<BTreeMap<String, bool>> {
    let mut present = BTreeMap::new();
    let mut batchable: Vec<&(String, String, String)> = Vec::new();
    for req in requests {
        if req.1.contains('\n') {
            let ok_present = write_blob_to_path(&req.0, &req.1, &req.2)?;
            present.insert(format!("{}:{}", req.0, req.1), ok_present);
        } else {
            batchable.push(req);
        }
    }
    if batchable.is_empty() {
        return Ok(present);
    }

    let mut child = Command::new("git")
        .args(["cat-file", "--batch"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .context("failed to spawn git cat-file --batch")?;
    let result = (|| -> Result<()> {
        let mut stdin = child.stdin.take().context("cat-file stdin")?;
        let stdout = child.stdout.take().context("cat-file stdout")?;
        let mut reader = std::io::BufReader::new(stdout);
        for (reference, path, dest) in batchable.iter() {
            writeln!(stdin, "{reference}:{path}")
                .and_then(|()| stdin.flush())
                .with_context(|| format!("failed to feed cat-file spec '{reference}:{path}'"))?;

            let mut header = String::new();
            let n = reader.read_line(&mut header).context("cat-file header")?;
            if n == 0 {
                bail!("git cat-file --batch terminated early");
            }
            let trimmed = header.trim_end_matches('\n');
            let parts: Vec<&str> = trimmed.split(' ').collect();
            let blob_size: Option<usize> = parts
                .get(1)
                .filter(|t| **t == "blob")
                .and_then(|_| parts.get(2))
                .and_then(|sz| sz.parse().ok());
            let Some(size) = blob_size else {
                if trimmed.ends_with(" missing") {
                    present.insert(format!("{reference}:{path}"), false);
                    continue;
                }
                bail!("unexpected cat-file response for '{reference}:{path}': {trimmed}");
            };
            let mut buf = vec![0u8; size];
            std::io::Read::read_exact(&mut reader, &mut buf)
                .context("failed reading cat-file blob bytes")?;
            let mut newline = [0u8; 1];
            std::io::Read::read_exact(&mut reader, &mut newline)
                .context("failed reading cat-file terminator")?;
            std::fs::write(dest, &buf).with_context(|| {
                format!("failed to write blob of '{reference}:{path}' to '{dest}'")
            })?;
            present.insert(format!("{reference}:{path}"), true);
        }
        Ok(())
    })();
    drop(child.stdin.take());
    let _ = child.kill();
    let _ = child.wait();
    result?;
    Ok(present)
}

/// `git add` many paths in one invocation (stdin pathspec list where the
/// git version supports it).
pub fn stage_paths_batch(paths: &[String]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    if supports_pathspec_from_file() {
        return run_git_stdin(
            &["add", "--pathspec-from-file=-", "--pathspec-file-nul"],
            &nul_lines(paths.iter().map(String::as_str)),
        );
    }
    for chunk in pathspec_chunks(paths) {
        let mut args: Vec<&str> = vec!["add", "--"];
        args.extend(chunk.iter().map(String::as_str));
        run_git(&args)?;
    }
    Ok(())
}

/// Write worktree copies for paths already staged in the index (stdin when
/// git supports it).
fn restore_worktree_from_index(paths: &[String]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    if supports_pathspec_from_file() {
        return run_git_stdin(
            &["checkout-index", "-f", "-z", "--stdin"],
            &nul_lines(paths.iter().map(String::as_str)),
        );
    }
    for chunk in pathspec_chunks(paths) {
        let mut args: Vec<&str> = vec!["checkout-index", "-f", "--"];
        args.extend(chunk.iter().map(String::as_str));
        run_git(&args)?;
    }
    Ok(())
}

/// Re-evaluate the conflicted set against ONE concrete `chosen_base`.
///
/// With multiple merge bases (criss-cross) git's merge consults a *virtual*
/// base built from all of them and can report conflicts that no single base
/// would produce. The oracle is `git merge-tree --merge-base=<chosen_base>`:
/// git's own conflict semantics (rename detection included, so correlated
/// rename/rename or modify/delete conflicts survive) but pinned to the base
/// Mergetopus anchors to. Paths the pinned merge resolves cleanly are
/// materialized from its result tree exactly as that single-base merge would
/// have left them; genuinely conflicting paths stay untouched.
///
/// Returns the remaining conflicts. Falls back to the unchanged input set
/// when git is too old for the pinned merge-tree probe.
pub fn deconflict_against_base(
    conflicted: &[String],
    chosen_base: &str,
    ours_ref: &str,
    theirs_ref: &str,
) -> Result<Vec<String>> {
    if conflicted.is_empty() {
        return Ok(Vec::new());
    }
    let Some(pinned) = merge_tree_with_base(ours_ref, theirs_ref, chosen_base)? else {
        return Ok(conflicted.to_vec());
    };

    let mut remaining = Vec::new();
    let mut to_settle = Vec::new();
    for path in conflicted {
        if pinned.conflicts.contains(path.as_str()) {
            remaining.push(path.clone());
        } else {
            to_settle.push(path.clone());
        }
    }
    if to_settle.is_empty() {
        return Ok(remaining);
    }

    let entries = tree_entries(&pinned.tree, &to_settle)?;
    let present: Vec<(String, String, String)> = to_settle
        .iter()
        .filter_map(|p| {
            entries
                .get(p)
                .map(|(m, o)| (m.clone(), o.clone(), p.clone()))
        })
        .collect();
    let deleted: Vec<String> = to_settle
        .iter()
        .filter(|p| !entries.contains_key(p.as_str()))
        .cloned()
        .collect();

    write_stages_to_staged_paths(&present)?;
    resolve_paths_as_deleted_batch(&deleted)?;
    Ok(remaining)
}

/// Parse unmerged index entries (`git ls-files -u`) into
/// `path -> (stage -> blob)`. Stage 1 = base, 2 = ours, 3 = theirs.
/// Each entry keeps its index mode so materialization can preserve the
/// exec bit and symlink-ness.
pub fn conflict_stage_map() -> Result<
    std::collections::BTreeMap<String, std::collections::BTreeMap<usize, crate::models::StageBlob>>,
> {
    let out = run_git(&["ls-files", "-u", "-z"])?;
    let mut map = std::collections::BTreeMap::new();
    for record in out.split('\0') {
        if record.is_empty() {
            continue;
        }
        let Some((meta, path)) = record.split_once('\t') else {
            continue;
        };
        let mut meta_parts = meta.split_whitespace();
        let (Some(mode), Some(oid), Some(stage)) =
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
            .insert(
                stage,
                crate::models::StageBlob {
                    mode: mode.to_string(),
                    oid: oid.to_string(),
                },
            );
    }
    Ok(map)
}

/// Materialize an unmerged index stage at `path`: stage the exact blob with
/// its original mode via `update-index --cacheinfo`, then write the worktree
/// copy via `checkout-index`. Git itself writes the blob bytes, so binary
/// content and trailing newlines survive intact; modes 100755 (exec) and
/// 120000 (symlink) are preserved.
pub fn write_stage_to_staged_path(mode: &str, oid: &str, path: &str) -> Result<()> {
    if !oid.starts_with(|c: char| c.is_ascii_hexdigit()) || oid.len() < 4 {
        bail!("invalid blob oid '{oid}' while materializing '{path}'");
    }

    let fs_path = crate::win32_path::to_fs_path(path);
    if let Some(parent) = std::path::Path::new(&fs_path).parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create parent directory for '{path}'"))?;
    }

    run_git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("{mode},{oid},{path}"),
    ])
    .with_context(|| format!("failed to stage blob {oid} for '{path}'"))?;

    // Gitlinks (submodule pointers) have no blob to check out; staging is
    // the whole job.
    if mode == "160000" {
        return Ok(());
    }

    // Write the worktree copy from the freshly staged index entry. A
    // pre-existing file at the path is intentionally overwritten.
    let _ = std::fs::remove_file(&fs_path);
    run_git(&["checkout-index", "-f", "--", path])
        .with_context(|| format!("failed to check out staged '{path}'"))?;
    Ok(())
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

/// Names reachable from `merged_into`, split by where they live: local
/// branch tips and remote-tracking tips (mapped to local names). One
/// `git branch --merged` pair replaces a `merge-base --is-ancestor` probe
/// per branch on repos with hundreds of slice branches. Unresolvable
/// `merged_into` yields empty sets (everything "not merged"), matching the
/// per-probe fallback behavior instead of erroring.
pub fn merged_branch_sets(merged_into: &str) -> Result<(BTreeSet<String>, BTreeSet<String>)> {
    let mut locals = BTreeSet::new();
    let mut remotes = BTreeSet::new();
    let (ok, out, _) = run_git_allow_failure(&[
        "branch",
        "--merged",
        merged_into,
        "--format=%(refname:short)",
    ])?;
    if ok {
        for line in out.lines().map(str::trim).filter(|l| !l.is_empty()) {
            locals.insert(line.to_string());
        }
    }
    let (ok, out, _) = run_git_allow_failure(&[
        "branch",
        "-r",
        "--merged",
        merged_into,
        "--format=%(refname:short)",
    ])?;
    if ok {
        for line in out
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && *l != "origin/HEAD")
        {
            if let Some(local) = local_branch_name_from_remote_ref(line) {
                remotes.insert(local);
            }
        }
    }
    Ok((locals, remotes))
}

pub fn slice_merge_status(
    integration_branch: &str,
    slice_branches: &[String],
) -> Result<BTreeMap<String, bool>> {
    let (merged_local, merged_remote) = merged_branch_sets(integration_branch)?;
    let all_local = list_local_branches()?;
    let mut result = BTreeMap::new();
    for slice in slice_branches {
        // Mirror `best_ref_for_local_branch`'s preference: a local branch is
        // judged by its local tip even when a remote-tracking ref exists.
        let exists_local = all_local.iter().any(|b| b == slice);
        let merged = if exists_local {
            merged_local.contains(slice.as_str())
        } else {
            merged_remote.contains(slice.as_str())
        };
        result.insert(slice.clone(), merged);
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

// ── bulk path operations ───────────────────────────────────────────────────
//
// Huge repositories produce thousands of conflicted paths. One git
// subprocess per path dominates the wall time, so slicing goes through the
// helpers below. Where git accepts path lists on stdin they do
// (`--pathspec-from-file=- --pathspec-file-nul`, `update-index --index-info`,
// `checkout-index --stdin`), which removes any command-line length limit
// entirely — argv chunking remains only where git has no stdin alternative
// (`ls-tree` classification, `log` pathspecs), bounded so the whole argv
// fits comfortably in the Windows command-line limit (~32 KiB).

/// Run git with a payload on stdin; fail with the stderr on non-zero exit.
fn run_git_stdin(args: &[&str], payload: &[u8]) -> Result<()> {
    let mut child = Command::new("git")
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to execute git {}", args.join(" ")))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(payload)
            .with_context(|| format!("failed to feed paths to git {}", args.join(" ")))?;
    }
    let output = child
        .wait_with_output()
        .with_context(|| format!("failed to wait for git {}", args.join(" ")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git {} failed: {}", args.join(" "), stderr.trim());
    }
    Ok(())
}

/// NUL-terminated path lines.
fn nul_lines<'a>(paths: impl Iterator<Item = &'a str>) -> Vec<u8> {
    let mut buf = Vec::new();
    for p in paths {
        buf.extend_from_slice(p.as_bytes());
        buf.push(0);
    }
    buf
}

static PATHSPEC_FROM_FILE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

/// `--pathspec-from-file` exists since git 2.25 (Feb 2020).
fn supports_pathspec_from_file() -> bool {
    *PATHSPEC_FROM_FILE.get_or_init(|| {
        let version = match run_git(&["--version"]) {
            Ok(v) => v,
            Err(_) => return false,
        };
        let Some(rest) = version.trim().strip_prefix("git version ") else {
            return false;
        };
        let mut nums = rest
            .split('.')
            .take(2)
            .map(|part| part.parse::<u32>().unwrap_or(0));
        match (nums.next(), nums.next()) {
            (Some(major), Some(minor)) => (major, minor) >= (2, 25),
            _ => false,
        }
    })
}

/// Split paths into chunks bounded by count and total byte length, safe for
/// a single git pathspec argument list on every platform.
pub fn pathspec_chunks(paths: &[String]) -> Vec<Vec<String>> {
    const MAX_PATHS: usize = 400;
    const MAX_BYTES: usize = 12_000;
    let mut chunks: Vec<Vec<String>> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    let mut cur_len = 0usize;
    for p in paths {
        let l = p.len() + 1;
        if !cur.is_empty() && (cur.len() >= MAX_PATHS || cur_len + l > MAX_BYTES) {
            cur_len = 0;
            chunks.push(std::mem::take(&mut cur));
        }
        cur_len += l;
        cur.push(p.clone());
    }
    if !cur.is_empty() {
        chunks.push(cur);
    }
    chunks
}

/// Blob entries `(mode, oid)` of `paths` in `reference`; paths absent from
/// the tree are missing from the result. One `ls-tree` per chunk.
///
/// Paths containing newlines or double quotes can appear in quoted form in
/// the listing (git escapes them regardless of `core.quotePath`); those are
/// verified individually so exotic names never get silently misclassified.
pub fn tree_entries(
    reference: &str,
    paths: &[String],
) -> Result<BTreeMap<String, (String, String)>> {
    let mut found = BTreeMap::new();
    let mut exotic = Vec::new();
    let mut plain: Vec<String> = Vec::new();
    for p in paths {
        if p.contains('\n') || p.contains('"') {
            exotic.push(p.clone());
        } else {
            plain.push(p.clone());
        }
    }

    for chunk in pathspec_chunks(&plain) {
        let mut args: Vec<&str> = vec![
            "-c",
            "core.quotePath=false",
            "ls-tree",
            "-r",
            reference,
            "--",
        ];
        args.extend(chunk.iter().map(String::as_str));
        let entries_out = run_git(&args)?;
        for line in entries_out.lines() {
            // "<mode> <type> <oid>\t<path>"
            let Some((meta, name)) = line.split_once('\t') else {
                continue;
            };
            let mut parts = meta.split_whitespace();
            let (Some(mode), Some("blob"), Some(oid)) = (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            found.insert(name.to_string(), (mode.to_string(), oid.to_string()));
        }
    }

    for p in &exotic {
        if path_exists_in_ref(reference, p)? {
            let oid = run_git(&["rev-parse", &format!("{reference}:{p}")])?;
            let listing = run_git(&["ls-tree", reference, "--", p])?;
            let mode = listing
                .split_whitespace()
                .next()
                .unwrap_or("100644")
                .to_string();
            found.insert(p.clone(), (mode, oid.trim().to_string()));
        }
    }
    Ok(found)
}

/// `git restore --source=<reference> --staged --worktree` for many paths.
pub fn restore_paths_from_ref(reference: &str, paths: &[String]) -> Result<()> {
    for chunk in pathspec_chunks(paths) {
        let source_arg = format!("--source={reference}");
        let mut args: Vec<&str> = vec![
            "restore",
            source_arg.as_str(),
            "--staged",
            "--worktree",
            "--",
        ];
        args.extend(chunk.iter().map(String::as_str));
        run_git(&args)?;
    }
    Ok(())
}

/// Remove many paths from index and worktree in bulk (`git rm` over stdin).
pub fn rm_paths_batch(paths: &[String]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    if supports_pathspec_from_file() {
        return run_git_stdin(
            &[
                "rm",
                "-q",
                "--ignore-unmatch",
                "--pathspec-from-file=-",
                "--pathspec-file-nul",
            ],
            &nul_lines(paths.iter().map(String::as_str)),
        );
    }
    for chunk in pathspec_chunks(paths) {
        let mut args: Vec<&str> = vec!["rm", "-q", "--ignore-unmatch", "--"];
        args.extend(chunk.iter().map(String::as_str));
        run_git(&args)?;
    }
    Ok(())
}

/// Settle many conflicted paths as deletions (unmerged-index tolerant).
pub fn resolve_paths_as_deleted_batch(paths: &[String]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    if supports_pathspec_from_file() {
        run_git_stdin(
            &[
                "rm",
                "-f",
                "--cached",
                "--ignore-unmatch",
                "--pathspec-from-file=-",
                "--pathspec-file-nul",
            ],
            &nul_lines(paths.iter().map(String::as_str)),
        )?;
    } else {
        for chunk in pathspec_chunks(paths) {
            let mut args: Vec<&str> = vec!["rm", "-f", "--cached", "--ignore-unmatch", "--"];
            args.extend(chunk.iter().map(String::as_str));
            run_git(&args)?;
        }
    }
    for p in paths {
        let fs_path = crate::win32_path::to_fs_path(p);
        let _ = std::fs::remove_file(&fs_path);
    }
    Ok(())
}

/// Batched `restore_ours`: classify HEAD-membership with bulk tree listings
/// (identical predicate to the per-path version), then restore or delete in
/// bulk. Note an ours-stage entry can exist at paths HEAD does not contain
/// (dir-rename "file location" suggestions), which is why the stage map is
/// not used for classification.
pub fn restore_ours_batch(paths: &[String]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let present = tree_entries("HEAD", paths)?;
    let mut ours: Vec<String> = Vec::new();
    let mut deleted: Vec<String> = Vec::new();
    for p in paths {
        if present.contains_key(p) {
            ours.push(p.clone());
        } else {
            deleted.push(p.clone());
        }
    }
    if !ours.is_empty() {
        restore_paths_from_ref("HEAD", &ours)?;
    }
    if !deleted.is_empty() {
        resolve_paths_as_deleted_batch(&deleted)?;
    }
    Ok(())
}

/// The most recent commit touching each of `paths` on `source_sha`'s
/// simplified history, collected with one streamed `git log` per chunk
/// (killed early once every path is matched).
pub struct PathTouched {
    pub commit: String,
    pub author_name: String,
    pub author_email: String,
    pub author_date: String,
}

pub fn paths_last_touched(
    source_sha: &str,
    paths: &[String],
) -> Result<BTreeMap<String, PathTouched>> {
    let mut found: BTreeMap<String, PathTouched> = BTreeMap::new();
    let wanted: BTreeSet<String> = paths.iter().cloned().collect();

    for chunk in pathspec_chunks(paths) {
        let mut cmd = Command::new("git");
        cmd.args([
            "-c",
            "core.quotePath=false",
            "log",
            "--no-renames",
            "--name-only",
            "--format=%x01%H%x1f%an%x1f%ae%x1f%aI",
            source_sha,
            "--",
        ])
        .args(chunk.iter().map(String::as_str));
        cmd.stdout(std::process::Stdio::piped());
        let child = cmd.spawn().context("failed to spawn git log")?;
        // Early completion (or a panic while streaming) must not leave a git
        // process running nor leak its stdio pipe, so the guard kills and
        // reaps the child on every scope exit.
        let mut guard = ChildGuard::new(child);
        let child = &mut guard.child;
        if let Some(out) = child.stdout.take() {
            let reader = std::io::BufReader::new(out);
            let mut current: Option<PathTouched> = None;
            for line in reader.lines().map_while(Result::ok) {
                if let Some(rest) = line.strip_prefix('\u{1}') {
                    let mut parts = rest.split('\u{1f}');
                    current = Some(PathTouched {
                        commit: parts.next().unwrap_or("").to_string(),
                        author_name: parts.next().unwrap_or("").to_string(),
                        author_email: parts.next().unwrap_or("").to_string(),
                        author_date: parts.next().unwrap_or("").to_string(),
                    });
                    if found.len() == wanted.len() {
                        break;
                    }
                    continue;
                }
                if line.is_empty() {
                    continue;
                }
                if wanted.contains(&line) && !found.contains_key(&line) {
                    let Some(t) = &current else { continue };
                    found.insert(
                        line.clone(),
                        PathTouched {
                            commit: t.commit.clone(),
                            author_name: t.author_name.clone(),
                            author_email: t.author_email.clone(),
                            author_date: t.author_date.clone(),
                        },
                    );
                    if found.len() == wanted.len() {
                        break;
                    }
                }
            }
        }
    }
    Ok(found)
}

/// Kill+reap a streaming child process on scope exit.
struct ChildGuard {
    child: std::process::Child,
}

impl ChildGuard {
    fn new(child: std::process::Child) -> Self {
        ChildGuard { child }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
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

pub fn three_way_diff(path: &str, base_ref: &str, source_ref: &str) -> Result<String> {
    let ours = show_file_at("HEAD", path)?;
    let base_txt = show_file_at(base_ref, path)?;
    let theirs = show_file_at(source_ref, path)?;

    Ok(format!(
        "=== OURS (HEAD) ===\n{ours}\n\n=== BASE ({base_ref}) ===\n{base_txt}\n\n=== THEIRS ({source_ref}) ===\n{theirs}"
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
        conflict_stage_map, deconflict_against_base, is_slice_branch_ref, list_all_slice_branches,
        list_slice_branches_for_integration, prepare_absent_side_file,
        resolve_paths_as_deleted_batch, restore_ours, write_blob_to_path,
        write_stage_to_staged_path,
    };
    use super::{conflicted_files, merge_base};
    use crate::test_support as test_helpers;

    type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

    #[test]
    fn bulk_deletion_streams_more_paths_than_an_argv_chunk() -> TestResult<()> {
        // 500 > pathspec_chunks' 400-path cap: the stdin path must handle
        // the whole set without splitting at chunk boundaries losing entries.
        let repo = test_helpers::init_repo()?;
        let count = 500;
        for i in 0..count {
            test_helpers::write_file(&repo, &format!("pkg/f{i:03}.txt"), &format!("base{i}\n"))?;
        }
        test_helpers::commit_all(&repo, "base")?;

        test_helpers::git(&repo, &["checkout", "-b", "feature"])?;
        for i in 0..count {
            test_helpers::write_file(&repo, &format!("pkg/f{i:03}.txt"), &format!("feat{i}\n"))?;
        }
        test_helpers::commit_all(&repo, "feature")?;
        test_helpers::git(&repo, &["checkout", "main"])?;
        for i in 0..count {
            test_helpers::write_file(&repo, &format!("pkg/f{i:03}.txt"), &format!("main{i}\n"))?;
        }
        test_helpers::commit_all(&repo, "main")?;

        test_helpers::git(&repo, &["merge", "--no-commit", "feature"]).ok();
        let conflicts = test_helpers::with_repo_cwd(&repo, conflicted_files)?;
        assert_eq!(conflicts.len(), count);

        test_helpers::with_repo_cwd(&repo, || resolve_paths_as_deleted_batch(&conflicts))?;

        let still = test_helpers::with_repo_cwd(&repo, conflicted_files)?;
        assert!(still.is_empty(), "all {count} paths settled: {still:?}");
        assert!(!repo.join("pkg/f000.txt").exists());
        assert!(!repo.join("pkg/f499.txt").exists());
        Ok(())
    }

    #[test]
    fn write_stage_preserves_binary_bytes_and_exec_mode() -> TestResult<()> {
        let repo = test_helpers::init_repo()?;
        test_helpers::write_file(&repo, "seed.txt", "seed\n")?;
        test_helpers::commit_all(&repo, "seed")?;

        let payload: &[u8] = b"PK\x00\x01binary blob without trailing newline\x00\xff";
        std::fs::write(repo.join("payload.bin"), payload)?;
        let oid = test_helpers::git(&repo, &["hash-object", "-w", "payload.bin"])?;

        test_helpers::with_repo_cwd(&repo, || {
            write_stage_to_staged_path("100755", &oid, "out/exec.bin")
        })?;

        let written = std::fs::read(repo.join("out/exec.bin"))?;
        assert_eq!(written, payload, "blob bytes must be written unchanged");

        let index_line = test_helpers::git(&repo, &["ls-files", "-s", "out/exec.bin"])?;
        assert!(
            index_line.starts_with("100755 "),
            "index mode must survive: {index_line}"
        );
        assert!(index_line.contains(oid.trim()));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(repo.join("out/exec.bin"))?
                .permissions()
                .mode();
            assert_ne!(mode & 0o111, 0, "exec bit must be set on the worktree copy");
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn write_stage_creates_symlink_for_120000() -> TestResult<()> {
        let repo = test_helpers::init_repo()?;
        test_helpers::write_file(&repo, "seed.txt", "seed\n")?;
        test_helpers::commit_all(&repo, "seed")?;

        std::fs::write(repo.join("linksrc.txt"), b"seed.txt")?;
        let oid = test_helpers::git(&repo, &["hash-object", "-w", "linksrc.txt"])?;

        test_helpers::with_repo_cwd(&repo, || {
            write_stage_to_staged_path("120000", &oid, "dir/link")
        })?;

        let link = std::fs::read_link(repo.join("dir/link"))?;
        assert_eq!(link, std::path::Path::new("seed.txt"));
        let index_line = test_helpers::git(&repo, &["ls-files", "-s", "dir/link"])?;
        assert!(
            index_line.starts_with("120000 "),
            "symlink mode must be staged: {index_line}"
        );
        Ok(())
    }

    #[test]
    fn deconflict_against_base_settles_virtual_base_artifacts() -> TestResult<()> {
        let repo = test_helpers::init_repo()?;
        test_helpers::write_file(&repo, "f.txt", "0\n")?;
        test_helpers::commit_all(&repo, "A")?;
        test_helpers::git(&repo, &["checkout", "-b", "b1"])?;
        test_helpers::write_file(&repo, "f.txt", "1\n")?;
        test_helpers::commit_all(&repo, "B1")?;
        let b1 = test_helpers::git(&repo, &["rev-parse", "b1"])?;
        test_helpers::git(&repo, &["checkout", "-b", "b2", "main"])?;
        test_helpers::write_file(&repo, "f.txt", "2\n")?;
        test_helpers::commit_all(&repo, "B2")?;
        let b2 = test_helpers::git(&repo, &["rev-parse", "b2"])?;

        // X merges B2 keeping f=1, Y merges B1 keeping f=2 → criss-cross.
        test_helpers::git(&repo, &["checkout", "b1"])?;
        test_helpers::git(&repo, &["merge", "--no-commit", b2.trim()]).ok();
        test_helpers::write_file(&repo, "f.txt", "1\n")?;
        test_helpers::git(&repo, &["add", "f.txt"])?;
        test_helpers::git(&repo, &["commit", "-m", "X"])?;
        let x = test_helpers::git(&repo, &["rev-parse", "b1"])?;
        test_helpers::git(&repo, &["checkout", "b2"])?;
        test_helpers::git(&repo, &["merge", "--no-commit", b1.trim()]).ok();
        test_helpers::write_file(&repo, "f.txt", "2\n")?;
        test_helpers::git(&repo, &["add", "f.txt"])?;
        test_helpers::git(&repo, &["commit", "-m", "Y"])?;
        let y = test_helpers::git(&repo, &["rev-parse", "b2"])?;

        // Merging X and Y: git's virtual base conflicts f although any
        // single base resolves it cleanly.
        test_helpers::git(&repo, &["checkout", "b1"])?;
        test_helpers::git(&repo, &["merge", "--no-ff", "--no-commit", y.trim()]).ok();
        let raw = test_helpers::with_repo_cwd(&repo, conflicted_files)?;
        assert!(
            raw.contains(&"f.txt".to_string()),
            "git must report f.txt: {raw:?}"
        );

        let default = test_helpers::with_repo_cwd(&repo, || merge_base(x.trim(), y.trim()))?;
        let base_f = test_helpers::git(&repo, &["show", &format!("{}:f.txt", default.trim())])?;
        let remaining = test_helpers::with_repo_cwd(&repo, || {
            deconflict_against_base(&raw, default.trim(), x.trim(), y.trim())
        })?;
        assert!(
            remaining.is_empty(),
            "single-base conflict set: {remaining:?}"
        );

        // Expectation follows the chosen base: the side equal to it yields.
        let expected = if base_f.trim() == "1" { "2" } else { "1" };
        let content = std::fs::read_to_string(repo.join("f.txt"))?;
        assert_eq!(content.trim(), expected);
        let still = test_helpers::with_repo_cwd(&repo, conflicted_files)?;
        assert!(still.is_empty(), "index settled: {still:?}");
        Ok(())
    }

    #[test]
    fn deconflict_against_base_keeps_real_conflicts_on_single_base() -> TestResult<()> {
        let repo = test_helpers::setup_single_conflict_repo()?;
        test_helpers::git(&repo, &["merge", "--no-commit", "feature"]).ok();
        let raw = test_helpers::with_repo_cwd(&repo, conflicted_files)?;
        assert!(!raw.is_empty());

        let head = test_helpers::git(&repo, &["rev-parse", "HEAD"])?;
        let feature = test_helpers::git(&repo, &["rev-parse", "feature"])?;
        let base = test_helpers::with_repo_cwd(&repo, || merge_base(head.trim(), feature.trim()))?;
        let remaining = test_helpers::with_repo_cwd(&repo, || {
            deconflict_against_base(&raw, base.trim(), head.trim(), feature.trim())
        })?;
        assert_eq!(
            remaining, raw,
            "a genuine two-sided conflict must survive deconfliction"
        );
        Ok(())
    }

    #[test]
    fn conflict_stage_map_preserves_modes() -> TestResult<()> {
        let repo = test_helpers::init_repo()?;

        // Build the trees directly (hash-object + cacheinfo) so the exec
        // stage modes hold regardless of host filesystem permissions.
        let put_exec = |content: &[u8], message: &str| -> TestResult<()> {
            std::fs::write(repo.join(".tmpblob"), content)?;
            let oid = test_helpers::git(&repo, &["hash-object", "-w", ".tmpblob"])?;
            test_helpers::git(
                &repo,
                &[
                    "update-index",
                    "--add",
                    "--cacheinfo",
                    &format!("100755,{},run.sh", oid.trim()),
                ],
            )?;
            test_helpers::git(&repo, &["commit", "-m", message])?;
            // cacheinfo only touched the index; sync the worktree so a
            // later merge isn't refused over stale local content.
            test_helpers::git(&repo, &["checkout", "-f", "--", "run.sh"]).ok();
            Ok(())
        };

        put_exec(b"one\n", "base with exec")?;
        test_helpers::git(&repo, &["checkout", "-b", "feature"])?;
        put_exec(b"feature\n", "feature edits")?;
        test_helpers::git(&repo, &["checkout", "main"])?;
        put_exec(b"main\n", "main edits")?;

        test_helpers::git(&repo, &["merge", "--no-commit", "feature"]).ok();
        let map = test_helpers::with_repo_cwd(&repo, conflict_stage_map)?;
        let stages = map.get("run.sh").expect("conflicted entry for run.sh");
        assert_eq!(stages.get(&2).expect("ours stage").mode, "100755");
        assert_eq!(stages.get(&3).expect("theirs stage").mode, "100755");
        Ok(())
    }

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
