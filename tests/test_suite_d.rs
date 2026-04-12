//! Suite D: Multi-LTS cascade merge scenario with team role-based slice resolution.
//!
//! Depicts a realistic large-project merge flow where a senior integrator (Stan)
//! cascades changes from older LTS branches into newer ones and upstream to main,
//! with team members resolving different slices based on code ownership.
//!
//! Developers:
//!   Wendy Corduroy  – features in LTS_v17, LTS_v32
//!   Gideon Gleeful  – features in LTS_v17, LTS_v32
//!   Dipper Pines    – features in LTS_v32, main
//!   Mabel Pines     – team lead (Gideon's), features in LTS_v32, main
//!   Stan Pines      – senior integrator, merges forward through LTS_v42 to main

use std::path::Path;
use std::process::Command;

mod test_helpers;

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

// --- Author identities ---
const WENDY_NAME: &str = "Wendy Corduroy";
const WENDY_EMAIL: &str = "wendy@gravityfalls.example.com";

const GIDEON_NAME: &str = "Gideon Gleeful";
const GIDEON_EMAIL: &str = "gideon@gravityfalls.example.com";

const DIPPER_NAME: &str = "Dipper Pines";
const DIPPER_EMAIL: &str = "dipper@gravityfalls.example.com";

const MABEL_NAME: &str = "Mabel Pines";
const MABEL_EMAIL: &str = "mabel@gravityfalls.example.com";

const STAN_NAME: &str = "Stan Pines";
const STAN_EMAIL: &str = "stan@gravityfalls.example.com";

/// Writes files, stages, and commits with a specific author.
fn write_and_commit_as(
    repo: &Path,
    files: &[(&str, &str)],
    name: &str,
    email: &str,
    message: &str,
) -> TestResult<()> {
    for (path, content) in files {
        test_helpers::write_file(repo, path, content)?;
    }
    test_helpers::git(repo, &["add", "."])?;
    let author = format!("{name} <{email}>");
    test_helpers::git(repo, &["commit", "-m", message, "--author", &author])?;
    Ok(())
}

/// Resolves a slice by merging it into the integration branch, taking the
/// source-side ("theirs") content for all conflicting files, and committing
/// as a specific team member.
fn resolve_slice_take_theirs(
    repo: &Path,
    integration: &str,
    slice: &str,
    conflicted_files: &[&str],
    resolver_name: &str,
    resolver_email: &str,
    message: &str,
) -> TestResult<()> {
    let current = test_helpers::git(repo, &["branch", "--show-current"])?;
    if current != integration {
        test_helpers::git(repo, &["checkout", integration])?;
    }

    // merge --no-commit may exit non-zero when there are conflicts — expected
    let _merge_out = test_helpers::run(
        Command::new("git")
            .args(["merge", "--no-commit", "--no-ff", slice])
            .current_dir(repo),
    )?;

    for file in conflicted_files {
        test_helpers::git(repo, &["checkout", "--theirs", "--", file])?;
    }

    let mut add_args = vec!["add"];
    add_args.extend(conflicted_files.iter().copied());
    test_helpers::git(repo, &add_args)?;

    let author = format!("{resolver_name} <{resolver_email}>");
    test_helpers::git(repo, &["commit", "-m", message, "--author", &author])?;

    Ok(())
}

