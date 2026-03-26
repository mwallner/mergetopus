use anyhow::{Context, Result, bail};

use crate::git_ops;
use crate::helpers;
use crate::planner;
use crate::tui;

/// Resolve a slice by merging it into the corresponding integration branch with
/// `--no-commit`, then invoking the configured merge tool for each conflicted
/// file one-by-one.
///
/// For each conflicted file the function writes three temporary files:
/// - LOCAL  – the version from the integration branch HEAD before the merge
/// - BASE   – the merge-base between integration HEAD and the slice tip
/// - REMOTE – the version from the slice branch tip
///
/// It then sets LOCAL / BASE / REMOTE / MERGED as shell environment variables
/// and executes the command from `git config mergetool.<tool>.cmd`. MERGED
/// points to the conflicted working-tree file on the integration branch.
pub(crate) fn resolve_command(
    branch_arg: Option<&str>,
    do_commit: bool,
    quiet: bool,
    tui_title: &str,
) -> Result<()> {
    let slice_branch = if let Some(b) = branch_arg {
        if !git_ops::branch_exists(b)? {
            bail!("branch '{}' does not exist", b);
        }
        b.to_string()
    } else {
        if quiet {
            bail!("--quiet requires BRANCH to be provided for 'resolve'");
        }
        let all_slices = git_ops::list_all_slice_branches()?;
        if all_slices.is_empty() {
            bail!("no slice branches found in this repository");
        }
        match tui::pick_branch(&all_slices, tui_title)? {
            Some(b) => b,
            None => bail!("slice branch selection was canceled"),
        }
    };

    let integration_branch = planner::integration_from_slice_branch(&slice_branch).ok_or_else(|| {
        anyhow::anyhow!(
            "could not derive integration branch from slice branch '{}' (expected suffix '/slice<N>')",
            slice_branch
        )
    })?;
    if !git_ops::branch_exists(&integration_branch)? {
        bail!(
            "corresponding integration branch '{}' does not exist for slice '{}'",
            integration_branch,
            slice_branch
        );
    }

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
        git_ops::ensure_git_context()?;
        git_ops::checkout(&integration_branch)?;
        let local = git_ops::head_sha()?;
        git_ops::merge_no_commit(&slice_branch)?;
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
        println!(
            "No conflicted files remain for merge '{}' into '{}'.",
            slice_branch, integration_branch
        );

        if do_commit {
            if git_ops::staged_has_changes()? || git_ops::merge_in_progress()? {
                let msg =
                    format!("Mergetopus resolve: '{slice_branch}' into '{integration_branch}'");
                git_ops::commit_strict(&msg)?;
                println!("  Merge commit created on '{integration_branch}'.");
            } else {
                println!("  No staged changes to commit.");
            }
        } else {
            println!("  Merge result is staged on '{integration_branch}' but not committed.");
            println!("  Review and commit when ready, or re-run with --commit.");
        }

        return Ok(());
    }

    let tmp_dir = std::env::temp_dir().join(format!("mergetopus-{}", std::process::id()));
    std::fs::create_dir_all(&tmp_dir)
        .context("failed to create temporary directory for merge tool files")?;

    for path in &conflicted_paths {
        let safe_name = path
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect::<String>();
        let local_tmp = tmp_dir
            .join(format!("{safe_name}.LOCAL"))
            .to_string_lossy()
            .into_owned();
        let base_tmp = tmp_dir
            .join(format!("{safe_name}.BASE"))
            .to_string_lossy()
            .into_owned();
        let remote_tmp = tmp_dir
            .join(format!("{safe_name}.REMOTE"))
            .to_string_lossy()
            .into_owned();

        git_ops::write_blob_to_path(&local_commit, path, &local_tmp)?;
        git_ops::write_blob_to_path(&merge_base, path, &base_tmp)?;
        git_ops::write_blob_to_path(&remote_commit, path, &remote_tmp)?;

        let merged_before = std::fs::read(path).ok();
        let base_before = std::fs::read(&base_tmp)
            .with_context(|| format!("failed to read temporary BASE file for '{path}'"))?;

        println!("Resolving '{path}' with '{tool_name}'...");

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

        if !status.success() {
            eprintln!(
                "warning: merge tool exited with non-zero status for '{path}' \
                 (exit code: {}); the file has been staged as-is – \
                 please verify the resolution manually before committing",
                status.code().unwrap_or(-1)
            );
        }

        if !cmd_uses_merged {
            let merged_after = std::fs::read(path).ok();
            if merged_after == merged_before {
                let base_after = std::fs::read(&base_tmp)
                    .with_context(|| format!("failed to read temporary BASE file for '{path}'"))?;
                if base_after != base_before {
                    std::fs::write(path, &base_after)
                        .with_context(|| format!("failed to write resolved content to '{path}'"))?;
                    println!("Applied '{tool_name}' output from BASE temp file back to '{path}'.");
                }
            }
        }

        git_ops::stage_path(path)?;
    }

    let paths_list = conflicted_paths.join(", ");

    println!(
        "Resolve complete for merge '{}' into '{}'",
        slice_branch, integration_branch
    );
    println!(
        "  Resolved {} file(s): {}",
        conflicted_paths.len(),
        paths_list
    );

    if do_commit {
        if git_ops::staged_has_changes()? || git_ops::merge_in_progress()? {
            let msg = format!(
                "Mergetopus resolve: '{slice_branch}' into '{integration_branch}'\n\nResolved-Paths: {paths_list}\nSource-Commit: {remote_commit}"
            );
            git_ops::commit_strict(&msg)?;
            println!("  Merge commit created on '{integration_branch}'.");
        } else {
            println!("  No staged changes to commit.");
        }
    } else {
        println!("  Merge result is staged on '{integration_branch}' but not committed.");
        println!("  Review and commit when ready, or re-run with --commit.");
    }

    Ok(())
}
