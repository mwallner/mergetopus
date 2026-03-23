use anyhow::{Context, Result, bail};

use crate::cli::{Args, Commands};
use crate::git_ops;
use crate::license;
use crate::models::SlicePlanItem;
use crate::planner;
use crate::tui;

pub fn run(args: Args) -> Result<()> {
    if let Some(Commands::License { full, json_output }) = &args.command {
        license::print_license(*full, *json_output);
        return Ok(());
    }

    if let Some(Commands::Resolve { branch, commit }) = &args.command {
        git_ops::ensure_git_worktree()?;
        let current_branch = git_ops::current_branch()?;
        let tui_title = format!("Mergetopus [{current_branch}]");
        return resolve_command(branch.as_deref(), *commit, args.quiet, &tui_title);
    }

    git_ops::ensure_git_context()?;
    let current_branch = git_ops::current_branch()?;
    let tui_title = format!("Mergetopus [{current_branch}]");

    run_merge_workflow(&args, &current_branch, &tui_title)
}

fn run_merge_workflow(args: &Args, current_branch: &str, tui_title: &str) -> Result<()> {
    let source_ref = match args.source.as_ref() {
        Some(s) => s.clone(),
        None => {
            if args.quiet {
                bail!("--quiet requires SOURCE to be provided explicitly");
            }
            let mut branches = git_ops::list_branch_refs()?;

            // Slice branches are resolve targets only; don't allow selecting them as a source.
            branches.retain(|b| !planner::is_slice_branch(b));

            match tui::pick_branch(&branches, tui_title)? {
                Some(b) => b,
                None => bail!("merge source selection was canceled"),
            }
        }
    };

    // If an integration branch is selected, redirect to the original/source pair.
    let (actual_source_ref, actual_integration_branch) =
        if let Some((original, source)) = planner::parse_integration_branch(&source_ref) {
            println!("Note: '{source_ref}' is an integration branch.");
            println!("Redirecting: checking out '{original}' and merging '{source}' instead.\n");

            git_ops::checkout(&original)?;
            let new_current = git_ops::current_branch()?;
            let new_integration = planner::integration_branch_name(&new_current, &source);

            (source.clone(), new_integration)
        } else {
            let integration_branch = planner::integration_branch_name(current_branch, &source_ref);
            (source_ref.clone(), integration_branch)
        };

    let actual_source_sha = git_ops::resolve_commit(&actual_source_ref)?;
    let actual_remembered_head = git_ops::head_sha()?;

    if git_ops::branch_exists(&actual_integration_branch)? {
        git_ops::checkout(&actual_integration_branch)?;
        let slices = git_ops::list_slice_branches_for_integration(&actual_integration_branch)?;
        let status = git_ops::slice_merge_status(&actual_integration_branch, &slices)?;

        if !status.is_empty() {
            println!("Existing slice merge status for {actual_integration_branch}:");
            for (slice, merged) in &status {
                println!(
                    "  - {slice}: {}",
                    if *merged { "merged" } else { "pending" }
                );
            }
        }

        let all_merged = !status.is_empty() && status.values().all(|v| *v);
        if all_merged {
            let do_consolidate = if args.yes {
                true
            } else if args.quiet {
                println!(
                    "All slice branches are already merged. Skipping consolidation prompt due to --quiet (use --yes to auto-consolidate)."
                );
                false
            } else {
                tui::confirm(
                    "All slice branches are already merged. Create a non-destructive consolidated merge commit branch? (Enter/y=yes, n/Esc=no)",
                    tui_title,
                )?
            };

            if do_consolidate {
                let consolidated = git_ops::create_consolidated_merge_commit_branch(
                    &actual_integration_branch,
                    &actual_source_ref,
                    &status,
                )?;
                println!("Created consolidated branch: {consolidated}");
                println!(
                    "Integration branch was not rewritten. Review and promote explicitly if desired."
                );
            }
        }

        if !all_merged {
            println!("Integration branch already exists and has pending slice merges.");
            println!(
                "Resolve pending slices first, then re-run for consolidation or new operations."
            );
        }

        return Ok(());
    }

    git_ops::checkout_new_or_reset(&actual_integration_branch, &actual_remembered_head)?;
    git_ops::merge_no_commit(&actual_source_ref)?;

    let conflicted_files = git_ops::conflicted_files()?;
    for path in &conflicted_files {
        git_ops::restore_ours(path)?;
    }

    let auto_merged_files = git_ops::staged_files()?;

    let slice_plan = conflicted_files
        .iter()
        .enumerate()
        .map(|(i, file)| {
            let branch = planner::slice_branch_name(&actual_integration_branch, i + 1)?;
            Ok(SlicePlanItem {
                path: file.clone(),
                branch,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    if git_ops::merge_in_progress()? {
        let merged_section = if auto_merged_files.is_empty() {
            "* (none)".to_string()
        } else {
            auto_merged_files
                .iter()
                .map(|f| format!("* {f}"))
                .collect::<Vec<_>>()
                .join("\n")
        };

        let sliced_section = if slice_plan.is_empty() {
            "* (none)".to_string()
        } else {
            slice_plan
                .iter()
                .map(|s| format!("* {} -> {}", s.path, s.branch))
                .collect::<Vec<_>>()
                .join("\n")
        };

        let msg = format!(
            "Mergetopus: partial merge '{actual_source_ref}' into '{actual_integration_branch}' (conflicts sliced)\n\nmerged:\n{merged_section}\n\nsliced:\n{sliced_section}"
        );

        git_ops::commit(&msg)?;
    }

    let explicit_slices = select_conflicts(args, &actual_source_ref, &conflicted_files, tui_title)?;
    planner::create_slice_branches(
        &actual_integration_branch,
        &actual_remembered_head,
        &actual_source_ref,
        &actual_source_sha,
        &conflicted_files,
        &explicit_slices,
    )?;

    git_ops::checkout(&actual_integration_branch)?;
    println!("Mergetopus complete");
    println!("  Integration branch: {actual_integration_branch}");
    println!("  Source ref: {actual_source_ref} ({actual_source_sha})");
    println!("  Conflict count: {}", conflicted_files.len());
    println!("  Explicit slice groups: {}", explicit_slices.len());
    for (idx, group) in explicit_slices.iter().enumerate() {
        println!("  - SliceGroup {}: {} file(s)", idx + 1, group.len());
    }

    Ok(())
}

fn select_conflicts(
    args: &Args,
    source_ref: &str,
    all_conflicts: &[String],
    tui_title: &str,
) -> Result<Vec<Vec<String>>> {
    match args.select_paths.as_deref() {
        Some(csv) => {
            let paths = git_ops::select_conflicts_by_list(all_conflicts, csv)?;
            if paths.is_empty() {
                Ok(Vec::new())
            } else {
                Ok(vec![paths])
            }
        }
        None => {
            if args.quiet {
                Ok(Vec::new())
            } else {
                match tui::select_conflicts(
                    all_conflicts,
                    |path| git_ops::three_way_diff(path, source_ref),
                    tui_title,
                )? {
                    Some(groups) => Ok(groups),
                    None => bail!("conflict selection canceled"),
                }
            }
        }
    }
}

/// Resolve a merge conflict on a slice branch by invoking the user's merge tool.
///
/// For each file in the slice the function:
/// 1. Writes three temporary files:
///    - LOCAL  – the version from the remembered-head (ours, before any merge)
///    - BASE   – the version from the merge-base commit
///    - REMOTE – the version from the source commit (theirs)
/// 2. Sets LOCAL / BASE / REMOTE / MERGED as shell environment variables and
///    executes the command from `git config mergetool.<tool>.cmd`.
///    MERGED points to the actual working-tree file so the tool writes directly
///    into the repository.
/// 3. Stages the resolved file and, once all files are done, commits the result.
fn resolve_command(
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

    let commit_msg = git_ops::branch_tip_commit_message(&slice_branch)?;
    if !commit_msg.contains("Mergetopus slice:") {
        bail!(
            "'{}' does not appear to be a mergetopus slice branch \
             (missing 'Mergetopus slice:' in commit message)",
            slice_branch
        );
    }

    let source_commit = extract_trailer(&commit_msg, "Source-Commit")
        .ok_or_else(|| anyhow::anyhow!("missing Source-Commit trailer in slice commit"))?;
    let slice_paths = extract_slice_paths(&commit_msg);
    if slice_paths.is_empty() {
        bail!(
            "could not determine file paths for slice branch '{}'",
            slice_branch
        );
    }

    let remembered_head = git_ops::parent_sha(&slice_branch)?;
    let merge_base = git_ops::merge_base(&remembered_head, &source_commit)?;

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

    git_ops::checkout(&slice_branch)?;

    let tmp_dir = std::env::temp_dir().join(format!("mergetopus-{}", std::process::id()));
    std::fs::create_dir_all(&tmp_dir)
        .context("failed to create temporary directory for merge tool files")?;

    for path in &slice_paths {
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

        git_ops::write_blob_to_path(&remembered_head, path, &local_tmp)?;
        git_ops::write_blob_to_path(&merge_base, path, &base_tmp)?;
        git_ops::write_blob_to_path(&source_commit, path, &remote_tmp)?;

        let merged_before = std::fs::read(path).ok();
        let base_before = std::fs::read(&base_tmp)
            .with_context(|| format!("failed to read temporary BASE file for '{path}'"))?;

        println!("Resolving '{path}' with '{tool_name}'...");

        let status = if cfg!(target_os = "windows") {
            std::process::Command::new("cmd")
                .args(["/c", &tool_cmd])
                .env("LOCAL", &local_tmp)
                .env("BASE", &base_tmp)
                .env("REMOTE", &remote_tmp)
                .env("MERGED", path)
                .status()
                .with_context(|| format!("failed to launch merge tool '{tool_name}'"))?
        } else {
            std::process::Command::new("sh")
                .args(["-c", &tool_cmd])
                .env("LOCAL", &local_tmp)
                .env("BASE", &base_tmp)
                .env("REMOTE", &remote_tmp)
                .env("MERGED", path)
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

    let paths_list = slice_paths.join(", ");

    println!("Resolve complete on '{slice_branch}'");
    println!("  Resolved {} file(s): {}", slice_paths.len(), paths_list);

    if do_commit {
        if git_ops::staged_has_changes()? {
            let msg = format!(
                "Mergetopus resolve: '{slice_branch}'\n\nResolved-Paths: {paths_list}\nSource-Commit: {source_commit}"
            );
            git_ops::commit_strict(&msg)?;
            println!("  Resolution commit created.");
        } else {
            println!("  No staged changes to commit.");
        }
    } else {
        println!("  Changes are staged but not committed.");
        println!("  Review and commit when ready, or re-run with --commit.");
    }

    Ok(())
}

fn extract_trailer(message: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}: ");
    message
        .lines()
        .find(|line| line.starts_with(&prefix))
        .map(|line| line[prefix.len()..].trim().to_string())
}

fn extract_slice_paths(message: &str) -> Vec<String> {
    if let Some(val) = extract_trailer(message, "Slice-Paths") {
        let paths: Vec<String> = val
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        if !paths.is_empty() {
            return paths;
        }
    }

    message
        .lines()
        .filter(|line| line.starts_with("Source-Path: "))
        .map(|line| line["Source-Path: ".len()..].trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_trailer_finds_key() {
        let msg = "Mergetopus slice: 1 file(s) from 'main' (theirs)\n\nSource-Ref: origin/feature\nSource-Commit: abc123\nSource-Path: src/foo.rs\nSource-Path-Commit: def456\n";
        assert_eq!(
            extract_trailer(msg, "Source-Ref"),
            Some("origin/feature".to_string())
        );
        assert_eq!(
            extract_trailer(msg, "Source-Commit"),
            Some("abc123".to_string())
        );
        assert_eq!(extract_trailer(msg, "Missing-Key"), None);
    }

    #[test]
    fn extract_slice_paths_single_file() {
        let msg = "Mergetopus slice: 'src/foo.rs' from 'main' (theirs)\n\nSource-Ref: main\nSource-Commit: abc123\nSource-Path: src/foo.rs\nSource-Path-Commit: def456\n";
        let paths = extract_slice_paths(msg);
        assert_eq!(paths, vec!["src/foo.rs".to_string()]);
    }

    #[test]
    fn extract_slice_paths_explicit_multi_file() {
        let msg = "Mergetopus slice: 2 file(s) from 'main' (theirs)\n\nSource-Ref: main\nSource-Commit: abc123\nSlice-Paths: src/a.rs, src/b.rs\nSource-Path: src/a.rs\nSource-Path-Commit: x\nSource-Path: src/b.rs\nSource-Path-Commit: y\n";
        let paths = extract_slice_paths(msg);
        assert_eq!(paths, vec!["src/a.rs".to_string(), "src/b.rs".to_string()]);
    }

    #[test]
    fn extract_slice_paths_does_not_match_source_path_commit() {
        let msg = "Mergetopus slice: 'src/lib.rs' from 'feat' (theirs)\n\nSource-Ref: feat\nSource-Commit: 111\nSource-Path: src/lib.rs\nSource-Path-Commit: 222\n";
        let paths = extract_slice_paths(msg);
        assert_eq!(paths, vec!["src/lib.rs".to_string()]);
    }
}
