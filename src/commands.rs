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

    if let Some(Commands::Status { source }) = &args.command {
        git_ops::ensure_git_worktree()?;
        let current_branch = git_ops::current_branch()?;
        let tui_title = format!("Mergetopus [{current_branch}]");
        return status_command(source.as_deref(), args.quiet, &current_branch, &tui_title);
    }

    if let Some(Commands::Cleanup) = &args.command {
        git_ops::ensure_git_worktree()?;
        let current_branch = git_ops::current_branch()?;
        let tui_title = format!("Mergetopus [{current_branch}]");
        return cleanup_command(args.quiet, &current_branch, &tui_title);
    }

    git_ops::ensure_git_context()?;
    let current_branch = git_ops::current_branch()?;
    let tui_title = format!("Mergetopus [{current_branch}]");

    run_merge_workflow(&args, &current_branch, &tui_title)
}

fn status_command(
    source_arg: Option<&str>,
    quiet: bool,
    current_branch: &str,
    tui_title: &str,
) -> Result<()> {
    let integration_branch =
        resolve_status_integration_branch(source_arg, quiet, current_branch, tui_title)?;

    // If a kokomeco consolidated branch already exists for this integration
    // branch, show the merge suggestion instead of the raw integration status.
    let kokomeco = git_ops::consolidated_branch_name(&integration_branch);
    if git_ops::branch_exists(&kokomeco)? {
        println!("Mergetopus status");
        println!("  Integration branch:  {integration_branch}");
        println!("  Consolidated branch: {kokomeco}");
        println!();
        println!(
            "All slices are resolved. The kokomeco branch is ready to merge into '{current_branch}'."
        );
        println!();
        println!("Suggested next command:");
        println!("  git merge {kokomeco}");
        println!();
        println!("To clean up slice and integration branches afterward:");
        println!("  mergetopus cleanup");
        return Ok(());
    }

    let initial_commit = git_ops::first_parent_oldest_commit(&integration_branch)?;
    let initial_message = git_ops::commit_message(&initial_commit)?;
    let parents = git_ops::commit_parent_shas(&initial_commit)?;

    let source_sha = parents
        .get(1)
        .cloned()
        .unwrap_or_else(|| "(unknown)".to_string());
    let source_ref =
        parse_partial_merge_source_ref(&initial_message).unwrap_or_else(|| "(unknown)".to_string());

    let slices = git_ops::list_slice_branches_for_integration(&integration_branch)?;
    let status = git_ops::slice_merge_status(&integration_branch, &slices)?;

    let merged = status.values().filter(|v| **v).count();
    let pending = status.values().filter(|v| !**v).count();

    println!("Mergetopus status");
    println!("  Integration branch: {integration_branch}");
    println!("  Source ref: {source_ref}");
    println!("  Source SHA: {source_sha}");
    println!("  Total slices: {}", slices.len());
    println!("  Merged slices: {merged}");
    println!("  Pending slices: {pending}");

    if pending > 0 {
        println!("\nPending slice details:");
        for slice in &slices {
            let is_merged = status.get(slice).copied().unwrap_or(false);
            if is_merged {
                continue;
            }

            let tip_msg = git_ops::branch_tip_commit_message(slice)?;
            let mut paths = extract_slice_paths(&tip_msg);
            let resolve_tip = tip_msg
                .lines()
                .next()
                .unwrap_or("")
                .contains("Mergetopus resolve:");
            if paths.is_empty() && resolve_tip {
                let parent = git_ops::parent_sha(slice)?;
                let parent_msg = git_ops::commit_message(&parent)?;
                paths = extract_slice_paths(&parent_msg);
            }

            println!(
                "  - {slice}: {}",
                if resolve_tip {
                    "resolved (not merged)"
                } else {
                    "pending resolution"
                }
            );

            if paths.is_empty() {
                println!("    detected paths: (unknown)");
            } else {
                println!("    detected paths: {}", paths.join(", "));
            }
        }
    }

    println!("\nSuggested next command(s):");
    if pending == 0 {
        if slices.is_empty() {
            println!("  - No slice branches were found for this integration branch.");
        } else if source_ref == "(unknown)" {
            println!("  - mergetopus <source> --yes");
        } else {
            println!("  - mergetopus {source_ref} --yes");
        }
    } else {
        let first_pending = slices
            .iter()
            .find(|s| !status.get(*s).copied().unwrap_or(false));
        if let Some(slice) = first_pending {
            println!("  - mergetopus resolve {slice}");
            println!("  - git checkout {integration_branch} && git merge --no-ff {slice}");
        }
    }

    Ok(())
}

