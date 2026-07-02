use anyhow::{Result, bail};
use crate::color;

use crate::git_ops;

/// Sanitize a string for use as a Git branch name fragment.
///
/// Replaces invalid characters with `_`, collapses consecutive underscores,
/// and trims leading/trailing underscores. When characters are replaced, a
/// short deterministic hash of the original input is appended to prevent
/// collisions between different names that sanitize to the same fragment
/// (e.g. `"feature/foo"` and `"feature:foo"` both producing `"feature_foo"`).
pub fn sanitize_branch_fragment(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut prev_underscore = false;
    let mut had_replacements = false;

    for c in input.chars() {
        let ok = c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');
        if ok {
            out.push(c);
            prev_underscore = false;
        } else if !prev_underscore {
            out.push('_');
            prev_underscore = true;
            had_replacements = true;
        }
    }

    let trimmed = out.trim_matches('_').to_string();
    if trimmed.is_empty() || !had_replacements {
        return trimmed;
    }

    // A simple polynomial hash that is stable across Rust versions and
    // platforms. 16-bit suffix = 1-in-65536 collision chance, more than
    // adequate for disambiguation within a single repository.
    let hash = input
        .bytes()
        .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32));
    format!("{trimmed}_{:04x}", hash & 0xFFFF)
}

/// Like `sanitize_branch_fragment` but never appends a disambiguation hash.
/// Use this when comparing against stored branch-name tokens in existing MMM
/// branch names (status, verify) for backward compatibility.
pub fn sanitize_branch_fragment_legacy(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut prev_underscore = false;

    for c in input.chars() {
        let ok = c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');
        if ok {
            out.push(c);
            prev_underscore = false;
        } else if !prev_underscore {
            out.push('_');
            prev_underscore = true;
        }
    }

    out.trim_matches('_').to_string()
}

fn sanitize_or_default(input: &str, fallback: &str) -> String {
    let value = sanitize_branch_fragment(input);
    if value.is_empty() {
        fallback.to_string()
    } else {
        value
    }
}

pub fn integration_branch_family_prefix(current_branch: &str) -> String {
    format!("_mmm/{}/", sanitize_or_default(current_branch, "current"))
}

fn integration_branch_prefix(current_branch: &str, merge_source: &str) -> String {
    format!(
        "_mmm/{}/{}",
        sanitize_or_default(current_branch, "current"),
        sanitize_or_default(merge_source, "source")
    )
}

pub fn integration_branch_name(current_branch: &str, merge_source: &str) -> String {
    format!(
        "{}/integration",
        integration_branch_prefix(current_branch, merge_source)
    )
}

pub fn slice_branch_name(integration_branch: &str, index_one_based: usize) -> Result<String> {
    if index_one_based == 0 {
        bail!("slice index must be one-based");
    }

    let prefix = integration_branch
        .strip_suffix("/integration")
        .ok_or_else(|| {
            anyhow::anyhow!(
                "integration branch '{integration_branch}' must end with '/integration'"
            )
        })?;

    Ok(format!("{prefix}/slice{index_one_based}"))
}