/// Creates the full multi-LTS scenario with proper per-developer authorship.
///
/// File layout (all branches start from M0):
///   config.toml  – 2 lines: max_connections, timeout
///   engine.rs    – 2 lines: init(), process()
///   api.rs       – 1 line:  handle()
///   utils.rs     – 1 line:  format()
///
/// Branch topology after setup:
///   main:     M0 ── Dipper ── Mabel
///   LTS_v17:  M0 ── Wendy ── Gideon
///   LTS_v32:  M0 ── Wendy ── Gideon ── Dipper ── Mabel
///   LTS_v42:  M0 ── Stan
fn setup_gravity_falls_lts_repo() -> TestResult<std::path::PathBuf> {
    let repo = test_helpers::init_repo()?;

    // === Initial base files on main ===
    test_helpers::write_file(&repo, "config.toml", "max_connections = 100\ntimeout = 30\n")?;
    test_helpers::write_file(
        &repo,
        "engine.rs",
        "fn init() { setup(); }\nfn process() { validate(); }\n",
    )?;
    test_helpers::write_file(&repo, "api.rs", "fn handle() { parse(); }\n")?;
    test_helpers::write_file(&repo, "utils.rs", "fn format() { serialize(); }\n")?;
    test_helpers::commit_all(&repo, "Initial project setup")?;

    // === Create LTS branches from main ===
    test_helpers::git(&repo, &["branch", "LTS_v17"])?;
    test_helpers::git(&repo, &["branch", "LTS_v32"])?;
    test_helpers::git(&repo, &["branch", "LTS_v42"])?;

    // === LTS_v17: Wendy and Gideon with overlapping changes ===
    test_helpers::git(&repo, &["checkout", "LTS_v17"])?;

    // Wendy changes config.toml line 1, engine.rs line 1, plus api.rs and utils.rs
    write_and_commit_as(
        &repo,
        &[
            ("config.toml", "max_connections = 200\ntimeout = 30\n"),
            (
                "engine.rs",
                "fn init() { wendy_pool(); }\nfn process() { validate(); }\n",
            ),
            ("api.rs", "fn handle() { wendy_caching(); }\n"),
            ("utils.rs", "fn format() { wendy_format(); }\n"),
        ],
        WENDY_NAME,
        WENDY_EMAIL,
        "Wendy: v17 connection pooling and caching",
    )?;

    // Gideon changes config.toml line 2 and engine.rs line 2 (overlapping files!)
    write_and_commit_as(
        &repo,
        &[
            ("config.toml", "max_connections = 200\ntimeout = 60\n"),
            (
                "engine.rs",
                "fn init() { wendy_pool(); }\nfn process() { gideon_errors(); }\n",
            ),
        ],
        GIDEON_NAME,
        GIDEON_EMAIL,
        "Gideon: v17 timeout hardening and error handling",
    )?;

    // === LTS_v32: All four developers contribute ===
    test_helpers::git(&repo, &["checkout", "LTS_v32"])?;

    write_and_commit_as(
        &repo,
        &[
            ("config.toml", "max_connections = 100\ntimeout = 45\n"),
            ("api.rs", "fn handle() { wendy_retry(); }\n"),
        ],
        WENDY_NAME,
        WENDY_EMAIL,
        "Wendy: v32 retry logic",
    )?;

    write_and_commit_as(
        &repo,
        &[(
            "engine.rs",
            "fn init() { setup(); }\nfn process() { gideon_cache(); }\n",
        )],
        GIDEON_NAME,
        GIDEON_EMAIL,
        "Gideon: v32 response caching",
    )?;

    write_and_commit_as(
        &repo,
        &[(
            "config.toml",
            "max_connections = 100\ntimeout = 45\nlog_level = debug\n",
        )],
        DIPPER_NAME,
        DIPPER_EMAIL,
        "Dipper: v32 debug logging config",
    )?;

    write_and_commit_as(
        &repo,
        &[
            (
                "engine.rs",
                "fn init() { mabel_auth(); }\nfn process() { gideon_cache(); }\n",
            ),
            ("utils.rs", "fn format() { mabel_logging(); }\n"),
        ],
        MABEL_NAME,
        MABEL_EMAIL,
        "Mabel: v32 auth enforcement and structured logging",
    )?;

    // === LTS_v42: Stan makes baseline changes (will conflict with LTS branches) ===
    test_helpers::git(&repo, &["checkout", "LTS_v42"])?;

    write_and_commit_as(
        &repo,
        &[
            ("config.toml", "max_connections = 150\ntimeout = 35\n"),
            (
                "engine.rs",
                "fn init() { stan_baseline(); }\nfn process() { stan_validate(); }\n",
            ),
            ("api.rs", "fn handle() { stan_api(); }\n"),
            ("utils.rs", "fn format() { stan_utils(); }\n"),
        ],
        STAN_NAME,
        STAN_EMAIL,
        "Stan: v42 baseline adjustments",
    )?;

    // === main: Dipper and Mabel also work on main ===
    test_helpers::git(&repo, &["checkout", "main"])?;

    write_and_commit_as(
        &repo,
        &[(
            "config.toml",
            "max_connections = 100\ntimeout = 30\nmetrics = true\n",
        )],
        DIPPER_NAME,
        DIPPER_EMAIL,
        "Dipper: main metrics infrastructure",
    )?;

    write_and_commit_as(
        &repo,
        &[(
            "engine.rs",
            "fn init() { setup(); }\nfn process() { mabel_telemetry(); }\n",
        )],
        MABEL_NAME,
        MABEL_EMAIL,
        "Mabel: main telemetry hooks",
    )?;

    Ok(repo)
}