fn resolve_status_integration_branch(
    source_arg: Option<&str>,
    quiet: bool,
    current_branch: &str,
    tui_title: &str,
) -> Result<String> {
    if let Some(source) = source_arg {
        let target = if planner::parse_integration_branch(source).is_some() {
            source.to_string()
        } else if let Some((_, current_source)) = planner::parse_integration_branch(current_branch)
        {
            if current_source == planner::sanitize_branch_fragment(source) {
                current_branch.to_string()
            } else {
                planner::integration_branch_name(current_branch, source)
            }
        } else {
            planner::integration_branch_name(current_branch, source)
        };

        if !git_ops::branch_exists(&target)? {
            bail!(
                "could not find integration branch '{}'; provide an existing integration branch or source ref",
                target
            );
        }

        return Ok(target);
    }

    if planner::parse_integration_branch(current_branch).is_some() {
        return Ok(current_branch.to_string());
    }

    let prefix = planner::integration_branch_family_prefix(current_branch);
    let mut candidates = git_ops::list_local_branches()?
        .into_iter()
        .filter(|b| b.starts_with(&prefix))
        .filter(|b| planner::parse_integration_branch(b).is_some())
        .collect::<Vec<_>>();

    match candidates.len() {
        0 => bail!(
            "no integration branches found for current branch '{}'; provide SOURCE explicitly, e.g. 'mergetopus status <source>'",
            current_branch
        ),
        1 => Ok(candidates.remove(0)),
        _ => {
            if quiet {
                bail!(
                    "multiple integration branches found; provide SOURCE explicitly in --quiet mode"
                );
            }
            match tui::pick_branch(&candidates, tui_title)? {
                Some(b) => Ok(b),
                None => bail!("status branch selection was canceled"),
            }
        }
    }
}

fn parse_partial_merge_source_ref(message: &str) -> Option<String> {
    let first = message.lines().next()?.trim();
    let prefix = "Mergetopus: partial merge '";
    let rest = first.strip_prefix(prefix)?;
    let end = rest.find("' into '")?;
    let source = &rest[..end];
    if source.is_empty() {
        None
    } else {
        Some(source.to_string())
    }
}

fn normalize_merge_source_ref(source_ref: &str) -> Result<String> {
    let trimmed = source_ref.trim_start_matches('/');

    if !git_ops::remote_branch_exists(trimmed)? {
        return Ok(trimmed.to_string());
    }

    let Some((remote_name, local_candidate)) = trimmed.split_once('/') else {
        return Ok(trimmed.to_string());
    };

    if local_candidate.is_empty() {
        bail!(
            "selected source '{}' is a remote ref with no local branch name; choose a concrete branch",
            source_ref
        );
    }

    if git_ops::branch_exists(local_candidate)? {
        // Local branch exists: check if it's in sync with the remote.
        let local_sha = git_ops::resolve_ref(local_candidate)?;
        let remote_sha = git_ops::resolve_ref(trimmed)?;

        if local_sha == remote_sha {
            // Local is in sync with remote; use it.
            println!("Using existing local branch '{local_candidate}' (in sync with '{trimmed}').");
            return Ok(local_candidate.to_string());
        } else {
            // Local and remote diverge.
            bail!(
                "selected source '{}' maps to local branch '{}' which has diverged from its remote counterpart; \
                 local is at {} while remote is at {}. Synchronize or use a different branch.",
                source_ref,
                local_candidate,
                &local_sha[..8.min(local_sha.len())],
                &remote_sha[..8.min(remote_sha.len())]
            );
        }
    }

    git_ops::create_tracking_branch(local_candidate, trimmed)?;
    println!("Using remote source '{trimmed}' via new local tracking branch '{local_candidate}'");
    println!(
        "Tip: remote name '{remote_name}' is omitted for this merge context (source = '{local_candidate}')."
    );

    Ok(local_candidate.to_string())
}

