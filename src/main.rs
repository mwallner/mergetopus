mod git_ops;
mod models;
mod planner;
mod tui;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use crate::models::SlicePlanItem;

use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
struct LicenseInfo {
    license: String,
    text: String,
}

#[derive(Debug, Deserialize)]
struct Library {
    package_name: String,
    license: String,
    licenses: Vec<LicenseInfo>,
}

#[derive(Debug, Deserialize)]
struct Root {
    third_party_libraries: Vec<Library>,
}

include!(concat!(env!("OUT_DIR"), "/licenses_json.rs"));

fn parse_json(data: &str) -> Result<Root, serde_json::Error> {
    serde_json::from_str(data)
}

fn normalize_license(license: &str) -> String {
    let separators = [" OR ", "/"];
    let mut parts: Vec<&str> = Vec::new();

    for sep in &separators {
        if license.contains(sep) {
            parts = license.split(sep).collect();
            break;
        }
    }

    if parts.is_empty() {
        parts.push(license);
    }

    parts.sort();
    parts.join(" OR ")
}

pub fn license(full: bool, json_output: bool) {
    if json_output {
        println!("{}", JSON_LICENSE_DATA);
        return;
    }

    println!("Mergetopus is licensed under the {}", JSON_LICENSE_DATA);
    println!("------------------------------------------------");
    println!(" Mergetopus is built using the following crates: ");
    println!("------------------------------------------------");

    let root: Root = parse_json(JSON_LICENSE_DATA).expect("Failed to parse JSON");

    if full {
        // Print all packages with their full license text
        for library in root.third_party_libraries {
            println!("Package: {}", library.package_name);
            for license_info in library.licenses {
                println!("License: {}", license_info.license);
                println!("{}", license_info.text);
            }
            println!("------------------------------------------------");
        }
    } else {
        // Create a HashMap to group packages by license
        let mut license_map: HashMap<String, Vec<String>> = HashMap::new();

        // Populate the HashMap
        for library in root.third_party_libraries {
            let normalized_license = normalize_license(&library.license);
            license_map
                .entry(normalized_license)
                .or_insert_with(Vec::new)
                .push(library.package_name.clone());
        }

        // Print the licenses and their respective packages
        for (license, packages) in license_map {
            println!("License: {}", license);
            println!("Packages: {}", packages.join(", "));
            println!("------------------------------------------------");
        }
    }
}

#[derive(Parser, Debug)]
#[command(name = "mergetopus")]
#[command(about = "Split complex merges into integration + selectable conflict slice branches")]
#[command(
    long_about = "Mergetopus turns a regular git merge into an integration branch plus optional per-conflict slice branches.\n\nWorkflow:\n  1) Create/reset an integration branch from your current HEAD\n  2) Merge SOURCE into it with --no-commit\n  3) Keep auto-merged files in integration\n  4) Optionally group selected conflicted paths into one explicit slice branch via --select-paths\n\nIf SOURCE is omitted, an interactive branch picker is shown (unless --quiet is set)."
)]
#[command(
    after_help = "Examples:\n  mergetopus origin/main\n  mergetopus release/1.4 --select-paths 'src/a.rs,src/b.rs'\n  mergetopus hotfix --yes\n  mergetopus origin/main --quiet"
)]
struct Args {
    #[command(subcommand)]
    command: Option<Commands>,