/// Full cascade merge: LTS_v17 → LTS_v42, LTS_v32 → LTS_v42, LTS_v42 → main.
///
/// Verifies:
/// - Integration and slice branches are created correctly
/// - Mabel resolves slices containing Gideon's changes
/// - Dipper resolves slices containing Wendy's changes
/// - Kokomeco merge topology has correct parents
/// - git blame on kokomeco correctly attributes lines to original authors
#[test]
fn lts_cascade_merge_preserves_authorship_in_kokomeco() -> TestResult<()> {
    let repo = setup_gravity_falls_lts_repo()?;

    let lts_v17_sha = test_helpers::git(&repo, &["rev-parse", "LTS_v17"])?;
    let lts_v42_original = test_helpers::git(&repo, &["rev-parse", "LTS_v42"])?;

    // ====================================================================
    // MERGE 1: LTS_v17 → LTS_v42
    //
    // Stan groups config.toml + engine.rs (Gideon's overlapping changes)
    // into one explicit slice for Mabel. The remaining files (api.rs,
    // utils.rs — Wendy's sole changes) become auto-slices for Dipper.
    // ====================================================================
    test_helpers::git(&repo, &["checkout", "LTS_v42"])?;

    let out = test_helpers::mergetopus(
        &repo,
        &[
            "LTS_v17",
            "--quiet",
            "--select-paths",
            "config.toml,engine.rs",
        ],
    )?;
    assert!(
        out.status.success(),
        "mergetopus LTS_v17 → LTS_v42 failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let v17_integration = "_mmm/LTS_v42/LTS_v17/integration";
    let v17_slice1 = "_mmm/LTS_v42/LTS_v17/slice1"; // explicit: config.toml, engine.rs
    let v17_slice2 = "_mmm/LTS_v42/LTS_v17/slice2"; // auto: api.rs
    let v17_slice3 = "_mmm/LTS_v42/LTS_v17/slice3"; // auto: utils.rs

    for branch in [v17_integration, v17_slice1, v17_slice2, v17_slice3] {
        test_helpers::git(
            &repo,
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        )?;
    }

    // --- Mabel resolves slice1 (Gideon's config + engine changes) ---
    resolve_slice_take_theirs(
        &repo,
        v17_integration,
        v17_slice1,
        &["config.toml", "engine.rs"],
        MABEL_NAME,
        MABEL_EMAIL,
        "Mabel resolves slice1: Gideon's config and engine changes",
    )?;

    // --- Dipper resolves slice2 (Wendy's API changes) ---
    resolve_slice_take_theirs(
        &repo,
        v17_integration,
        v17_slice2,
        &["api.rs"],
        DIPPER_NAME,
        DIPPER_EMAIL,
        "Dipper resolves slice2: Wendy's API changes",
    )?;

    // --- Dipper resolves slice3 (Wendy's utils changes) ---
    resolve_slice_take_theirs(
        &repo,
        v17_integration,
        v17_slice3,
        &["utils.rs"],
        DIPPER_NAME,
        DIPPER_EMAIL,
        "Dipper resolves slice3: Wendy's utils changes",
    )?;

    // --- Stan triggers consolidation → kokomeco₁ ---
    test_helpers::git(&repo, &["checkout", "LTS_v42"])?;
    let consolidate = test_helpers::mergetopus(&repo, &["LTS_v17", "--quiet", "--yes"])?;
    assert!(
        consolidate.status.success(),
        "kokomeco₁ creation failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&consolidate.stdout),
        String::from_utf8_lossy(&consolidate.stderr)
    );

    let v17_kokomeco = "_mmm/LTS_v42/LTS_v17/kokomeco";

    // === Verify kokomeco₁ topology ===
    let parents = test_helpers::git(&repo, &["show", "-s", "--format=%P", v17_kokomeco])?;
    let parent_list: Vec<&str> = parents.split_whitespace().collect();
    assert_eq!(
        parent_list.len(),
        2,
        "kokomeco₁ must be a merge commit with 2 parents"
    );
    assert_eq!(
        parent_list[0], lts_v42_original,
        "kokomeco₁ parent 1 = original LTS_v42 HEAD"
    );
    assert_eq!(
        parent_list[1], lts_v17_sha,
        "kokomeco₁ parent 2 = LTS_v17 tip"
    );

    // === Verify blame on kokomeco₁ ===

    // config.toml: line 1 (max_connections) → Wendy, line 2 (timeout) → Gideon
    let blame_config = test_helpers::git(
        &repo,
        &["blame", "--porcelain", v17_kokomeco, "--", "config.toml"],
    )?;
    assert!(
        blame_config.contains("author Wendy Corduroy"),
        "config.toml blame should show Wendy:\n{blame_config}",
    );
    assert!(
        blame_config.contains("author Gideon Gleeful"),
        "config.toml blame should show Gideon:\n{blame_config}",
    );

    // engine.rs: line 1 (init) → Wendy, line 2 (process) → Gideon
    let blame_engine = test_helpers::git(
        &repo,
        &["blame", "--porcelain", v17_kokomeco, "--", "engine.rs"],
    )?;
    assert!(
        blame_engine.contains("author Wendy Corduroy"),
        "engine.rs blame should show Wendy:\n{blame_engine}",
    );
    assert!(
        blame_engine.contains("author Gideon Gleeful"),
        "engine.rs blame should show Gideon:\n{blame_engine}",
    );

    // api.rs → Wendy only
    let blame_api = test_helpers::git(
        &repo,
        &["blame", "--porcelain", v17_kokomeco, "--", "api.rs"],
    )?;
    assert!(
        blame_api.contains("author Wendy Corduroy"),
        "api.rs blame should show Wendy:\n{blame_api}",
    );

    // utils.rs → Wendy only
    let blame_utils = test_helpers::git(
        &repo,
        &["blame", "--porcelain", v17_kokomeco, "--", "utils.rs"],
    )?;
    assert!(
        blame_utils.contains("author Wendy Corduroy"),
        "utils.rs blame should show Wendy:\n{blame_utils}",
    );

    // ====================================================================
    // Promote kokomeco₁ into LTS_v42
    // ====================================================================
    test_helpers::git(&repo, &["checkout", "LTS_v42"])?;
    test_helpers::git(&repo, &["merge", "--ff", v17_kokomeco])?;

    // ====================================================================
    // MERGE 2: LTS_v32 → LTS_v42
    //
    // Stan groups engine.rs + utils.rs (Gideon + Mabel changes) into one
    // explicit slice for Mabel. Remaining files become auto-slices for Dipper.
    // ====================================================================
    let lts_v32_sha = test_helpers::git(&repo, &["rev-parse", "LTS_v32"])?;
    let lts_v42_before_v32 = test_helpers::git(&repo, &["rev-parse", "LTS_v42"])?;

    let out = test_helpers::mergetopus(
        &repo,
        &[
            "LTS_v32",
            "--quiet",
            "--select-paths",
            "engine.rs,utils.rs",
        ],
    )?;
    assert!(
        out.status.success(),
        "mergetopus LTS_v32 → LTS_v42 failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let v32_integration = "_mmm/LTS_v42/LTS_v32/integration";
    let v32_slice1 = "_mmm/LTS_v42/LTS_v32/slice1"; // explicit: engine.rs, utils.rs
    let v32_slice2 = "_mmm/LTS_v42/LTS_v32/slice2"; // auto: api.rs
    let v32_slice3 = "_mmm/LTS_v42/LTS_v32/slice3"; // auto: config.toml

    for branch in [v32_integration, v32_slice1, v32_slice2, v32_slice3] {
        test_helpers::git(
            &repo,
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        )?;
    }

    // --- Mabel resolves slice1 (Gideon + Mabel's engine and utils changes) ---
    resolve_slice_take_theirs(
        &repo,
        v32_integration,
        v32_slice1,
        &["engine.rs", "utils.rs"],
        MABEL_NAME,
        MABEL_EMAIL,
        "Mabel resolves slice1: Gideon's caching and Mabel's auth changes",
    )?;

    // --- Dipper resolves slice2 (Wendy's API changes) ---
    resolve_slice_take_theirs(
        &repo,
        v32_integration,
        v32_slice2,
        &["api.rs"],
        DIPPER_NAME,
        DIPPER_EMAIL,
        "Dipper resolves slice2: Wendy's retry logic",
    )?;

    // --- Dipper resolves slice3 (Wendy + Dipper's config changes) ---
    resolve_slice_take_theirs(
        &repo,
        v32_integration,
        v32_slice3,
        &["config.toml"],
        DIPPER_NAME,
        DIPPER_EMAIL,
        "Dipper resolves slice3: Wendy and Dipper's config changes",
    )?;

    // --- Stan triggers consolidation → kokomeco₂ ---
    test_helpers::git(&repo, &["checkout", "LTS_v42"])?;
    let consolidate = test_helpers::mergetopus(&repo, &["LTS_v32", "--quiet", "--yes"])?;
    assert!(
        consolidate.status.success(),
        "kokomeco₂ creation failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&consolidate.stdout),
        String::from_utf8_lossy(&consolidate.stderr)
    );

    let v32_kokomeco = "_mmm/LTS_v42/LTS_v32/kokomeco";

    // === Verify kokomeco₂ topology ===
    let parents = test_helpers::git(&repo, &["show", "-s", "--format=%P", v32_kokomeco])?;
    let parent_list: Vec<&str> = parents.split_whitespace().collect();
    assert_eq!(parent_list.len(), 2, "kokomeco₂ must be a merge commit");
    assert_eq!(
        parent_list[0], lts_v42_before_v32,
        "kokomeco₂ parent 1 = LTS_v42 before v32 merge"
    );
    assert_eq!(
        parent_list[1], lts_v32_sha,
        "kokomeco₂ parent 2 = LTS_v32 tip"
    );

    // === Verify blame on kokomeco₂ ===

    // config.toml from LTS_v32: line 2 → Wendy, line 3 → Dipper
    let blame_config = test_helpers::git(
        &repo,
        &["blame", "--porcelain", v32_kokomeco, "--", "config.toml"],
    )?;
    assert!(
        blame_config.contains("author Wendy Corduroy"),
        "config.toml blame should show Wendy (v32):\n{blame_config}",
    );
    assert!(
        blame_config.contains("author Dipper Pines"),
        "config.toml blame should show Dipper (v32):\n{blame_config}",
    );

    // engine.rs from LTS_v32: line 1 → Mabel, line 2 → Gideon
    let blame_engine = test_helpers::git(
        &repo,
        &["blame", "--porcelain", v32_kokomeco, "--", "engine.rs"],
    )?;
    assert!(
        blame_engine.contains("author Mabel Pines"),
        "engine.rs blame should show Mabel (v32):\n{blame_engine}",
    );
    assert!(
        blame_engine.contains("author Gideon Gleeful"),
        "engine.rs blame should show Gideon (v32):\n{blame_engine}",
    );

    // api.rs → Wendy
    let blame_api = test_helpers::git(
        &repo,
        &["blame", "--porcelain", v32_kokomeco, "--", "api.rs"],
    )?;
    assert!(
        blame_api.contains("author Wendy Corduroy"),
        "api.rs blame should show Wendy (v32):\n{blame_api}",
    );

    // utils.rs → Mabel
    let blame_utils = test_helpers::git(
        &repo,
        &["blame", "--porcelain", v32_kokomeco, "--", "utils.rs"],
    )?;
    assert!(
        blame_utils.contains("author Mabel Pines"),
        "utils.rs blame should show Mabel (v32):\n{blame_utils}",
    );

    // ====================================================================
    // Promote kokomeco₂ into LTS_v42
    // ====================================================================
    test_helpers::git(&repo, &["checkout", "LTS_v42"])?;
    test_helpers::git(&repo, &["merge", "--ff", v32_kokomeco])?;

    // ====================================================================
    // MERGE 3: LTS_v42 → main
    //
    // Only config.toml and engine.rs conflict (api.rs and utils.rs
    // auto-merge since main never changed them from M0).
    // ====================================================================
    let main_original = test_helpers::git(&repo, &["rev-parse", "main"])?;
    let lts_v42_final = test_helpers::git(&repo, &["rev-parse", "LTS_v42"])?;

    test_helpers::git(&repo, &["checkout", "main"])?;

    let out = test_helpers::mergetopus(&repo, &["LTS_v42", "--quiet"])?;
    assert!(
        out.status.success(),
        "mergetopus LTS_v42 → main failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let main_integration = "_mmm/main/LTS_v42/integration";
    let main_slice1 = "_mmm/main/LTS_v42/slice1"; // auto: config.toml
    let main_slice2 = "_mmm/main/LTS_v42/slice2"; // auto: engine.rs

    for branch in [main_integration, main_slice1, main_slice2] {
        test_helpers::git(
            &repo,
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        )?;
    }

    // --- Resolve slice1 (config.toml) – take LTS_v42 content ---
    resolve_slice_take_theirs(
        &repo,
        main_integration,
        main_slice1,
        &["config.toml"],
        STAN_NAME,
        STAN_EMAIL,
        "Stan resolves slice1: LTS_v42 config into main",
    )?;

    // --- Resolve slice2 (engine.rs) – take LTS_v42 content ---
    resolve_slice_take_theirs(
        &repo,
        main_integration,
        main_slice2,
        &["engine.rs"],
        STAN_NAME,
        STAN_EMAIL,
        "Stan resolves slice2: LTS_v42 engine into main",
    )?;

    // --- Consolidate → kokomeco₃ ---
    test_helpers::git(&repo, &["checkout", "main"])?;
    let consolidate = test_helpers::mergetopus(&repo, &["LTS_v42", "--quiet", "--yes"])?;
    assert!(
        consolidate.status.success(),
        "kokomeco₃ creation failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&consolidate.stdout),
        String::from_utf8_lossy(&consolidate.stderr)
    );

    let main_kokomeco = "_mmm/main/LTS_v42/kokomeco";

    // === Verify kokomeco₃ topology ===
    let parents = test_helpers::git(&repo, &["show", "-s", "--format=%P", main_kokomeco])?;
    let parent_list: Vec<&str> = parents.split_whitespace().collect();
    assert_eq!(parent_list.len(), 2, "kokomeco₃ must be a merge commit");
    assert_eq!(
        parent_list[0], main_original,
        "kokomeco₃ parent 1 = original main HEAD"
    );
    assert_eq!(
        parent_list[1], lts_v42_final,
        "kokomeco₃ parent 2 = final LTS_v42 (after both LTS merges)"
    );

    // === Verify blame on kokomeco₃ ===
    // Content comes from LTS_v42 which has LTS_v32 files after the second promotion.
    // Blame should trace through kokomeco₃ → LTS_v42 → kokomeco₂ → LTS_v32 → authors.

    // config.toml: Wendy (timeout line) + Dipper (log_level line)
    let blame_config = test_helpers::git(
        &repo,
        &["blame", "--porcelain", main_kokomeco, "--", "config.toml"],
    )?;
    assert!(
        blame_config.contains("author Wendy Corduroy"),
        "final config.toml blame should trace back to Wendy:\n{blame_config}",
    );
    assert!(
        blame_config.contains("author Dipper Pines"),
        "final config.toml blame should trace back to Dipper:\n{blame_config}",
    );

    // engine.rs: Mabel (init line) + Gideon (process line)
    let blame_engine = test_helpers::git(
        &repo,
        &["blame", "--porcelain", main_kokomeco, "--", "engine.rs"],
    )?;
    assert!(
        blame_engine.contains("author Mabel Pines"),
        "final engine.rs blame should trace back to Mabel:\n{blame_engine}",
    );
    assert!(
        blame_engine.contains("author Gideon Gleeful"),
        "final engine.rs blame should trace back to Gideon:\n{blame_engine}",
    );

    // api.rs: Wendy (auto-merged from LTS_v42 side, traces back to LTS_v32 → Wendy)
    let blame_api = test_helpers::git(
        &repo,
        &["blame", "--porcelain", main_kokomeco, "--", "api.rs"],
    )?;
    assert!(
        blame_api.contains("author Wendy Corduroy"),
        "final api.rs blame should trace back to Wendy:\n{blame_api}",
    );

    // utils.rs: Mabel (auto-merged from LTS_v42 side, traces back to LTS_v32 → Mabel)
    let blame_utils = test_helpers::git(
        &repo,
        &["blame", "--porcelain", main_kokomeco, "--", "utils.rs"],
    )?;
    assert!(
        blame_utils.contains("author Mabel Pines"),
        "final utils.rs blame should trace back to Mabel:\n{blame_utils}",
    );

    Ok(())
}
