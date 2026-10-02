//! The library surface must be reachable from an external crate without
//! building the TUI/CLI stack (overlay-plan task 0a acceptance).

#[test]
fn planner_branch_logic_is_pure_and_public() {
    let name = mergetopus::planner::integration_branch_name("main", "feature/x");
    let parsed = mergetopus::planner::parse_integration_branch(&name);
    // The source segment is sanitized into the ref name; parse round-trips it.
    let (target, source) = parsed.expect("must parse its own branch naming");
    assert_eq!(target, "main");
    assert!(source.starts_with("feature_x"), "got {source}");
    assert!(mergetopus::planner::is_slice_branch("_mmm/main/feature_x/slice1"));
    assert!(!mergetopus::planner::is_slice_branch("main"));
}

#[test]
fn git_ops_and_forges_types_are_exported() {
    // Compile-time reachability of the plumbing surface biggit will use.
    let _run: fn(&[&str]) -> anyhow::Result<String> = mergetopus::git_ops::run_git;
    fn _takes_forge(f: &dyn mergetopus::forges::Forge) -> &str {
        f.name()
    }
    let _ = _takes_forge;
}
