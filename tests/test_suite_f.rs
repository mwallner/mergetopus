//! Suite F: integration tests for rename/delete conflict topologies.
//!
//! These cover the edge cases that previously aborted the workflow
//! (`git restore --source=HEAD` on paths without an ours stage) and verify
//! the rename-aware behavior: correlated paths are sliced as one logical
//! group, deletion-aware slicing round-trips, and silently auto-applied
//! deletions/renames are reported.

use std::fs;
type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

mod test_helpers;

fn integration_branch() -> &'static str {
    "_mmm/main/feature/integration"
}

fn slice_branch(n: usize) -> String {
    format!("_mmm/main/feature/slice{n}")
}

fn assert_ok(result: &std::process::Output, ctx: &str) {
    assert!(
        result.status.success(),
        "{ctx} failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

fn tree_paths(repo: &std::path::Path, reference: &str) -> TestResult<Vec<String>> {
    let out = test_helpers::git(repo, &["ls-tree", "-r", "--name-only", reference])?;
    Ok(out.lines().map(str::to_string).collect())
}

fn branch_exists(repo: &std::path::Path, branch: &str) -> TestResult<bool> {
    let out = test_helpers::git(
        repo,
        &["branch", "--list", branch, "--format=%(refname:short)"],
    )?;
    Ok(!out.trim().is_empty())
}

// ── GAP 1: restore_ours no longer aborts on ours-absent paths ───────────────

/// Rename/rename (1→2): both sides rename the same file to different names.
/// Previously this aborted the workflow in the restore_ours loop; now it must
/// complete, keeping all three correlated paths in ONE slice (unit of
/// assignment, not three unrelated files).
#[test]
fn rename_rename_conflict_slices_as_one_group() -> TestResult<()> {
    let repo = test_helpers::init_repo()?;
    test_helpers::write_file(&repo, "src/a.txt", "a\n")?;
    test_helpers::commit_all(&repo, "base")?;

    test_helpers::git(&repo, &["checkout", "-b", "feature"])?;
    test_helpers::git(&repo, &["mv", "src/a.txt", "renamed_feature.txt"])?;
    test_helpers::git(&repo, &["commit", "-m", "feature renames a"])?;

    test_helpers::git(&repo, &["checkout", "main"])?;
    test_helpers::git(&repo, &["mv", "src/a.txt", "renamed_main.txt"])?;
    test_helpers::git(&repo, &["commit", "-m", "main renames a differently"])?;

    let out = test_helpers::mergetopus(&repo, &["feature", "--quiet"])?;
    assert_ok(&out, "mergetopus (rename/rename)");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Conflict count: 3"),
        "all three index paths are conflicted:\n{stdout}"
    );

    // One slice covering the whole group, not three.
    for n in 2..4 {
        assert!(
            !branch_exists(&repo, &slice_branch(n))?,
            "slice{n} must not exist; the rename/rename group is one slice"
        );
    }
    let slice1 = slice_branch(1);
    assert!(
        branch_exists(&repo, &slice1)?,
        "slice1 (the rename group) must exist"
    );

    // The slice carries theirs: content added at the feature-side name and
    // the old path removed relative to the merge base.
    let tree = tree_paths(&repo, &slice1)?;
    assert!(
        tree.iter().any(|p| p == "renamed_feature.txt"),
        "slice must carry theirs content at renamed_feature.txt:\n{tree:?}"
    );
    assert!(
        !tree.iter().any(|p| p == "src/a.txt"),
        "slice must express the rename (old path removed):\n{tree:?}"
    );

    // Integration kept OURS: the main-side name, no theirs path yet.
    let integration = tree_paths(&repo, integration_branch())?;
    assert!(
        integration.iter().any(|p| p == "renamed_main.txt"),
        "integration must keep ours rename target:\n{integration:?}"
    );
    assert!(
        !integration.iter().any(|p| p == "renamed_feature.txt"),
        "theirs side belongs to the slice, not integration:\n{integration:?}"
    );
    Ok(())
}

/// Delete/modify where the TARGET deleted the file: the unmerged path has no
/// ours stage, which previously aborted restore_ours. Now the workflow
/// completes and the slice carries theirs (the modified content).
#[test]
fn deleted_by_target_conflict_slices_theirs_content() -> TestResult<()> {
    let repo = test_helpers::init_repo()?;
    test_helpers::write_file(&repo, "f.txt", "base\n")?;
    test_helpers::commit_all(&repo, "base")?;

    test_helpers::git(&repo, &["checkout", "-b", "feature"])?;
    test_helpers::write_file(&repo, "f.txt", "feature modified\n")?;
    test_helpers::git(&repo, &["commit", "-am", "feature modifies f"])?;

    test_helpers::git(&repo, &["checkout", "main"])?;
    test_helpers::git(&repo, &["rm", "-q", "f.txt"])?;
    test_helpers::git(&repo, &["commit", "-m", "main deletes f"])?;

    let out = test_helpers::mergetopus(&repo, &["feature", "--quiet"])?;
    assert_ok(&out, "mergetopus (deleted by target)");

    let slice1 = slice_branch(1);
    assert!(branch_exists(&repo, &slice1)?, "slice1 must exist");
    let content = test_helpers::git(&repo, &["show", &format!("{slice1}:f.txt")])?;
    assert_eq!(content, "feature modified");

    // Integration restored "ours" = the deletion (path absent).
    let integration = tree_paths(&repo, integration_branch())?;
    assert!(
        !integration.iter().any(|p| p == "f.txt"),
        "ours-side deletion must be kept on integration:\n{integration:?}"
    );
    Ok(())
}

// ── GAP 3: directory-rename file-location conflicts ─────────────────────────

/// Source renames a directory; target adds a new file under the old path.
/// Git reports "CONFLICT (file location)" with the new file staged at the
/// suggested location (a path HEAD does not contain), which previously
/// aborted the workflow AND — via the planner's `git rm` fallback — would
/// have dropped the user's file entirely. The slice must now carry the added
/// content at BOTH candidate locations so resolution picks one, never lose it.
#[test]
fn directory_rename_location_conflict_preserves_added_file() -> TestResult<()> {
    let repo = test_helpers::init_repo()?;
    fs::create_dir_all(repo.join("src"))?;
    test_helpers::write_file(&repo, "src/a.txt", "a\n")?;
    test_helpers::write_file(&repo, "src/b.txt", "b\n")?;
    test_helpers::write_file(&repo, "src/c.txt", "c\n")?;
    test_helpers::commit_all(&repo, "base")?;

    test_helpers::git(&repo, &["checkout", "-b", "feature"])?;
    test_helpers::git(&repo, &["mv", "src", "dst"])?;
    test_helpers::git(&repo, &["commit", "-m", "feature renames src->dst"])?;

    test_helpers::git(&repo, &["checkout", "main"])?;
    test_helpers::write_file(&repo, "src/new.txt", "new file added by target\n")?;
    test_helpers::commit_all(&repo, "main adds src/new.txt")?;

    let out = test_helpers::mergetopus(&repo, &["feature", "--quiet"])?;
    assert_ok(&out, "mergetopus (dir-rename file location)");

    let slice1 = slice_branch(1);
    assert!(branch_exists(&repo, &slice1)?, "slice1 must exist");
    let tree = tree_paths(&repo, &slice1)?;
    for expected in ["src/new.txt", "dst/new.txt"] {
        assert!(
            tree.iter().any(|p| p == expected),
            "slice must offer the added file at {expected}:\n{tree:?}"
        );
    }
    let at_src = test_helpers::git(&repo, &["show", &format!("{slice1}:src/new.txt")])?;
    assert_eq!(at_src, "new file added by target");
    Ok(())
}

// ── Positive control (still works) ──────────────────────────────────────────

/// Modify/delete where the SOURCE deletes works end-to-end: the slice branch
/// carries the deletion, and integration keeps the modified-ours version
/// until the slice is resolved.
#[test]
fn deleted_by_source_conflict_yields_deletion_slice() -> TestResult<()> {
    let repo = test_helpers::init_repo()?;
    test_helpers::write_file(&repo, "f.txt", "base\n")?;
    test_helpers::commit_all(&repo, "base")?;

    test_helpers::git(&repo, &["checkout", "-b", "feature"])?;
    test_helpers::git(&repo, &["rm", "-q", "f.txt"])?;
    test_helpers::git(&repo, &["commit", "-m", "feature deletes f"])?;

    test_helpers::git(&repo, &["checkout", "main"])?;
    test_helpers::write_file(&repo, "f.txt", "main modified\n")?;
    test_helpers::git(&repo, &["commit", "-am", "main modifies f"])?;

    let out = test_helpers::mergetopus(&repo, &["feature", "--quiet"])?;
    assert_ok(&out, "mergetopus (source-deleted modify/delete)");

    let slice_tree = tree_paths(&repo, &slice_branch(1))?;
    assert!(
        !slice_tree.iter().any(|p| p == "f.txt"),
        "slice must carry the deletion:\n{slice_tree:?}"
    );
    let integration = tree_paths(&repo, integration_branch())?;
    assert!(
        integration.iter().any(|p| p == "f.txt"),
        "integration must still contain f.txt (ours restored):\n{integration:?}"
    );
    Ok(())
}

// ── Group-aware resolve: --on-group modes settle whole groups ───────────────

fn setup_rename_rename_workflow(repo: &std::path::Path) -> TestResult<()> {
    test_helpers::write_file(repo, "src/a.txt", "a\n")?;
    test_helpers::commit_all(repo, "base")?;

    test_helpers::git(repo, &["checkout", "-b", "feature"])?;
    test_helpers::git(repo, &["mv", "src/a.txt", "renamed_feature.txt"])?;
    test_helpers::git(repo, &["commit", "-m", "feature renames a"])?;

    test_helpers::git(repo, &["checkout", "main"])?;
    test_helpers::git(repo, &["mv", "src/a.txt", "renamed_main.txt"])?;
    test_helpers::git(repo, &["commit", "-m", "main renames a differently"])?;

    let out = test_helpers::mergetopus(repo, &["feature", "--quiet"])?;
    assert_ok(&out, "mergetopus setup (rename/rename)");
    Ok(())
}

#[test]
fn resolve_quiet_theirs_applies_the_slice_rename() -> TestResult<()> {
    let repo = test_helpers::init_repo()?;
    setup_rename_rename_workflow(&repo)?;

    let resolve =
        test_helpers::mergetopus(&repo, &["--quiet", "resolve", &slice_branch(1), "--commit"])?;
    assert_ok(&resolve, "quiet resolve (default theirs)");
    let stdout = String::from_utf8_lossy(&resolve.stdout);
    assert!(
        stdout.contains("Settled") && stdout.contains("rename/rename"),
        "group must be settled in one decision:\n{stdout}"
    );

    let integration = tree_paths(&repo, integration_branch())?;
    assert!(integration.iter().any(|p| p == "renamed_feature.txt"));
    assert!(
        !integration.iter().any(|p| p == "renamed_main.txt"),
        "theirs mode drops the target-side rename:\n{integration:?}",
    );
    assert!(
        !integration.iter().any(|p| p == "src/a.txt"),
        "old path must be gone:\n{integration:?}",
    );
    // Merge commit exists (resolved) and no merge is in progress.
    let head_msg = test_helpers::git(&repo, &["log", "-1", "--format=%s", integration_branch()])?;
    assert!(
        head_msg.contains("Mergetopus resolve"),
        "expected resolve merge commit, got: {head_msg}"
    );
    Ok(())
}

/// Resuming an in-progress slice merge must still settle rename groups:
/// the original CONFLICT lines are unavailable, so resolve reconstructs the
/// topology from the merge sides instead of treating every path as content.
#[test]
fn resolve_reconstructs_topology_when_resuming_merge() -> TestResult<()> {
    let repo = test_helpers::init_repo()?;
    setup_rename_rename_workflow(&repo)?;

    test_helpers::git(&repo, &["checkout", integration_branch()])?;
    test_helpers::git(&repo, &["merge", "--no-commit", &slice_branch(1)]).ok();
    assert!(
        test_helpers::git(&repo, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).is_ok(),
        "expected an in-progress merge to resume"
    );

    let resolve =
        test_helpers::mergetopus(&repo, &["--quiet", "resolve", &slice_branch(1), "--commit"])?;
    assert_ok(&resolve, "resumed quiet resolve (default theirs)");
    let stdout = String::from_utf8_lossy(&resolve.stdout);
    assert!(
        stdout.contains("Settled") && stdout.contains("rename/rename"),
        "group must be settled via reconstructed topology, not a merge tool:\n{stdout}"
    );

    let integration = tree_paths(&repo, integration_branch())?;
    assert!(integration.iter().any(|p| p == "renamed_feature.txt"));
    assert!(
        !integration.iter().any(|p| p == "src/a.txt"),
        "old path must be gone:\n{integration:?}",
    );
    Ok(())
}

#[test]
fn resolve_on_group_both_keeps_all_names() -> TestResult<()> {
    let repo = test_helpers::init_repo()?;
    setup_rename_rename_workflow(&repo)?;

    let resolve = test_helpers::mergetopus(
        &repo,
        &[
            "--quiet",
            "resolve",
            &slice_branch(1),
            "--on-group",
            "both",
            "--commit",
        ],
    )?;
    assert_ok(&resolve, "quiet resolve --on-group both");

    let integration = tree_paths(&repo, integration_branch())?;
    assert!(integration.iter().any(|p| p == "renamed_feature.txt"));
    assert!(integration.iter().any(|p| p == "renamed_main.txt"));
    assert!(
        !integration.iter().any(|p| p == "src/a.txt"),
        "old path must be dropped even in both mode:\n{integration:?}",
    );
    Ok(())
}

#[test]
fn resolve_content_conflicts_ignore_on_group() -> TestResult<()> {
    // Plain content conflicts are NOT group-decided; --on-group must not
    // interfere and the merge tool keeps handling them.
    let repo = test_helpers::setup_single_conflict_repo()?;
    let create = test_helpers::mergetopus(&repo, &["feature", "--quiet"])?;
    assert_ok(&create, "mergetopus setup");

    test_helpers::git(&repo, &["config", "merge.tool", "copybase"])?;
    test_helpers::git(
        &repo,
        &[
            "config",
            "mergetool.copybase.cmd",
            "cp \"$BASE\" \"$MERGED\"",
        ],
    )?;
    test_helpers::git(&repo, &["config", "mergetool.trustExitCode", "true"])?;

    let resolve = test_helpers::mergetopus(
        &repo,
        &[
            "--quiet",
            "resolve",
            &slice_branch(1),
            "--on-group",
            "delete",
            "--commit",
        ],
    )?;
    assert_ok(
        &resolve,
        "content conflict must go through the tool untouched",
    );
    let content = test_helpers::git(
        &repo,
        &["show", &format!("{}:conflict.txt", integration_branch())],
    )?;
    assert_eq!(content, "base", "copybase resolved the content conflict");
    Ok(())
}

#[test]
fn resolve_modify_delete_group_modes() -> TestResult<()> {
    // feature deletes f (slice = deletion), main modifies f (integration keeps
    // ours pending resolution): merging the slice back re-conflicts as
    // modify/delete and the group mode decides the outcome.
    let make = || -> TestResult<std::path::PathBuf> {
        let repo = test_helpers::init_repo()?;
        test_helpers::write_file(&repo, "f.txt", "base\n")?;
        test_helpers::commit_all(&repo, "base")?;

        test_helpers::git(&repo, &["checkout", "-b", "feature"])?;
        test_helpers::git(&repo, &["rm", "-q", "f.txt"])?;
        test_helpers::git(&repo, &["commit", "-m", "feature deletes f"])?;

        test_helpers::git(&repo, &["checkout", "main"])?;
        test_helpers::write_file(&repo, "f.txt", "main version\n")?;
        test_helpers::git(&repo, &["commit", "-am", "main modifies f"])?;

        let out = test_helpers::mergetopus(&repo, &["feature", "--quiet"])?;
        assert_ok(&out, "mergetopus setup (modify/delete)");
        Ok(repo)
    };

    // Default (quiet) = theirs = accept the slice's deletion.
    let repo = make()?;
    let resolve =
        test_helpers::mergetopus(&repo, &["--quiet", "resolve", &slice_branch(1), "--commit"])?;
    assert_ok(&resolve, "quiet resolve modify/delete (theirs)");
    let integration = tree_paths(&repo, integration_branch())?;
    assert!(
        !integration.iter().any(|p| p == "f.txt"),
        "theirs must accept the deletion:\n{integration:?}",
    );

    // ours = keep the modified content.
    let repo = make()?;
    let resolve = test_helpers::mergetopus(
        &repo,
        &[
            "--quiet",
            "resolve",
            &slice_branch(1),
            "--on-group",
            "ours",
            "--commit",
        ],
    )?;
    assert_ok(&resolve, "quiet resolve modify/delete (ours)");
    let ib = integration_branch();
    let content = test_helpers::git(&repo, &["show", &format!("{ib}:f.txt")])?;
    assert_eq!(content, "main version");
    Ok(())
}

// ── GAP 4: silent auto-applied decisions are now reported ───────────────────

/// Both sides delete the same file (each with prior modifications): ort
/// auto-resolves to deletion with NO conflict entry. The workflow must
/// report the applied source-side deletion, record it in the partial-merge
/// commit message, and surface it via status for auditing.
#[test]
fn both_deleted_file_is_reported_as_auto_applied() -> TestResult<()> {
    let repo = test_helpers::init_repo()?;
    test_helpers::write_file(&repo, "gone.txt", "base\n")?;
    test_helpers::write_file(&repo, "keep.txt", "keep\n")?;
    test_helpers::commit_all(&repo, "base")?;

    test_helpers::git(&repo, &["checkout", "-b", "feature"])?;
    test_helpers::write_file(&repo, "gone.txt", "feature edit\n")?;
    test_helpers::git(&repo, &["commit", "-am", "feature edits gone"])?;
    test_helpers::git(&repo, &["rm", "-q", "gone.txt"])?;
    test_helpers::git(&repo, &["commit", "-m", "feature deletes gone"])?;

    test_helpers::git(&repo, &["checkout", "main"])?;
    test_helpers::write_file(&repo, "gone.txt", "main edit\n")?;
    test_helpers::git(&repo, &["commit", "-am", "main edits gone"])?;
    test_helpers::git(&repo, &["rm", "-q", "gone.txt"])?;
    test_helpers::git(&repo, &["commit", "-m", "main deletes gone"])?;

    let out = test_helpers::mergetopus(&repo, &["feature", "--quiet"])?;
    assert_ok(&out, "mergetopus (both-deleted)");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("gone.txt") && stdout.contains("Auto-applied"),
        "workflow must report the silently applied deletion:\n{stdout}"
    );

    // Embedded in the partial-merge commit message for auditability.
    let msg = test_helpers::git(&repo, &["log", "-1", "--format=%B", integration_branch()])?;
    assert!(
        msg.contains("auto-applied:") && msg.contains("gone.txt"),
        "partial merge commit must carry the auto-applied section:\n{msg}"
    );

    let integration = tree_paths(&repo, integration_branch())?;
    assert!(
        !integration.iter().any(|p| p == "gone.txt"),
        "gone.txt must be absent from the merged tree:\n{integration:?}"
    );

    // And visible in status detail output.
    let status = test_helpers::mergetopus(&repo, &["--color=never", "status", "feature"])?;
    assert_ok(&status, "mergetopus status");
    let status_out = format!(
        "{}{}",
        String::from_utf8_lossy(&status.stdout),
        String::from_utf8_lossy(&status.stderr)
    );
    assert!(
        status_out.contains("gone.txt"),
        "status must surface the auto-applied deletion:\n{status_out}"
    );
    Ok(())
}