    #[arg(
        value_name = "SOURCE",
        help = "Source branch/ref to merge into the integration branch",
        long_help = "Source branch/ref/commit-ish to merge.\n\nAccepted forms include local branches (feature/foo), remote-tracking refs (origin/main), tags, and commit SHAs.\nIf omitted, Mergetopus opens an interactive branch picker unless --quiet is set."
    )]
    source: Option<String>,

    #[arg(
        long,
        value_name = "CSV_PATHS",
        help = "Comma-separated conflicted file paths to group into one explicit slice",
        long_help = "Comma-separated list of conflicted file paths to include in a single explicit slice group.\n\nAny conflicted file not listed here is handled as a default one-file slice branch.\nExample: --select-paths 'src/lib.rs,src/main.rs,README.md'"
    )]
    select_paths: Option<String>,

    #[arg(
        long,
        default_value_t = false,
        help = "Run non-interactively and never open TUI screens",
        long_help = "Run in non-interactive mode suitable for CI/CD.\n\nBehavior changes:\n- SOURCE must be provided explicitly (no source picker)\n- Consolidation prompts are skipped unless --yes is provided\n- Conflict grouping comes only from --select-paths (no interactive conflict selector)"
    )]
    quiet: bool,

    #[arg(
        long,
        default_value_t = false,
        help = "Auto-confirm prompts when safe to proceed",
        long_help = "Assume 'yes' for non-destructive confirmation prompts.\n\nCurrently used when an existing integration branch already has all slices merged and Mergetopus asks whether to create a consolidated merge-commit branch."
    )]
    yes: bool,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Show bundled license data
    License {
        #[arg(long, default_value_t = false)]
        full: bool,
        #[arg(long, default_value_t = false)]
        json_output: bool,
    },
    /// Resolve a merge conflict on a slice branch using the configured merge tool.
    ///
    /// The command looks up the three versions of each conflicted file
    /// (LOCAL = ours/remembered-head, BASE = merge-base, REMOTE = theirs/source)
    /// and invokes the tool named by `git config merge.tool`, whose command
    /// template is taken from `git config mergetool.<tool>.cmd`.
    /// The variables LOCAL, BASE, REMOTE and MERGED are set as shell environment
    /// variables before the command is executed (same convention as git mergetool).
    Resolve {
        /// Slice branch to resolve.  When omitted an interactive TUI picker is shown.
        #[arg(value_name = "BRANCH")]
        branch: Option<String>,
    },
}

