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
            println!("Created default single-file slice branch {slice_branch} for {path}");
        } else {
            println!("Skipped {slice_branch} for {path}: no staged changes");
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