pub fn create_slice_branches(
    integration_branch: &str,
    slice_base: &str,
    source_ref: &str,
    source_sha: &str,
    all_conflicts: &[String],
    explicit_slices: &[Vec<String>],
) -> Result<()> {
    let mut slice_index = 1usize;
    let mut explicitly_assigned = std::collections::BTreeSet::new();

    for group in explicit_slices {
        if group.is_empty() {
            continue;
        }

        let slice_number = slice_index;
        let slice_branch = slice_branch_name(integration_branch, slice_index)?;
        slice_index += 1;
        git_ops::checkout_new_or_reset(&slice_branch, slice_base)?;

        for path in group {
            explicitly_assigned.insert(path.clone());

            if git_ops::path_exists_in_ref(source_ref, path)? {
                git_ops::restore_from_ref(source_ref, path)?;
            } else {
                git_ops::rm_path(path)?;
            }
        }

        if git_ops::staged_has_changes()? {
            let trailers = {
                let mut t = vec![
                    format!("Source-Ref: {source_ref}"),
                    format!("Source-Commit: {source_sha}"),
                    format!("Slice-Paths: {}", group.join(", ")),
                ];

                for path in group {
                    let p = git_ops::path_provenance(source_ref, source_sha, path)?;
                    t.push(format!("Source-Path: {}", p.path));
                    t.push(format!(
                        "Source-Path-Commit: {}",
                        p.path_commit.unwrap_or_else(|| "(none)".to_string())
                    ));
                    if let (Some(name), Some(email)) = (p.author_name, p.author_email) {
                        t.push(format!("Co-authored-by: {name} <{email}>"));
                    }
                }

                t.join("\n")
            };

            let files_list = group
                .iter()
                .map(|p| format!("* {p}"))
                .collect::<Vec<_>>()
                .join("\n");

            let message = format!(
                "Mergetopus - slice{slice_number} from {source_ref} (theirs)\n\nFiles:\n{files_list}\n\n{trailers}"
            );

            git_ops::commit(&message)?;
            color::print_success(&format!("Created explicit slice branch {slice_branch} for {} file(s)", group.len()), None);
        } else {
            color::print_warning(&format!("Skipped {slice_branch}: no staged changes"), None);
        }
    }

    for path in all_conflicts {
        if explicitly_assigned.contains(path) {
            continue;
        }

        let slice_number = slice_index;
        let slice_branch = slice_branch_name(integration_branch, slice_index)?;
        slice_index += 1;
        git_ops::checkout_new_or_reset(&slice_branch, slice_base)?;

        if git_ops::path_exists_in_ref(source_ref, path)? {
            git_ops::restore_from_ref(source_ref, path)?;
        } else {
            git_ops::rm_path(path)?;
        }

        if git_ops::staged_has_changes()? {
            let provenance = git_ops::path_provenance(source_ref, source_sha, path)?;

            let trailers = {
                let mut t = vec![
                    format!("Source-Ref: {}", provenance.source_ref),
                    format!("Source-Commit: {}", provenance.source_commit),
                    format!("Source-Path: {}", provenance.path),
                    format!(
                        "Source-Path-Commit: {}",
                        provenance
                            .path_commit
                            .clone()
                            .unwrap_or_else(|| "(none)".to_string())
                    ),
                ];

                if let (Some(name), Some(email)) =
                    (&provenance.author_name, &provenance.author_email)
                {
                    t.push(format!("Co-authored-by: {name} <{email}>"));
                }

                t.join("\n")
            };

            let message = format!(
                "Mergetopus - slice{slice_number} from {source_ref} (theirs)\n\nFiles:\n* {path}\n\n{trailers}"
            );

            git_ops::commit_slice(&message, &provenance)?;
            color::print_success(&format!("Created default single-file slice branch {slice_branch} for {path}"), None);
        } else {
            color::print_warning(&format!("Skipped {slice_branch} for {path}: no staged changes"), None);
        }
    }

    Ok(())
}

/// Check if a branch name is a slice branch (ends with /slice<digits>).
pub fn is_slice_branch(branch: &str) -> bool {
    let Some((prefix, suffix)) = branch.rsplit_once("/slice") else {
        return false;
    };

    branch.starts_with("_mmm/")
        && !prefix.ends_with('/')
        && !suffix.is_empty()
        && suffix.chars().all(|c| c.is_ascii_digit())
}

/// Parse an integration branch name to extract the original branch and source.
/// Integration branch format: _mmm/<original>/<source>/integration
/// Returns (original_branch, source) if it's a valid integration branch, None otherwise.
pub fn parse_integration_branch(branch: &str) -> Option<(String, String)> {
    let parts = branch.split('/').collect::<Vec<_>>();
    if parts.len() == 4
        && parts[0] == "_mmm"
        && !parts[1].is_empty()
        && !parts[2].is_empty()
        && parts[3] == "integration"
    {
        return Some((parts[1].to_string(), parts[2].to_string()));
    }

    None
}