fn main() -> Result<()> {
    let args = Args::parse();

    if let Some(Commands::License { full, json_output }) = &args.command {
        license(*full, *json_output);
        return Ok(());
    }

    git_ops::ensure_git_context()?;
    let current_branch = git_ops::current_branch()?;
    let tui_title = format!("Mergetopus [{current_branch}]");

    if let Some(Commands::Resolve { branch }) = &args.command {
        return resolve_command(branch.as_deref(), args.quiet, &tui_title);
    }

    let source_ref = match args.source.as_ref() {
        Some(s) => s.clone(),
        None => {
            if args.quiet {
                bail!("--quiet requires SOURCE to be provided explicitly");
            }
            let branches = git_ops::list_branch_refs()?;
            match tui::pick_branch(&branches, &tui_title)? {
                Some(b) => b,
                None => bail!("merge source selection was canceled"),
            }
        }
    };

    let source_sha = git_ops::resolve_commit(&source_ref)?;
    let remembered_head = git_ops::head_sha()?;
    let integration_branch = planner::integration_branch_name(&current_branch, &source_ref);

    if git_ops::branch_exists(&integration_branch)? {
        git_ops::checkout(&integration_branch)?;
        let slices = git_ops::list_slice_branches_for_integration(&integration_branch)?;
        let status = git_ops::slice_merge_status(&integration_branch, &slices)?;

        if !status.is_empty() {
            println!("Existing slice merge status for {integration_branch}:");
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
                    &tui_title,
                )?
            };

            if do_consolidate {
                let consolidated = git_ops::create_consolidated_merge_commit_branch(
                    &integration_branch,
                    &source_ref,
                    &source_sha,
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

    git_ops::checkout_new_or_reset(&integration_branch, &remembered_head)?;
    git_ops::merge_no_commit(&source_ref)?;

    let conflicted_files = git_ops::conflicted_files()?;
    for path in &conflicted_files {
        git_ops::restore_ours(path)?;
    }

    let auto_merged_files = git_ops::staged_files()?;

    let slice_plan = conflicted_files
        .iter()
        .enumerate()
        .map(|(i, file)| {
            let branch = planner::slice_branch_name(&integration_branch, i + 1)?;
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
            "Mergetopus: partial merge '{source_ref}' into '{integration_branch}' (conflicts sliced)\n\nmerged:\n{merged_section}\n\nsliced:\n{sliced_section}"
        );

        git_ops::commit(&msg)?;
    }

    let explicit_slices = select_conflicts(&args, &source_ref, &conflicted_files, &tui_title)?;
    planner::create_slice_branches(
        &integration_branch,
        &remembered_head,
        &source_ref,
        &source_sha,
        &conflicted_files,
        &explicit_slices,
    )?;

    git_ops::checkout(&integration_branch)?;
    println!("Mergetopus complete");
    println!("  Integration branch: {integration_branch}");
    println!("  Source ref: {source_ref} ({source_sha})");
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

// ---------------------------------------------------------------------------
// resolve subcommand
// ---------------------------------------------------------------------------

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
fn resolve_command(branch_arg: Option<&str>, quiet: bool, tui_title: &str) -> Result<()> {
    // 1. Determine the slice branch to work on.
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

    // 2. Parse commit message for merge metadata.
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

    // 3. Derive the remembered-head (parent of the slice tip) and merge-base.
    let remembered_head = git_ops::parent_sha(&slice_branch)?;
    let merge_base = git_ops::merge_base(&remembered_head, &source_commit)?;

    // 4. Read the configured merge tool.
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

    // 5. Checkout the slice branch so the working tree holds the THEIRS version.
    git_ops::checkout(&slice_branch)?;

    // 6. Prepare a per-process temporary directory so parallel invocations and
    //    other users cannot read or tamper with the auxiliary merge files.
    let tmp_dir = std::env::temp_dir().join(format!("mergetopus-{}", std::process::id()));
    std::fs::create_dir_all(&tmp_dir)
        .context("failed to create temporary directory for merge tool files")?;

    // 7. For each file: populate temp files, invoke the tool, stage the result.
    for path in &slice_paths {
        // Replace characters that are problematic in file-system names.
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

        // LOCAL  = ours (remembered-head before the merge)
        git_ops::write_blob_to_path(&remembered_head, path, &local_tmp)?;
        // BASE   = common ancestor of both sides
        git_ops::write_blob_to_path(&merge_base, path, &base_tmp)?;
        // REMOTE = theirs (source commit)
        git_ops::write_blob_to_path(&source_commit, path, &remote_tmp)?;
        // MERGED = working-tree file (currently theirs; the tool writes the resolution here)

        let merged_before = std::fs::read(path).ok();
        let base_before = std::fs::read(&base_tmp)
            .with_context(|| format!("failed to read temporary BASE file for '{path}'"))?;

        println!("Resolving '{path}' with '{tool_name}'...");

        // Execute the merge tool.  LOCAL / BASE / REMOTE / MERGED are set as
        // shell variables matching git mergetool's own convention.
        let status = std::process::Command::new("sh")
            .args(["-c", &tool_cmd])
            .env("LOCAL", &local_tmp)
            .env("BASE", &base_tmp)
            .env("REMOTE", &remote_tmp)
            .env("MERGED", path)
            .status()
            .with_context(|| format!("failed to launch merge tool '{tool_name}'"))?;

        if !status.success() {
            eprintln!(
                "warning: merge tool exited with non-zero status for '{path}' \
                 (exit code: {}); the file has been staged as-is – \
                 please verify the resolution manually before committing",
                status.code().unwrap_or(-1)
            );
        }

        // Some three-way tools take only LOCAL/BASE/REMOTE and write the
        // merged output back into BASE. Mirror that result into MERGED.
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

    // 8. Leave commit control to the user.
    let paths_list = slice_paths.join(", ");

    println!("Resolve complete on '{slice_branch}'");
    println!("  Resolved {} file(s): {}", slice_paths.len(), paths_list);
    println!("  Changes are staged but not committed.");
    println!("  Review and commit when ready.");
    Ok(())
}

/// Extract the value of a single-line commit-message trailer (`Key: value`).
/// Returns the *first* occurrence of the key.
fn extract_trailer(message: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}: ");
    message
        .lines()
        .find(|line| line.starts_with(&prefix))
        .map(|line| line[prefix.len()..].trim().to_string())
}

/// Collect the list of file paths stored in a slice branch commit message.
///
/// For explicit multi-file slices the `Slice-Paths: p1, p2` trailer is used.
/// For single-file default slices the `Source-Path: <path>` trailer is used
/// (taking care not to match `Source-Path-Commit:`).
fn extract_slice_paths(message: &str) -> Vec<String> {
    // Prefer the compact Slice-Paths trailer written by explicit slices.
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

    // Fall back to individual Source-Path lines (single-file slices).
    // The prefix "Source-Path: " must be matched exactly to avoid matching
    // "Source-Path-Commit: " lines.
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
        // Ensure "Source-Path-Commit: ..." is NOT interpreted as a path.
        let msg = "Mergetopus slice: 'src/lib.rs' from 'feat' (theirs)\n\nSource-Ref: feat\nSource-Commit: 111\nSource-Path: src/lib.rs\nSource-Path-Commit: 222\n";
        let paths = extract_slice_paths(msg);
        assert_eq!(paths, vec!["src/lib.rs".to_string()]);
    }
}
