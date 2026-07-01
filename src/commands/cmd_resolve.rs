use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::color;
use crate::git_ops;
use crate::helpers;
use crate::planner;
use crate::tui;
use crate::tui_progress;

/// RAII guard that creates a temporary directory on construction and deletes
/// it (including all contents) when the guard is dropped, covering all exit
/// paths: normal return, early bail, panic unwind, and Ctrl-C.
struct MergetopusTempDir {
    path: PathBuf,
    cleaned: bool,
}

impl MergetopusTempDir {
    fn new() -> Result<Self> {
        let pid = std::process::id();
        let base = std::env::temp_dir().join(format!("mergetopus-{pid}"));
        // Use a counter suffix so sequential resolves in the same process
        // each get a fresh directory.
        let path = {
            let mut n = 0u64;
            loop {
                let candidate = if n == 0 {
                    base.clone()
                } else {
                    base.with_extension(n.to_string())
                };
                if !candidate.exists() {
                    break candidate;
                }
                n += 1;
            }
        };
        std::fs::create_dir_all(&path)
            .context("failed to create temporary directory for merge tool files")?;
        Ok(MergetopusTempDir { path, cleaned: false })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    /// Mark the directory as successfully cleaned — prevents the Drop handler
    /// from printing a warning on normal exit paths.
    fn clean(&mut self) {
        self.cleaned = true;
    }
}

impl Drop for MergetopusTempDir {
    fn drop(&mut self) {
        if self.cleaned {
            return;
        }
        if let Err(e) = std::fs::remove_dir_all(&self.path) {
            // Only warn on actual errors (not "not found" — that's fine).
            if e.kind() != std::io::ErrorKind::NotFound {
                let _ = std::fs::remove_dir_all(&self.path);
                eprintln!(
                    "warning: failed to remove temporary directory '{}': {e}",
                    self.path.display()
                );
            }
        }
    }
}

/// Check whether a file still contains git conflict markers.
fn has_conflict_markers(path: &str) -> bool {
    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };
    let mut has_ours = false;
    let mut has_separator = false;
    for line in content.lines() {
        if line.starts_with("<<<<<<<") {
            has_ours = true;
        } else if has_ours && line.starts_with("=======") {
            has_separator = true;
        } else if has_separator && line.starts_with(">>>>>>>") {
            return true;
        }
    }
    false
}

/// Determine the effective trustExitCode setting for the given merge tool.
///
/// Precedence: `mergetool.<tool>.trustExitCode` > `mergetool.trustExitCode`.
/// Returns `Some(true)` / `Some(false)` when explicitly configured, `None` when unset.
fn effective_trust_exit_code(tool_name: &str) -> Result<Option<bool>> {
    if let Some(val) = git_ops::get_git_config(&format!("mergetool.{tool_name}.trustExitCode"))? {
        return Ok(Some(val == "true"));
    }
    if let Some(val) = git_ops::get_git_config("mergetool.trustExitCode")? {
        return Ok(Some(val == "true"));
    }
    Ok(None)
}

/// Decide whether a resolved file should be staged, based on the mergetool
/// exit status, conflict marker presence, and `trustExitCode` configuration.
///
/// Returns `true` if the file should be staged, `false` to skip.
fn should_stage_after_mergetool(
    path: &str,
    exit_success: bool,
    trust_exit_code: Option<bool>,
    quiet: bool,
    tui_title: &str,
) -> Result<bool> {
    match trust_exit_code {
        Some(true) => {
            // Explicit trust: rely solely on exit code.
            if !exit_success {
                color::print_error(&format!(
                    "warning: merge tool exited with non-zero status for '{path}'; \
                     skipping staging (trustExitCode is enabled)"
                ), None);
                return Ok(false);
            }
            Ok(true)
        }
        Some(false) => {
            // Explicit distrust: always stage regardless of exit code.
            Ok(true)
        }
        None => {
            // Unset — variant C: check exit code AND conflict markers.
            let markers = has_conflict_markers(path);
            if exit_success && !markers {
                return Ok(true);
            }

            let reason = match (!exit_success, markers) {
                (true, true) => {
                    "merge tool exited with non-zero status and conflict markers remain"
                }
                (true, false) => "merge tool exited with non-zero status",
                (false, true) => "conflict markers remain in the file",
                (false, false) => unreachable!(),
            };

            if quiet {
                color::print_error(&format!("warning: {reason} for '{path}'; skipping staging in --quiet mode"), None);
                return Ok(false);
            }

            let prompt = format!(
                "File '{path}' may not be fully resolved ({reason}).\n\n\
                 Stage it anyway?"
            );
            tui::confirm(&prompt, tui_title)
        }
    }
}