fn run_merge_workflow(args: &Args, current_branch: &str, tui_title: &str) -> Result<()> {
    let selected_source_ref = match args.source.as_ref() {
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
    let source_ref = normalize_merge_source_ref(&selected_source_ref)?;

    // If an integration branch is selected, redirect to the original/source pair.
    let (actual_source_ref, actual_integration_branch, target_branch_for_merge) =
        if let Some((original, source)) = planner::parse_integration_branch(&source_ref) {
            println!("Note: '{source_ref}' is an integration branch.");
            println!("Redirecting: checking out '{original}' and merging '{source}' instead.\n");

            git_ops::checkout(&original)?;
            let new_current = git_ops::current_branch()?;
            let new_integration = planner::integration_branch_name(&new_current, &source);

            (source.clone(), new_integration, new_current)
        } else {
            let integration_branch = planner::integration_branch_name(current_branch, &source_ref);
            (
                source_ref.clone(),
                integration_branch,
                current_branch.to_string(),
            )
        };

    let kokomeco_branch = git_ops::consolidated_branch_name(&actual_integration_branch);
    if git_ops::branch_exists(&kokomeco_branch)? {
        println!("Kokomeco branch already exists for this merge context: {kokomeco_branch}");
        println!("To merge it back into your current target branch:");
        println!("  git checkout {target_branch_for_merge}");
        println!("  git merge --no-ff {kokomeco_branch}");
        println!("After promotion, delete it manually when no longer needed:");
        println!("  git branch -d {kokomeco_branch}");
        return Ok(());
    }

    let actual_source_sha = git_ops::resolve_commit(&actual_source_ref)?;
    let actual_remembered_head = git_ops::head_sha()?;
    let actual_merge_base = git_ops::merge_base(&actual_remembered_head, &actual_source_sha)
        .with_context(|| {
            "failed before entering conflict resolution: could not compute merge-base between current HEAD and source; verify source/history compatibility, then retry (for unrelated histories, merge manually with --allow-unrelated-histories first)"
        })?;

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
                    "All slice branches are already merged. Skipping kokomeco prompt due to --quiet (use --yes to auto-create the kokomeco branch)."
                );
                false
            } else {
                tui::confirm(
                    "All slice branches are already merged. Create a non-destructive kokomeco merge commit branch? (Enter/y=yes, n/Esc=no)",
                    tui_title,
                )?
            };

            if do_consolidate {
                let consolidated = git_ops::create_consolidated_merge_commit_branch(
                    &actual_integration_branch,
                    &actual_source_ref,
                    &status,
                )?;
                println!("Created kokomeco branch: {consolidated}");
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

    let explicit_slices = match select_conflicts(
        args,
        &actual_source_ref,
        &conflicted_files,
        tui_title,
    ) {
        Ok(slices) => slices,
        Err(e) => {
            // Clean up on cancellation: checkout target branch and delete integration branch.
            if let Err(checkout_err) = git_ops::checkout(&target_branch_for_merge) {
                eprintln!(
                    "Warning: failed to checkout '{target_branch_for_merge}' during cleanup: {checkout_err}"
                );
            }
            if let Err(delete_err) = git_ops::delete_branch(&actual_integration_branch) {
                eprintln!(
                    "Warning: failed to delete integration branch '{actual_integration_branch}' during cleanup: {delete_err}"
                );
            }
            return Err(e).context("conflict selection canceled; integration branch cleaned up");
        }
    };
    planner::create_slice_branches(
        &actual_integration_branch,
        &actual_merge_base,
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
                let diff_tool = git_ops::get_git_config("diff.tool")?;
                match tui::select_conflicts(
                    all_conflicts,
                    |path| git_ops::three_way_diff(path, source_ref),
                    diff_tool.as_deref(),
                    |path| git_ops::launch_difftool(path, source_ref),
                    tui_title,
                )? {
                    Some(groups) => Ok(groups),
                    None => bail!("conflict selection canceled"),
                }
            }
        }
    }
}

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