/// Convert a slice branch name to its matching integration branch name.
/// Slice format: _mmm/<original>/<source>/slice<N>
/// Integration format: _mmm/<original>/<source>/integration
pub fn integration_from_slice_branch(slice_branch: &str) -> Option<String> {
    let (prefix, suffix) = slice_branch.rsplit_once("/slice")?;
    if !slice_branch.starts_with("_mmm/")
        || suffix.is_empty()
        || !suffix.chars().all(|c| c.is_ascii_digit())
    {
        return None;
    }

    Some(format!("{prefix}/integration"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_fragment_keeps_safe_chars() {
        // Characters that are replaced get a short disambiguation hash suffix.
        let a = sanitize_branch_fragment("feature/refactor-auth");
        assert!(a.starts_with("feature_refactor-auth_"), "expected hash suffix, got {a}");
        let b = sanitize_branch_fragment("release 1.0");
        assert!(b.starts_with("release_1.0_"), "expected hash suffix, got {b}");
        // All-invalid input still produces empty (no hash needed).
        assert_eq!(sanitize_branch_fragment("***"), "");
        // Purely safe input has no hash suffix (backward compatible).
        assert_eq!(
            sanitize_branch_fragment("feature.refactor-auth"),
            "feature.refactor-auth"
        );
    }

    #[test]
    fn sanitize_fragment_legacy_preserves_old_behavior() {
        assert_eq!(
            sanitize_branch_fragment_legacy("feature/refactor-auth"),
            "feature_refactor-auth"
        );
        assert_eq!(
            sanitize_branch_fragment_legacy("release 1.0"),
            "release_1.0"
        );
        assert_eq!(sanitize_branch_fragment_legacy("***"), "");
    }

    #[test]
    fn sanitize_fragment_disambiguates_collisions() {
        let a = sanitize_branch_fragment("feature/foo");
        let b = sanitize_branch_fragment("feature:foo");
        assert_ne!(a, b, "colliding inputs must produce different outputs");
    }

    #[test]
    fn sanitize_fragment_hash_hardcoded_output() {
        // Hardcoded expected values so changes to the hash algorithm are
        // caught by CI rather than silently producing different branch names.
        // Values below were computed by the current polynomial hash.
        // If CI fails here, the hash algorithm changed and branch names
        // will differ from previous runs — update intentionally.
        assert_eq!(
            sanitize_branch_fragment("feature/foo"),
            "feature_foo_fa2d",
        );
        assert_eq!(
            sanitize_branch_fragment("feature:foo"),
            "feature_foo_fa42",
        );
    }

    #[test]
    fn sanitize_fragment_hash_is_deterministic() {
        let a = sanitize_branch_fragment("feature/release/1.0");
        let b = sanitize_branch_fragment("feature/release/1.0");
        assert_eq!(a, b, "same input must produce same output across calls");
        let expected_prefix = "feature_release_1.0_";
        assert!(
            a.starts_with(expected_prefix),
            "expected prefix '{expected_prefix}', got '{a}'"
        );
        // Verify the suffix is a 4-char hex string.
        let suffix = a.strip_prefix(expected_prefix).unwrap();
        assert_eq!(suffix.len(), 4, "hash suffix should be 4 hex chars, got '{suffix}'");
        assert!(
            suffix.chars().all(|c| c.is_ascii_hexdigit()),
            "hash suffix should be hex, got '{suffix}'"
        );
    }

    #[test]
    fn integration_name_uses_default_for_empty_source() {
        let name = integration_branch_name("main", "***");
        assert_eq!(name, "_mmm/main/source/integration");
    }

    #[test]
    fn slice_name_is_one_based() {
        assert_eq!(
            slice_branch_name("_mmm/main/x/integration", 1).unwrap(),
            "_mmm/main/x/slice1"
        );
        assert!(slice_branch_name("x", 0).is_err());
    }

    #[test]
    fn test_is_slice_branch() {
        assert!(is_slice_branch("_mmm/main/feature/slice1"));
        assert!(is_slice_branch("_mmm/main/feature/slice99"));
        assert!(!is_slice_branch("_mmm/main/feature/integration"));
        assert!(!is_slice_branch("_mmm/main/feature/kokomeco"));
        assert!(!is_slice_branch("slice1"));
    }

    #[test]
    fn test_parse_integration_branch() {
        assert_eq!(
            parse_integration_branch("_mmm/main/feature/integration"),
            Some(("main".to_string(), "feature".to_string()))
        );
        assert_eq!(
            parse_integration_branch("_mmm/develop/release_v1/integration"),
            Some(("develop".to_string(), "release_v1".to_string()))
        );
        assert_eq!(parse_integration_branch("main"), None);
        assert_eq!(parse_integration_branch("_mmm/main/feature/slice1"), None);
        assert_eq!(parse_integration_branch("_mmm/main/feature/kokomeco"), None);
    }

    #[test]
    fn integration_from_slice_branch_works() {
        assert_eq!(
            integration_from_slice_branch("_mmm/main/feature/slice1"),
            Some("_mmm/main/feature/integration".to_string())
        );
        assert_eq!(
            integration_from_slice_branch("_mmm/main/feature/slice99"),
            Some("_mmm/main/feature/integration".to_string())
        );
        assert_eq!(
            integration_from_slice_branch("_mmm/main/feature/integration"),
            None
        );
        assert_eq!(integration_from_slice_branch("slice1"), None);
    }
}