/// Resolves one slice branch into its integration branch using the configured
/// git mergetool, stages resolved paths, and optionally creates the merge commit.
///
/// Behavior summary:
/// - derives the integration branch from the slice name
/// - reuses an existing in-progress merge when MERGE_HEAD already matches slice
/// - otherwise checks out the integration branch and runs `git merge --no-commit`
/// - launches `mergetool.<tool>.cmd` per conflicted path with LOCAL/BASE/REMOTE
///   temp files and MERGED pointing at the working-tree file
/// - stages each processed path and, with `do_commit`, commits the merge result
pub fn resolve_command(
    branch_arg: Option<&str>,
    do_commit: bool,
    quiet: bool,
    tui_title: &str,
) -> Result<()> {
    git_ops::ensure_longpaths_support()?;

    let selected_slice = if let Some(b) = branch_arg {
        b.to_string()
    } else {
        if quiet {
            bail!("--quiet requires BRANCH to be provided for 'resolve'");
        }
        let all_slices = git_ops::list_all_slice_branches()?;
        if all_slices.is_empty() {
            bail!("no slice branches found in this repository");
        }
        match tui::pick_branch(&all_slices, tui_title, None, &[])? {
            Some(b) => b,
            None => bail!("slice branch selection was canceled"),
        }
    };

    let slice_branch = git_ops::ensure_local_branch_for_operation(&selected_slice)?;

    let derived_integration = planner::integration_from_slice_branch(&slice_branch).ok_or_else(|| {
        anyhow::anyhow!(
            "could not derive integration branch from slice branch '{}' (expected suffix '/slice<N>')",
            slice_branch
        )
    })?;
    let integration_branch = git_ops::ensure_local_branch_for_operation(&derived_integration)
        .with_context(|| {
            format!(
                "corresponding integration branch '{}' could not be materialized for slice '{}'",
                derived_integration, slice_branch
            )
        })?;

    let slice_commit = git_ops::resolve_commit(&slice_branch)?;
    let merge_in_progress = git_ops::merge_in_progress()?;
    let current_branch = git_ops::current_branch()?;

    let (local_commit, remote_commit) = if merge_in_progress {
        if current_branch != integration_branch {
            bail!(
                "a merge is already in progress on branch '{}'; resolve or abort it before resolving '{}'",
                current_branch,
                slice_branch
            );
        }

        let merge_head = git_ops::merge_head_sha()?;
        if merge_head != slice_commit {
            bail!(
                "a different merge is already in progress on '{}' (MERGE_HEAD = {}), not the selected slice '{}'",
                integration_branch,
                merge_head,
                slice_branch
            );
        }

        (git_ops::head_sha()?, merge_head)
    } else {
        if !quiet {
            let ib = integration_branch.clone();
            let sb = slice_branch.clone();
            tui_progress::run_progress(
                tui_title,
                vec![
                    tui_progress::ProgressStep {
                        label: "Preparing integration branch".into(),
                        action: Box::new(move || {
                            git_ops::ensure_git_context()?;
                            git_ops::checkout(&ib)
                        }),
                    },
                    tui_progress::ProgressStep {
                        label: format!("Merging slice branch: {slice_branch}"),
                        action: Box::new(move || git_ops::merge_no_commit(&sb)),
                    },
                ],
            )?;
        } else {
            git_ops::ensure_git_context()?;
            git_ops::checkout(&integration_branch)?;
            git_ops::merge_no_commit(&slice_branch)?;
        }
        let local = git_ops::head_sha()?;
        (local, slice_commit.clone())
    };

    let merge_base = git_ops::merge_base(&local_commit, &remote_commit)?;
    let conflicted_paths = git_ops::conflicted_files()?;

    let tool_name = match git_ops::get_git_config("merge.tool")? {
        Some(t) => t,
        None => bail!(
            "no merge tool configured; set one with:\n  \
             git config merge.tool <tool>\n  \
             git config mergetool.<tool>.cmd '<cmd with $LOCAL $BASE $REMOTE $MERGED>'"
        ),
    };
    let trust_exit_code = effective_trust_exit_code(&tool_name)?;
    let tool_cmd = match git_ops::get_git_config(&format!("mergetool.{tool_name}.cmd"))? {
        Some(c) => c,
        None => bail!(
            "no command configured for merge tool '{tool_name}'; set one with:\n  \
             git config mergetool.{tool_name}.cmd '<cmd with $LOCAL $BASE $REMOTE [optional $MERGED]>'"
        ),
    };
    let cmd_uses_merged = tool_cmd.contains("$MERGED")
        || tool_cmd.contains("${MERGED}")
        || tool_cmd.contains("%MERGED%");

    if conflicted_paths.is_empty() {
        color::print_info(&format!(
            "No conflicted files remain for merge '{}' into '{}'.",
            slice_branch, integration_branch
        ), None);

        if do_commit {
            if git_ops::staged_has_changes()? || git_ops::merge_in_progress()? {
                let msg =
                    format!("Mergetopus resolve: '{slice_branch}' into '{integration_branch}'");
                git_ops::commit_strict(&msg)?;
                color::print_success(&format!("  Merge commit created on '{integration_branch}'."), None);
            } else {
                color::print_info("  No staged changes to commit.", None);
            }
        } else {
            color::print_info(&format!("  Merge result is staged on '{integration_branch}' but not committed."), None);
            color::print_info("  Review and commit when ready, or re-run with --commit.", None);
        }

        return Ok(());
    }

    let mut tmp_dir = MergetopusTempDir::new()?;

    let mut skipped_paths: Vec<String> = Vec::new();

    for path in &conflicted_paths {
        // Derive a short safe name from the filename alone, suffixed with
        // a hash of the full path to keep unique even if the same filename
        // appears in different directories.
        let filename = path.rsplit_once(['/', '\\'])
            .map(|(_, name)| name)
            .unwrap_or(path);
        let safe_file: String = filename
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let hash = path.bytes().fold(0u32, |acc, b| {
            acc.wrapping_mul(31).wrapping_add(b as u32)
        });
        let safe_name = format!("{}_{:08x}", safe_file, hash);

        let local_tmp = tmp_dir
            .path()
            .join(format!("{safe_name}.LOCAL"))
            .to_string_lossy()
            .into_owned();
        let base_tmp = tmp_dir
            .path()
            .join(format!("{safe_name}.BASE"))
            .to_string_lossy()
            .into_owned();
        let remote_tmp = tmp_dir
            .path()
            .join(format!("{safe_name}.REMOTE"))
            .to_string_lossy()
            .into_owned();

        git_ops::write_blob_to_path(&local_commit, path, &local_tmp)?;
        git_ops::write_blob_to_path(&merge_base, path, &base_tmp)?;
        git_ops::write_blob_to_path(&remote_commit, path, &remote_tmp)?;

        let merged_before = std::fs::read(path).ok();
        let base_before = std::fs::read(&base_tmp)
            .with_context(|| format!("failed to read temporary BASE file for '{path}'"))?;

        color::print_info(&format!("Resolving '{path}' with '{tool_name}'..."), None);

        // Substitute variables in the command to handle both Unix-style ($VAR, ${VAR})
        // and Windows-style (%VAR%) variable references consistently across platforms.
        let expanded_cmd = tool_cmd
            .replace("$LOCAL", &local_tmp)
            .replace("${LOCAL}", &local_tmp)
            .replace("%LOCAL%", &local_tmp)
            .replace("$BASE", &base_tmp)
            .replace("${BASE}", &base_tmp)
            .replace("%BASE%", &base_tmp)
            .replace("$REMOTE", &remote_tmp)
            .replace("${REMOTE}", &remote_tmp)
            .replace("%REMOTE%", &remote_tmp)
            .replace("$MERGED", path)
            .replace("${MERGED}", path)
            .replace("%MERGED%", path);

        let status = if cfg!(target_os = "windows") {
            helpers::run_windows_merge_tool(&tool_name, &expanded_cmd)?
        } else {
            std::process::Command::new("sh")
                .args(["-c", &expanded_cmd])
                .status()
                .with_context(|| format!("failed to launch merge tool '{tool_name}'"))?
        };

        if !cmd_uses_merged {
            let merged_after = std::fs::read(path).ok();
            if merged_after == merged_before {
                let base_after = std::fs::read(&base_tmp)
                    .with_context(|| format!("failed to read temporary BASE file for '{path}'"))?;
                if base_after != base_before {
                    std::fs::write(path, &base_after)
                        .with_context(|| format!("failed to write resolved content to '{path}'"))?;
                    color::print_info(&format!("Applied '{tool_name}' output from BASE temp file back to '{path}'."), None);
                }
            }
        }

        if should_stage_after_mergetool(path, status.success(), trust_exit_code, quiet, tui_title)?
        {
            git_ops::stage_path(path)?;
        } else {
            skipped_paths.push(path.clone());
        }
    }

    let staged_count = conflicted_paths.len() - skipped_paths.len();
    let staged_paths: Vec<&String> = conflicted_paths
        .iter()
        .filter(|p| !skipped_paths.contains(p))
        .collect();
    let paths_list = staged_paths
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(", ");

    color::print_success(&format!(
        "Resolve complete for merge '{}' into '{}'",
        slice_branch, integration_branch
    ), None);
    color::print_info(&format!(
        "  Staged {} file(s): {}",
        staged_count,
        if paths_list.is_empty() {
            "(none)".to_string()
        } else {
            paths_list.clone()
        }
    ), None);
    if !skipped_paths.is_empty() {
        color::print_warning(&format!(
            "  Skipped {} file(s): {}",
            skipped_paths.len(),
            skipped_paths.join(", ")
        ), None);
    }

    if do_commit {
        if git_ops::staged_has_changes()? || git_ops::merge_in_progress()? {
            let msg = format!(
                "Mergetopus resolve: '{slice_branch}' into '{integration_branch}'\n\nResolved-Paths: {paths_list}\nSource-Commit: {remote_commit}"
            );
            git_ops::commit_strict(&msg)?;
            color::print_success(&format!("  Merge commit created on '{integration_branch}'."), None);
        } else {
            color::print_info("  No staged changes to commit.", None);
        }
    } else {
        color::print_info(&format!("  Merge result is staged on '{integration_branch}' but not committed."), None);
        color::print_info("  Review and commit when ready, or re-run with --commit.", None);
    }

    tmp_dir.clean();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_temp_file(content: &str) -> String {
        let dir =
            std::env::temp_dir().join(format!("mergetopus-test-resolve-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir
            .join(format!(
                "test-{}.txt",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ))
            .to_string_lossy()
            .into_owned();
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn conflict_markers_detected_in_standard_conflict() {
        let path =
            write_temp_file("before\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> branch\nafter\n");
        assert!(has_conflict_markers(&path));
    }

    #[test]
    fn no_conflict_markers_in_clean_file() {
        let path = write_temp_file("just some\nclean content\n");
        assert!(!has_conflict_markers(&path));
    }

    #[test]
    fn partial_markers_not_detected_as_conflict() {
        // Only opening marker, no separator or closing
        let path = write_temp_file("<<<<<<< HEAD\nsome text\n");
        assert!(!has_conflict_markers(&path));
    }

    #[test]
    fn opening_and_separator_without_closing_not_detected() {
        let path = write_temp_file("<<<<<<< HEAD\nours\n=======\ntheirs\n");
        assert!(!has_conflict_markers(&path));
    }

    #[test]
    fn markers_must_appear_in_order() {
        // Closing before opening — should not match
        let path = write_temp_file(">>>>>>> branch\n=======\n<<<<<<< HEAD\n");
        assert!(!has_conflict_markers(&path));
    }

    #[test]
    fn nonexistent_file_has_no_markers() {
        assert!(!has_conflict_markers("/nonexistent/path/to/file.txt"));
    }

    #[test]
    fn multiple_conflict_blocks_detected() {
        let path = write_temp_file(
            "<<<<<<< HEAD\na\n=======\nb\n>>>>>>> x\nok\n<<<<<<< HEAD\nc\n=======\nd\n>>>>>>> y\n",
        );
        assert!(has_conflict_markers(&path));
    }
}