/// Parse a command string into program and arguments, handling quoted tokens.
/// Splits on unquoted whitespace to handle paths and arguments with special characters.
fn parse_command_string(cmd: &str) -> Result<(String, Vec<String>)> {
    let mut tokens = Vec::new();
    let mut current_token = String::new();
    let mut in_double_quotes = false;
    let mut in_single_quotes = false;

    for ch in cmd.chars() {
        match ch {
            '"' if !in_single_quotes => {
                in_double_quotes = !in_double_quotes;
            }
            '\'' if !in_double_quotes => {
                in_single_quotes = !in_single_quotes;
            }
            ' ' | '\t' if !in_double_quotes && !in_single_quotes => {
                if !current_token.is_empty() {
                    tokens.push(current_token.clone());
                    current_token.clear();
                }
            }
            _ => current_token.push(ch),
        }
    }

    if !current_token.is_empty() {
        tokens.push(current_token);
    }

    if tokens.is_empty() {
        bail!("empty merge tool command");
    }

    let program = tokens.remove(0);
    Ok((program, tokens))
}

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
            let (program, args) = parse_command_string(&expanded_cmd)?;
            std::process::Command::new(&program)
                .args(&args)
                .status()
                .with_context(|| {
                    format!("failed to launch merge tool '{tool_name}' (program: '{program}')")
                })?
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

fn cleanup_command(quiet: bool, current_branch: &str, tui_title: &str) -> Result<()> {
    let all_local = git_ops::list_local_branches()?;

    let mut branches_to_delete: Vec<String> = Vec::new();

    for branch in &all_local {
        if planner::parse_integration_branch(branch).is_none() {
            continue;
        }

        let kokomeco = git_ops::consolidated_branch_name(branch);
        if !git_ops::branch_exists(&kokomeco)? {
            continue;
        }

        branches_to_delete.push(branch.clone());

        let slices = git_ops::list_slice_branches_for_integration(branch)?;
        branches_to_delete.extend(slices);
    }

    if branches_to_delete.is_empty() {
        println!(
            "Nothing to clean up: no integration branches with a corresponding kokomeco branch found."
        );
        return Ok(());
    }

    branches_to_delete.sort();
    branches_to_delete.dedup();

    let do_delete = if quiet {
        bail!("cleanup requires interactive confirmation; re-run without --quiet to proceed");
    } else {
        tui::confirm_list(
            &branches_to_delete,
            &format!(
                "Delete {} branch(es)? The kokomeco branch is retained. This cannot be undone.",
                branches_to_delete.len()
            ),
            tui_title,
        )?
    };

    if !do_delete {
        println!("Cleanup canceled.");
        return Ok(());
    }

    let mut deleted = 0usize;
    for branch in &branches_to_delete {
        if branch == current_branch {
            eprintln!("Skipping '{branch}': cannot delete the currently checked-out branch.");
            continue;
        }
        git_ops::delete_branch(branch)?;
        println!("Deleted: {branch}");
        deleted += 1;
    }

    println!("\nCleaned up {deleted} branch(es).");
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

    #[test]
    fn parse_partial_merge_source_ref_works() {
        let msg = "Mergetopus: partial merge 'origin/feature-x' into '_mmm/main/origin_feature-x/integration' (conflicts sliced)\n\nmerged:\n* a";
        assert_eq!(
            parse_partial_merge_source_ref(msg),
            Some("origin/feature-x".to_string())
        );
    }

    #[test]
    fn parse_partial_merge_source_ref_rejects_non_matching() {
        assert_eq!(parse_partial_merge_source_ref("hello"), None);
    }
}
