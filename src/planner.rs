use anyhow::{Result, bail};

use crate::git_ops;

pub fn sanitize_branch_fragment(input: &str) -> String {
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

pub fn integration_branch_name(current_branch: &str, merge_source: &str) -> String {
    let safe_source = {
        let s = sanitize_branch_fragment(merge_source);
        if s.is_empty() {
            "source".to_string()
        } else {
            s
        }
    };
    format!("{current_branch}_mw_int_{safe_source}")
}

pub fn slice_branch_name(integration_branch: &str, index_one_based: usize) -> Result<String> {
    if index_one_based == 0 {
        bail!("slice index must be one-based");
    }
    Ok(format!("{integration_branch}_slice{index_one_based}"))
}

pub fn create_slice_branches(
    integration_branch: &str,
    remembered_head: &str,
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

        let slice_branch = slice_branch_name(integration_branch, slice_index)?;
        slice_index += 1;
        git_ops::checkout_new_or_reset(&slice_branch, remembered_head)?;

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

            let message = format!(
                "Mergetopus slice: {} file(s) from '{source_ref}' (theirs)\n\n{trailers}",
                group.len()
            );

            git_ops::commit(&message)?;
            println!(
                "Created explicit slice branch {slice_branch} for {} file(s)",
                group.len()
            );
        } else {
            println!("Skipped {slice_branch}: no staged changes");
        }
    }

    for path in all_conflicts {
        if explicitly_assigned.contains(path) {
            continue;
        }

        let slice_branch = slice_branch_name(integration_branch, slice_index)?;
        slice_index += 1;
        git_ops::checkout_new_or_reset(&slice_branch, remembered_head)?;

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

            let message =
                format!("Mergetopus slice: '{path}' from '{source_ref}' (theirs)\n\n{trailers}");

            git_ops::commit_slice(&message, &provenance)?;
            println!("Created default single-file slice branch {slice_branch} for {path}");
        } else {
            println!("Skipped {slice_branch} for {path}: no staged changes");
        }
    }

    Ok(())
}

/// Check if a branch name is a slice branch (ends with _slice<digits>).
pub fn is_slice_branch(branch: &str) -> bool {
    const SLICE_SUFFIX: &str = "_slice";
    if let Some(idx) = branch.rfind(SLICE_SUFFIX) {
        let after = &branch[idx + SLICE_SUFFIX.len()..];
        !after.is_empty() && after.chars().all(|c| c.is_ascii_digit())
    } else {
        false
    }
}

/// Check if a branch name is an integration branch (contains _mw_int_).
#[allow(dead_code)]
pub fn is_integration_branch(branch: &str) -> bool {
    branch.contains("_mw_int_")
}

/// Parse an integration branch name to extract the original branch and source.
/// Integration branch format: <original>_mw_int_<source>
/// Returns (original_branch, source) if it's a valid integration branch, None otherwise.
pub fn parse_integration_branch(branch: &str) -> Option<(String, String)> {
    const INT_MARKER: &str = "_mw_int_";
    let parts: Vec<&str> = branch.splitn(2, INT_MARKER).collect();

    if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
        // Check that the source part doesn't end with _slice or _consolidated
        // (to avoid false positives with slices of integration branches)
        let source = parts[1];
        if !source.ends_with("_consolidated") && !is_slice_branch(branch) {
            return Some((parts[0].to_string(), source.to_string()));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_fragment_keeps_safe_chars() {
        assert_eq!(
            sanitize_branch_fragment("feature/refactor-auth"),
            "feature_refactor-auth"
        );
        assert_eq!(sanitize_branch_fragment("release 1.0"), "release_1.0");
        assert_eq!(sanitize_branch_fragment("***"), "");
    }

    #[test]
    fn integration_name_uses_default_for_empty_source() {
        let name = integration_branch_name("main", "***");
        assert_eq!(name, "main_mw_int_source");
    }

    #[test]
    fn slice_name_is_one_based() {
        assert_eq!(
            slice_branch_name("main_mw_int_x", 1).unwrap(),
            "main_mw_int_x_slice1"
        );
        assert!(slice_branch_name("x", 0).is_err());
    }

    #[test]
    fn test_is_slice_branch() {
        assert!(is_slice_branch("main_mw_int_feature_slice1"));
        assert!(is_slice_branch("main_mw_int_feature_slice99"));
        assert!(!is_slice_branch("main_mw_int_feature"));
        assert!(!is_slice_branch("main_slice"));
        assert!(!is_slice_branch("slice1"));
    }

    #[test]
    fn test_is_integration_branch() {
        assert!(is_integration_branch("main_mw_int_feature"));
        assert!(is_integration_branch("develop_mw_int_bugfix"));
        assert!(!is_integration_branch("main"));
        assert!(!is_integration_branch("feature_branch"));
    }

    #[test]
    fn test_parse_integration_branch() {
        assert_eq!(
            parse_integration_branch("main_mw_int_feature"),
            Some(("main".to_string(), "feature".to_string()))
        );
        assert_eq!(
            parse_integration_branch("develop_mw_int_release_v1"),
            Some(("develop".to_string(), "release_v1".to_string()))
        );
        assert_eq!(parse_integration_branch("main"), None);
        assert_eq!(parse_integration_branch("main_mw_int_feature_slice1"), None);
    }
}
