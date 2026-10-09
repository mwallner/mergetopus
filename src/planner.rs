use crate::color;
use anyhow::{Result, bail};

use crate::git_ops;
use crate::models::{ConflictGroup, ConflictKind, GroupMode, StageBlob, UnassignedPolicy};
use std::collections::BTreeMap;

/// One `CONFLICT (...)` description line reported by git during a merge.
#[derive(Debug, Clone)]
pub struct ParsedConflict {
    pub kind: ConflictKind,
    pub paths: Vec<String>,
}

fn conflict_kind_priority(kind: ConflictKind) -> u8 {
    match kind {
        ConflictKind::RenameRename => 0,
        ConflictKind::RenameDelete => 1,
        ConflictKind::FileLocation => 2,
        ConflictKind::ModifyDelete => 3,
        ConflictKind::Content => 4,
    }
}

/// Extract structured conflict topology from git merge output. Git prints one
/// `CONFLICT (<type>): ...` line per logical conflict; rename-related types
/// name every involved path, which lets slicing keep correlated paths in one
/// group instead of treating them as unrelated files.
pub fn parse_conflict_lines(merge_output: &str) -> Vec<ParsedConflict> {
    let mut parsed = Vec::new();

    for line in merge_output.lines() {
        let line = line.trim_start();
        let Some(rest) = line.strip_prefix("CONFLICT (") else {
            continue;
        };
        let Some((type_name, body)) = rest.split_once("): ") else {
            continue;
        };
        let body = body.trim_end_matches('.');

        let entry = match type_name {
            "rename/rename" => {
                // "<old> renamed to <ours> in <ref> and to <theirs> in <ref>"
                let (old_and_rest, theirs) = match body.split_once(" and to ") {
                    Some(v) => v,
                    None => continue,
                };
                let (old, rest2) = match old_and_rest.split_once(" renamed to ") {
                    Some(v) => v,
                    None => continue,
                };
                let ours = rest2
                    .split_once(" in ")
                    .map(|(name, _)| name)
                    .unwrap_or(rest2);
                let theirs_path = theirs
                    .split_once(" in ")
                    .map(|(name, _)| name)
                    .unwrap_or(theirs);
                ParsedConflict {
                    kind: ConflictKind::RenameRename,
                    paths: vec![old.to_string(), ours.to_string(), theirs_path.to_string()],
                }
            }
            "rename/delete" => {
                // "<old> renamed to <new> in <ref>, but deleted in <ref>"
                let Some((old, rest)) = body.split_once(" renamed to ") else {
                    continue;
                };
                let new = rest
                    .split_once(" in ")
                    .map(|(name, _)| name)
                    .unwrap_or(rest);
                ParsedConflict {
                    kind: ConflictKind::RenameDelete,
                    paths: vec![
                        old.to_string(),
                        new.split(',').next().unwrap_or(new).to_string(),
                    ],
                }
            }
            "file location" => {
                // "<old> added in <ref> inside a directory that was renamed in
                //  <ref>, suggesting it should perhaps be moved to <new>"
                let Some((old, rest)) = body.split_once(" added in ") else {
                    continue;
                };
                let Some((_, new)) = rest.rsplit_once(" moved to ") else {
                    continue;
                };
                ParsedConflict {
                    kind: ConflictKind::FileLocation,
                    paths: vec![old.to_string(), new.to_string()],
                }
            }
            "modify/delete" | "delete/modify" => {
                let path = body
                    .split_once(" deleted in ")
                    .map(|(p, _)| p)
                    .unwrap_or(body);
                if path.is_empty() {
                    continue;
                }
                ParsedConflict {
                    kind: ConflictKind::ModifyDelete,
                    paths: vec![path.to_string()],
                }
            }
            "content" | "add/add" | "rename/add" => {
                let path = match type_name {
                    "rename/add" => {
                        // "Rename <old>-><new> in <ref>. <new> added in <ref> ..."
                        let Some(first) = body.strip_prefix("Rename ") else {
                            continue;
                        };
                        let Some((pair, _)) = first.split_once(" in ") else {
                            continue;
                        };
                        let Some((_, new)) = pair.split_once("->") else {
                            continue;
                        };
                        new.to_string()
                    }
                    _ => {
                        let Some(stripped) = body.strip_prefix("Merge conflict in ") else {
                            continue;
                        };
                        stripped.to_string()
                    }
                };
                if path.is_empty() {
                    continue;
                }
                ParsedConflict {
                    kind: ConflictKind::Content,
                    paths: vec![path],
                }
            }
            _ => continue,
        };

        parsed.push(entry);
    }

    parsed.sort_by_key(|p| conflict_kind_priority(p.kind));
    parsed
}

/// Drop `CONFLICT (...)` lines whose paths have all been settled (e.g. by
/// re-evaluating the conflict set against a chosen merge base), so group
/// building only sees conflicts that actually remain. Non-CONFLICT lines
/// pass through untouched.
pub fn filter_conflict_lines(merge_output: &str, remaining: &[String]) -> String {
    let set: std::collections::BTreeSet<&str> = remaining.iter().map(String::as_str).collect();
    merge_output
        .lines()
        .filter(|line| {
            if !line.contains("CONFLICT") {
                return true;
            }
            parse_conflict_lines(line)
                .iter()
                .any(|c| c.paths.iter().any(|p| set.contains(p.as_str())))
        })
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Build logical conflict groups for all conflicted index paths. Paths named
/// by a parsed rename/file-location line are joined into one group; every
/// remaining conflicted path becomes its own single-path group.
pub fn build_conflict_groups(
    merge_output: &str,
    conflicted: &[String],
    stage_map: &BTreeMap<String, BTreeMap<usize, StageBlob>>,
) -> Vec<ConflictGroup> {
    let parsed = parse_conflict_lines(merge_output);
    let conflicted_set: BTreeMap<&str, ()> = conflicted.iter().map(|p| (p.as_str(), ())).collect();

    let mut assigned: BTreeMap<String, usize> = BTreeMap::new();
    let mut groups: Vec<ConflictGroup> = Vec::new();

    for conflict in &parsed {
        // File-location conflicts involve the added (original) path which is
        // NOT unmerged itself; keep it so slicing can materialize both
        // candidate locations. Other kinds only group unmerged paths.
        let mentioned: Vec<String> = if conflict.kind == ConflictKind::FileLocation {
            conflict.paths.clone()
        } else {
            conflict
                .paths
                .iter()
                .filter(|p| conflicted_set.contains_key(p.as_str()))
                .cloned()
                .collect()
        };
        if mentioned.is_empty() {
            continue;
        }

        let existing = mentioned
            .iter()
            .filter_map(|p| assigned.get(p.as_str()).copied())
            .max();

        let target_idx = match existing {
            Some(idx) => {
                // A lower-priority single-path group already exists: upgrade
                // it in place and merge any other overlapping groups into it.
                let group = &mut groups[idx];
                group.kind = conflict.kind;
                for p in &mentioned {
                    if !group.paths.contains(p) {
                        group.paths.push(p.clone());
                    }
                }
                idx
            }
            None => {
                groups.push(ConflictGroup {
                    kind: conflict.kind,
                    paths: Vec::new(),
                    stage_blobs: BTreeMap::new(),
                });
                groups.len() - 1
            }
        };

        // Merge all groups touched by this conflict into the target group.
        let merge_into: Vec<usize> = mentioned
            .iter()
            .filter_map(|p| assigned.get(p.as_str()).copied())
            .filter(|idx| *idx != target_idx)
            .collect();
        for src in merge_into {
            let paths = std::mem::take(&mut groups[src].paths);
            let blobs = std::mem::take(&mut groups[src].stage_blobs);
            for p in paths {
                if !groups[target_idx].paths.contains(&p) {
                    groups[target_idx].paths.push(p.clone());
                }
                if let Some(b) = blobs.get(&p) {
                    groups[target_idx].stage_blobs.insert(p.clone(), b.clone());
                }
                assigned.insert(p, target_idx);
            }
        }

        for p in &mentioned {
            if !groups[target_idx].paths.contains(p) {
                groups[target_idx].paths.push(p.clone());
            }
            if let Some(b) = stage_map.get(p) {
                groups[target_idx].stage_blobs.insert(p.clone(), b.clone());
            }
            assigned.insert(p.clone(), target_idx);
        }
    }

    for path in conflicted {
        if assigned.contains_key(path.as_str()) {
            continue;
        }
        let mut group = ConflictGroup {
            kind: ConflictKind::Content,
            paths: vec![path.clone()],
            stage_blobs: BTreeMap::new(),
        };
        if let Some(stages) = stage_map.get(path) {
            group.stage_blobs.insert(path.clone(), stages.clone());
        }
        let idx = groups.len();
        groups.push(group);
        assigned.insert(path.clone(), idx);
    }

    groups.sort_by(|a, b| a.paths.first().cmp(&b.paths.first()));
    groups
}

/// Ensure each rename/file-location group is assigned to exactly one slice:
/// the first slice containing any group member absorbs all remaining members
/// (which are removed from any other slice).
pub fn expand_slices_to_groups(explicit: &mut [Vec<String>], groups: &[ConflictGroup]) {
    for group in groups.iter().filter(|g| !g.is_single()) {
        let Some(target) = group
            .paths
            .iter()
            .filter_map(|p| {
                explicit
                    .iter()
                    .position(|slice| slice.iter().any(|q| q == p))
            })
            .min()
        else {
            continue;
        };

        for p in &group.paths {
            for (i, slice) in explicit.iter_mut().enumerate() {
                if i == target {
                    if !slice.iter().any(|q| q == p) {
                        slice.push(p.clone());
                    }
                } else {
                    slice.retain(|q| q != p);
                }
            }
        }
    }

    for slice in explicit.iter_mut() {
        slice.sort();
        slice.dedup();
    }
}

// ── group-aware resolve decisions ───────────────────────────────────────────

/// A user-selectable way to settle an entire conflict group at resolve time.
/// `Manual` defers to the per-path merge-tool loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupAction {
    KeepOurs,
    TakeTheirs,
    KeepBothNames,
    AcceptSuggested,
    KeepOriginal,
    AcceptRename,
    KeepDeleted,
    Manual,
}

/// Group member roles derived from the index stages each member holds:
/// stage 1 = base, 2 = ours, 3 = theirs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GroupRoles {
    pub old: Option<String>,
    pub ours: Option<String>,
    pub theirs: Option<String>,
    pub unsighted: Vec<String>,
    pub both_sided: Vec<String>,
}

pub fn derive_group_roles(group: &ConflictGroup) -> GroupRoles {
    let mut roles = GroupRoles::default();
    for path in &group.paths {
        match group.stage_blobs.get(path) {
            None => roles.unsighted.push(path.clone()),
            Some(stages) => {
                let has2 = stages.contains_key(&2);
                let has3 = stages.contains_key(&3);
                if has2 && has3 {
                    roles.both_sided.push(path.clone());
                } else if has2 {
                    roles.ours = Some(path.clone());
                } else if has3 {
                    roles.theirs = Some(path.clone());
                } else {
                    roles.old = Some(path.clone());
                }
            }
        }
    }
    roles
}

fn stage_for(group: &ConflictGroup, path: &Option<String>, stage: usize) -> Option<StageBlob> {
    path.as_ref()
        .and_then(|p| group.stage_blobs.get(p))
        .and_then(|s| s.get(&stage))
        .cloned()
}

/// First path that carries a stage blob — the survivor/suggested side for
/// rename/delete and file-location groups.
fn sighted_member(group: &ConflictGroup) -> Option<String> {
    group
        .paths
        .iter()
        .find(|p| group.stage_blobs.contains_key(p.as_str()))
        .cloned()
}

/// Map a [`GroupMode`] to the concrete [`GroupAction`] for one group, if the
/// group's topology and stage roles support it. `None` means "fall through to
/// the per-file merge tool".
pub fn decide_group_action(group: &ConflictGroup, mode: GroupMode) -> Option<GroupAction> {
    if mode.is_tool() {
        return None;
    }

    let roles = derive_group_roles(group);
    let candidate = match mode {
        GroupMode::Tool => return None,
        GroupMode::Delete => GroupAction::KeepDeleted,
        GroupMode::Ours => match group.kind {
            ConflictKind::RenameRename => GroupAction::KeepOurs,
            ConflictKind::FileLocation => GroupAction::KeepOriginal,
            ConflictKind::RenameDelete => GroupAction::KeepDeleted,
            ConflictKind::ModifyDelete => {
                if roles.ours.is_some() {
                    GroupAction::KeepOurs
                } else {
                    return None;
                }
            }
            ConflictKind::Content => return None,
        },
        GroupMode::Theirs => match group.kind {
            ConflictKind::RenameRename => GroupAction::TakeTheirs,
            ConflictKind::FileLocation => GroupAction::AcceptSuggested,
            ConflictKind::RenameDelete => GroupAction::AcceptRename,
            ConflictKind::ModifyDelete => {
                if roles.theirs.is_some() {
                    GroupAction::TakeTheirs
                } else {
                    GroupAction::KeepDeleted
                }
            }
            ConflictKind::Content => return None,
        },
        GroupMode::Both => match group.kind {
            ConflictKind::RenameRename | ConflictKind::FileLocation => GroupAction::KeepBothNames,
            ConflictKind::RenameDelete => GroupAction::AcceptRename,
            ConflictKind::ModifyDelete => {
                if roles.ours.is_some() {
                    GroupAction::KeepOurs
                } else if roles.theirs.is_some() {
                    GroupAction::TakeTheirs
                } else {
                    GroupAction::KeepDeleted
                }
            }
            ConflictKind::Content => return None,
        },
    };

    // Deleting the whole group is valid for any rename/delete topology,
    // even when the interactive menu would not offer it.
    if mode == GroupMode::Delete {
        return matches!(
            group.kind,
            ConflictKind::ModifyDelete
                | ConflictKind::RenameDelete
                | ConflictKind::RenameRename
                | ConflictKind::FileLocation
        )
        .then_some(GroupAction::KeepDeleted);
    }

    group_actions(group)
        .iter()
        .any(|(action, _)| *action == candidate)
        .then_some(candidate)
}

fn quoted(path: &Option<String>) -> String {
    path.clone().unwrap_or_else(|| "(none)".to_string())
}

/// One-line description of a conflict group's derived roles, for prompts.
pub fn describe_group(group: &ConflictGroup) -> String {
    let roles = derive_group_roles(group);
    format!(
        "{} [{}] ours: {}, theirs: {}, base: {}",
        group.display_label(),
        group.kind.label(),
        quoted(&roles.ours),
        quoted(&roles.theirs),
        quoted(&roles.old),
    )
}

/// Offered resolutions for a group, each with a concrete label naming the
/// paths involved; the last entry is always `Manual` (fall through to the
/// per-path merge tool). Groups whose roles cannot be derived confidently
/// return an empty list (tool loop only).
pub fn group_actions(group: &ConflictGroup) -> Vec<(GroupAction, String)> {
    let roles = derive_group_roles(group);
    let mut out: Vec<(GroupAction, String)> = Vec::new();

    match group.kind {
        ConflictKind::RenameRename
            if roles.ours.is_some()
                && roles.theirs.is_some()
                && roles.old.is_some()
                && roles.unsighted.is_empty() =>
        {
            let ours = roles.ours.clone().unwrap_or_default();
            let theirs = roles.theirs.clone().unwrap_or_default();
            let old = roles.old.clone().unwrap_or_default();
            out.push((
                GroupAction::TakeTheirs,
                format!("Take THEIR rename '{theirs}' (drop '{ours}', drop old '{old}')"),
            ));
            out.push((
                GroupAction::KeepOurs,
                format!("Keep OUR rename '{ours}' (drop '{theirs}', drop old '{old}')"),
            ));
            out.push((
                GroupAction::KeepBothNames,
                format!("Keep BOTH names: '{ours}' + '{theirs}' (drop old '{old}')"),
            ));
        }
        ConflictKind::FileLocation => {
            let suggested = roles
                .ours
                .clone()
                .or_else(|| roles.theirs.clone())
                .or_else(|| sighted_member(group));
            let original = group
                .paths
                .iter()
                .find(|p| Some(*p) != suggested.as_ref())
                .cloned();
            if let (Some(suggested), Some(original)) = (suggested, original) {
                out.push((
                    GroupAction::AcceptSuggested,
                    format!("Accept git's suggestion: keep as '{suggested}' (drop '{original}')"),
                ));
                out.push((
                    GroupAction::KeepOriginal,
                    format!("Keep original location '{original}' (drop '{suggested}')"),
                ));
                out.push((
                    GroupAction::KeepBothNames,
                    format!("Keep BOTH locations: '{original}' + '{suggested}'"),
                ));
            }
        }
        ConflictKind::RenameDelete => {
            let new = roles
                .ours
                .clone()
                .or_else(|| roles.theirs.clone())
                .or_else(|| sighted_member(group));
            let old = roles.old.clone().or_else(|| {
                group
                    .paths
                    .iter()
                    .find(|p| Some(*p) != new.as_ref())
                    .cloned()
            });
            if let (Some(new), Some(old)) = (new, old) {
                out.push((
                    GroupAction::AcceptRename,
                    format!("Accept rename to '{new}' (drop old '{old}')"),
                ));
                out.push((
                    GroupAction::KeepDeleted,
                    format!("Keep file deleted (drop '{new}' and '{old}')"),
                ));
            }
        }
        ConflictKind::ModifyDelete => {
            let path = roles
                .ours
                .clone()
                .or_else(|| roles.theirs.clone())
                .or_else(|| group.paths.first().cloned());
            let Some(path) = path else { return Vec::new() };
            if roles.ours.is_some() {
                out.push((
                    GroupAction::KeepOurs,
                    format!("Keep our modified version of '{path}'"),
                ));
            } else if roles.theirs.is_some() {
                out.push((
                    GroupAction::TakeTheirs,
                    format!("Take their version of '{path}'"),
                ));
            }
            out.push((
                GroupAction::KeepDeleted,
                format!("Accept deletion of '{path}'"),
            ));
        }
        _ => {}
    }

    if out.is_empty() {
        return Vec::new();
    }
    out.push((
        GroupAction::Manual,
        "Run merge tool per file (manual)".to_string(),
    ));
    out
}

/// Stage the outcome of a group decision across every member path.
/// Returns the paths settled so the caller can skip them in the per-path
/// merge-tool loop.
pub fn apply_group_action(group: &ConflictGroup, action: GroupAction) -> Result<Vec<String>> {
    if action == GroupAction::Manual {
        return Ok(Vec::new());
    }

    fn write_side(
        settled: &mut Vec<String>,
        path: &Option<String>,
        blob: &Option<StageBlob>,
    ) -> Result<()> {
        let Some(path) = path else { return Ok(()) };
        let Some(blob) = blob else {
            return Err(anyhow::anyhow!(
                "no staged blob for '{path}' to resolve group decision"
            ));
        };
        git_ops::write_stage_to_staged_path(&blob.mode, &blob.oid, path)?;
        if !settled.contains(path) {
            settled.push(path.clone());
        }
        Ok(())
    }

    fn delete_side(settled: &mut Vec<String>, path: &str) -> Result<()> {
        git_ops::resolve_path_as_deleted(path)?;
        if !settled.contains(&path.to_string()) {
            settled.push(path.to_string());
        }
        Ok(())
    }

    let roles = derive_group_roles(group);
    let ours_blob = stage_for(group, &roles.ours, 2);
    let theirs_blob = stage_for(group, &roles.theirs, 3);
    let fallback_blob = || -> Option<StageBlob> {
        ours_blob
            .clone()
            .or_else(|| theirs_blob.clone())
            .or_else(|| {
                sighted_member(group).and_then(|p| {
                    group
                        .stage_blobs
                        .get(&p)
                        .and_then(|s| s.get(&2).or(s.get(&3)))
                        .cloned()
                })
            })
    };

    let mut settled: Vec<String> = Vec::new();

    match action {
        GroupAction::KeepOurs => {
            write_side(&mut settled, &roles.ours, &ours_blob)?;
            if let Some(p) = &roles.theirs {
                delete_side(&mut settled, p)?;
            }
            if let Some(p) = &roles.old {
                delete_side(&mut settled, p)?;
            }
        }
        GroupAction::TakeTheirs => {
            write_side(&mut settled, &roles.theirs, &theirs_blob)?;
            if let Some(p) = &roles.ours {
                delete_side(&mut settled, p)?;
            }
            if let Some(p) = &roles.old {
                delete_side(&mut settled, p)?;
            }
        }
        GroupAction::KeepBothNames => {
            write_side(&mut settled, &roles.ours, &ours_blob)?;
            write_side(&mut settled, &roles.theirs, &theirs_blob)?;
            if let Some(p) = &roles.old {
                delete_side(&mut settled, p)?;
            }
            for extra in &roles.unsighted {
                // File-location groups: offer the SAME content at both
                // locations. Other kinds: unsighted members get dropped.
                if group.kind == ConflictKind::FileLocation {
                    write_side(&mut settled, &Some(extra.clone()), &fallback_blob())?;
                } else {
                    delete_side(&mut settled, extra)?;
                }
            }
        }
        GroupAction::AcceptSuggested | GroupAction::AcceptRename | GroupAction::KeepOriginal => {
            let survivor_path = if action == GroupAction::KeepOriginal {
                roles
                    .unsighted
                    .first()
                    .cloned()
                    .or_else(|| group.paths.first().cloned())
            } else {
                roles
                    .ours
                    .clone()
                    .or_else(|| roles.theirs.clone())
                    .or_else(|| sighted_member(group))
            };
            let content_blob = fallback_blob();
            write_side(&mut settled, &survivor_path, &content_blob)?;

            let dropped = if action == GroupAction::KeepOriginal {
                roles.ours.clone().or_else(|| roles.theirs.clone())
            } else if roles.old.is_some() {
                roles.old.clone()
            } else {
                roles.unsighted.first().cloned()
            };
            if let Some(p) = &dropped {
                delete_side(&mut settled, p)?;
            }
        }
        GroupAction::KeepDeleted => {
            for path in &group.paths {
                delete_side(&mut settled, path)?;
            }
        }
        GroupAction::Manual => return Ok(Vec::new()),
    }

    Ok(settled)
}

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

/// Conflicted paths that no explicit slice group covers, in conflict-list order.
pub fn unassigned_paths<'a>(
    all_conflicts: &'a [String],
    explicit_slices: &[Vec<String>],
) -> Vec<&'a String> {
    let assigned = explicit_slices
        .iter()
        .flatten()
        .map(|path| path.as_str())
        .collect::<std::collections::BTreeSet<_>>();

    all_conflicts
        .iter()
        .filter(|path| !assigned.contains(path.as_str()))
        .collect()
}

/// The blob to materialize for `path` when it belongs to a
/// file-location conflict group: content captured from the unmerged index
/// stage, since the path exists neither on the source ref nor at the slice
/// base. Returns None for ordinary groups, which use source-restore/rm.
fn file_location_blob(groups: &[ConflictGroup], path: &str) -> Option<StageBlob> {
    groups
        .iter()
        .find(|g| g.kind == ConflictKind::FileLocation && g.contains(path))
        .and_then(|g| {
            g.paths
                .iter()
                .filter_map(|p| g.stage_blobs.get(p))
                .find_map(|stages| stages.get(&2).or(stages.get(&3)).cloned())
        })
}

/// Precomputed per-run data shared by all slice creations, so huge conflict
/// sets need one tree walk per chunk instead of git subprocesses per path.
pub struct SliceInputs {
    /// Blob entries of every slice-relevant path in the source ref.
    src_entries: std::collections::BTreeMap<String, (String, String)>,
    /// Blob entries in the slice base tree.
    base_entries: std::collections::BTreeMap<String, (String, String)>,
    /// Last commit touching each path on the source side (trailers).
    provenance: std::collections::BTreeMap<String, git_ops::PathTouched>,
    /// Linked worktrees exist; checkout must manage them per slice.
    linked_worktrees: bool,
}

impl SliceInputs {
    fn checkout_slice_branch(&self, branch: &str, at: &str) -> Result<()> {
        if self.linked_worktrees {
            git_ops::checkout_new_or_reset(branch, at)
        } else {
            git_ops::checkout_new_or_reset_light(branch, at)
        }
    }
}

/// Plan how to materialize `paths` on a branch checked out at `slice_base`:
/// classify each into restore/rm/staged-blob and detect whether the slice
/// would contain any change at all. `None` inputs fall back to per-path
/// probing (small conflict sets, e.g. group decisions).
struct SlicePlan {
    changed: bool,
    restores: Vec<String>,
    deletes: Vec<String>,
    locations: Vec<(String, String, String)>,
}

fn plan_slice_paths(
    paths: &[String],
    groups: &[ConflictGroup],
    inputs: &SliceInputs,
) -> Result<SlicePlan> {
    let mut plan = SlicePlan {
        changed: false,
        restores: Vec::new(),
        deletes: Vec::new(),
        locations: Vec::new(),
    };
    for path in paths {
        if let Some(blob) = file_location_blob(groups, path) {
            plan.changed |= inputs
                .base_entries
                .get(path)
                .is_none_or(|e| e.1 != blob.oid);
            plan.locations
                .push((path.clone(), blob.mode.clone(), blob.oid.clone()));
            continue;
        }
        match inputs.src_entries.get(path) {
            Some(entry) => {
                if inputs.base_entries.get(path) != Some(entry) {
                    plan.changed = true;
                }
                plan.restores.push(path.clone());
            }
            None => {
                if inputs.base_entries.contains_key(path) {
                    plan.changed = true;
                    plan.deletes.push(path.clone());
                }
            }
        }
    }
    Ok(plan)
}

/// Apply a slice plan on the currently checked-out slice branch.
fn apply_slice_plan(plan: &SlicePlan, source_ref: &str) -> Result<()> {
    for (path, mode, oid) in &plan.locations {
        git_ops::write_stage_to_staged_path(mode, oid, path)?;
    }
    if !plan.restores.is_empty() {
        git_ops::restore_paths_from_ref(source_ref, &plan.restores)?;
    }
    if !plan.deletes.is_empty() {
        git_ops::rm_paths_batch(&plan.deletes)?;
    }
    Ok(())
}

/// Create one slice branch carrying `paths` (source-side content) and commit it
/// with per-path provenance trailers. Used for explicit groups and for the
/// combined unassigned slice. With precomputed `inputs` (bulk mode) the
/// materialization and change detection need no per-path subprocesses.
fn create_group_slice_branch(
    integration_branch: &str,
    slice_base: &str,
    source_ref: &str,
    source_sha: &str,
    slice_number: usize,
    paths: &[String],
    description: &str,
    groups: &[ConflictGroup],
    inputs: &SliceInputs,
) -> Result<()> {
    let slice_branch = slice_branch_name(integration_branch, slice_number)?;

    let plan = plan_slice_paths(paths, groups, inputs)?;
    if !plan.changed {
        color::print_warning(&format!("Skipped {slice_branch}: no staged changes"), None);
        return Ok(());
    }

    inputs.checkout_slice_branch(&slice_branch, slice_base)?;
    apply_slice_plan(&plan, source_ref)?;

    let trailers = {
        let mut t = vec![
            format!("Source-Ref: {source_ref}"),
            format!("Source-Commit: {source_sha}"),
            format!("Slice-Paths: {}", paths.join(", ")),
        ];

        for path in paths {
            let (path_commit, author_name, author_email) =
                match inputs.provenance.get(path).map(|touch| {
                    (
                        Some(touch.commit.clone()),
                        Some(touch.author_name.clone()),
                        Some(touch.author_email.clone()),
                    )
                }) {
                    Some(triple) => triple,
                    None => {
                        let p = git_ops::path_provenance(source_ref, source_sha, path)?;
                        (p.path_commit, p.author_name, p.author_email)
                    }
                };
            t.push(format!("Source-Path: {path}"));
            t.push(format!(
                "Source-Path-Commit: {}",
                path_commit.unwrap_or_else(|| "(none)".to_string())
            ));
            if let (Some(name), Some(email)) = (author_name, author_email) {
                t.push(format!("Co-authored-by: {name} <{email}>"));
            }
        }

        t.join("\n")
    };

    let files_list = paths
        .iter()
        .map(|p| format!("* {p}"))
        .collect::<Vec<_>>()
        .join("\n");

    let message = format!(
        "Mergetopus - slice{slice_number} from {source_ref} (theirs)\n\nFiles:\n{files_list}\n\n{trailers}"
    );

    git_ops::commit(&message)?;
    color::print_success(
        &format!(
            "Created {description} slice branch {slice_branch} for {} file(s)",
            paths.len()
        ),
        None,
    );
    Ok(())
}

pub fn create_slice_branches(
    integration_branch: &str,
    slice_base: &str,
    source_ref: &str,
    source_sha: &str,
    all_conflicts: &[String],
    explicit_slices: &[Vec<String>],
    unassigned_policy: UnassignedPolicy,
    groups: &[ConflictGroup],
) -> Result<()> {
    // Bulk precomputation: one bounded tree listing for source and base plus
    // one streamed log walk for provenance, replacing ~4 git subprocesses per
    // conflicted path in the old per-path loops.
    let all_paths: Vec<String> = {
        let mut set = std::collections::BTreeSet::new();
        set.extend(all_conflicts.iter().cloned());
        for g in explicit_slices {
            set.extend(g.iter().cloned());
        }
        set.into_iter().collect()
    };
    let inputs = SliceInputs {
        src_entries: git_ops::tree_entries(source_ref, &all_paths)?,
        base_entries: git_ops::tree_entries(slice_base, &all_paths)?,
        provenance: git_ops::paths_last_touched(source_sha, &all_paths)?,
        linked_worktrees: git_ops::has_linked_worktrees()?,
    };

    let mut slice_index = 1usize;

    for group in explicit_slices {
        if group.is_empty() {
            continue;
        }

        create_group_slice_branch(
            integration_branch,
            slice_base,
            source_ref,
            source_sha,
            slice_index,
            group,
            "explicit",
            groups,
            &inputs,
        )?;
        slice_index += 1;
    }

    let leftovers = unassigned_paths(all_conflicts, explicit_slices);

    if leftovers.is_empty() {
        return Ok(());
    }

    let in_leftovers = |path: &str| leftovers.iter().any(|p| p.as_str() == path);
    let leftover_groups: Vec<&ConflictGroup> = groups
        .iter()
        .filter(|g| !g.is_single() && g.paths.iter().any(|p| in_leftovers(p)))
        .collect();

    let mut handled: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for g in &leftover_groups {
        for p in &g.paths {
            handled.insert(p.clone());
        }
    }

    if !unassigned_policy.is_separate() {
        let mut paths: Vec<String> = Vec::new();
        for g in &leftover_groups {
            for p in &g.paths {
                if !paths.contains(p) {
                    paths.push(p.clone());
                }
            }
        }
        for path in &leftovers {
            if handled.contains(path.as_str()) {
                continue;
            }
            let path = (*path).clone();
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
        create_group_slice_branch(
            integration_branch,
            slice_base,
            source_ref,
            source_sha,
            slice_index,
            &paths,
            "unassigned",
            groups,
            &inputs,
        )?;
        return Ok(());
    }

    // Multi-path groups stay together: one slice per group.
    for g in &leftover_groups {
        create_group_slice_branch(
            integration_branch,
            slice_base,
            source_ref,
            source_sha,
            slice_index,
            &g.paths,
            "default",
            groups,
            &inputs,
        )?;
        slice_index += 1;
    }

    for path in leftovers {
        let path = path.to_string();
        if handled.contains(&path) {
            continue;
        }

        let slice_number = slice_index;
        let slice_branch = slice_branch_name(integration_branch, slice_index)?;
        slice_index += 1;

        let paths = vec![path.clone()];
        let plan = plan_slice_paths(&paths, groups, &inputs)?;
        if !plan.changed {
            color::print_warning(
                &format!("Skipped {slice_branch} for {path}: no staged changes"),
                None,
            );
            continue;
        }

        inputs.checkout_slice_branch(&slice_branch, slice_base)?;
        apply_slice_plan(&plan, source_ref)?;

        let provenance = match inputs.provenance.get(&path) {
            Some(touch) => crate::models::PathProvenance {
                source_ref: source_ref.to_string(),
                source_commit: source_sha.to_string(),
                path: path.clone(),
                path_commit: Some(touch.commit.clone()),
                author_name: Some(touch.author_name.clone()),
                author_email: Some(touch.author_email.clone()),
                author_date: Some(touch.author_date.clone()),
            },
            None => git_ops::path_provenance(source_ref, source_sha, &path)?,
        };

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

            if let (Some(name), Some(email)) = (&provenance.author_name, &provenance.author_email) {
                t.push(format!("Co-authored-by: {name} <{email}>"));
            }

            t.join("\n")
        };

        let message = format!(
            "Mergetopus - slice{slice_number} from {source_ref} (theirs)\n\nFiles:\n* {path}\n\n{trailers}"
        );

        git_ops::commit_slice(&message, &provenance)?;
        color::print_success(
            &format!("Created default single-file slice branch {slice_branch} for {path}"),
            None,
        );
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

    type TestResult<T> = Result<T, Box<dyn std::error::Error>>;
    use crate::test_support as test_helpers;

    #[test]
    fn sanitize_fragment_keeps_safe_chars() {
        // Characters that are replaced get a short disambiguation hash suffix.
        let a = sanitize_branch_fragment("feature/refactor-auth");
        assert!(
            a.starts_with("feature_refactor-auth_"),
            "expected hash suffix, got {a}"
        );
        let b = sanitize_branch_fragment("release 1.0");
        assert!(
            b.starts_with("release_1.0_"),
            "expected hash suffix, got {b}"
        );
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
        assert_eq!(sanitize_branch_fragment("feature/foo"), "feature_foo_fa2d",);
        assert_eq!(sanitize_branch_fragment("feature:foo"), "feature_foo_fa42",);
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
        assert_eq!(
            suffix.len(),
            4,
            "hash suffix should be 4 hex chars, got '{suffix}'"
        );
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

    #[test]
    fn unassigned_paths_excludes_explicit_group_members() {
        let conflicts = vec![
            "a.txt".to_string(),
            "b.txt".to_string(),
            "c.txt".to_string(),
        ];
        let explicit = vec![vec!["b.txt".to_string()]];

        let leftovers = unassigned_paths(&conflicts, &explicit);

        assert_eq!(leftovers, vec!["a.txt", "c.txt"]);
    }

    #[test]
    fn unassigned_paths_keeps_conflict_order() {
        let conflicts = vec!["z.txt".to_string(), "a.txt".to_string()];
        let explicit = Vec::new();

        let leftovers = unassigned_paths(&conflicts, &explicit);

        assert_eq!(leftovers, vec!["z.txt", "a.txt"]);
    }

    #[test]
    fn filter_conflict_lines_drops_settled_conflicts() {
        let out = "Auto-merging b.txt\nCONFLICT (rename/rename): src/a.txt renamed to m.txt in HEAD and to f.txt in feature.\nCONFLICT (content): Merge conflict in b.txt";
        let remaining = vec!["b.txt".to_string()];
        let filtered = filter_conflict_lines(out, &remaining);
        assert!(
            !filtered.contains("rename/rename"),
            "settled rename group must be dropped:\n{filtered}"
        );
        assert!(
            filtered.contains("content") && filtered.contains("Auto-merging b.txt"),
            "lines for remaining paths and context must survive:\n{filtered}"
        );
    }

    #[test]
    fn unassigned_paths_empty_when_everything_assigned() {
        let conflicts = vec!["a.txt".to_string()];
        let explicit = vec![vec!["a.txt".to_string()]];

        assert!(unassigned_paths(&conflicts, &explicit).is_empty());
    }

    #[test]
    fn unassigned_policy_parses_modes() {
        use std::str::FromStr;
        assert_eq!(
            UnassignedPolicy::from_str("separate").unwrap(),
            UnassignedPolicy::Separate
        );
        assert_eq!(
            UnassignedPolicy::from_str("SINGLE").unwrap(),
            UnassignedPolicy::Single
        );
        assert!(UnassignedPolicy::from_str("bogus").is_err());
        assert_eq!(UnassignedPolicy::default(), UnassignedPolicy::Separate);
        assert!(UnassignedPolicy::Separate.is_separate());
        assert!(!UnassignedPolicy::Single.is_separate());
    }

    fn stages(
        entries: &[(&str, &[(usize, &str)])],
    ) -> BTreeMap<String, BTreeMap<usize, StageBlob>> {
        entries
            .iter()
            .map(|(path, stages)| {
                (
                    path.to_string(),
                    stages
                        .iter()
                        .map(|(s, oid)| {
                            (
                                *s,
                                StageBlob {
                                    mode: "100644".to_string(),
                                    oid: oid.to_string(),
                                },
                            )
                        })
                        .collect::<BTreeMap<_, _>>(),
                )
            })
            .collect()
    }

    #[test]
    fn parse_conflict_lines_extracts_rename_rename() {
        let out = "Automatic merge failed; fix conflicts and then commit the result.\nCONFLICT (rename/rename): src/a.txt renamed to renamed_main.txt in HEAD and to renamed_feature.txt in feature.";
        let parsed = parse_conflict_lines(out);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].kind, ConflictKind::RenameRename);
        assert_eq!(
            parsed[0].paths,
            vec!["src/a.txt", "renamed_main.txt", "renamed_feature.txt"]
        );
    }

    #[test]
    fn parse_conflict_lines_extracts_file_location() {
        let out = "CONFLICT (file location): src/new.txt added in HEAD inside a directory that was renamed in feature, suggesting it should perhaps be moved to dst/new.txt.";
        let parsed = parse_conflict_lines(out);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].kind, ConflictKind::FileLocation);
        assert_eq!(parsed[0].paths, vec!["src/new.txt", "dst/new.txt"]);
    }

    #[test]
    fn parse_conflict_lines_extracts_rename_delete_and_modify_delete() {
        let out = "CONFLICT (rename/delete): old.txt renamed to new.txt in feature, but deleted in HEAD.\nCONFLICT (modify/delete): mod.txt deleted in feature and modified in HEAD. Version HEAD of mod.txt left in tree.";
        let parsed = parse_conflict_lines(out);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].kind, ConflictKind::RenameDelete);
        assert_eq!(parsed[0].paths, vec!["old.txt", "new.txt"]);
        assert_eq!(parsed[1].kind, ConflictKind::ModifyDelete);
        assert_eq!(parsed[1].paths, vec!["mod.txt"]);
    }

    #[test]
    fn build_conflict_groups_joins_rename_rename_paths() {
        let conflicted = vec![
            "src/a.txt".to_string(),
            "renamed_main.txt".to_string(),
            "renamed_feature.txt".to_string(),
            "plain.txt".to_string(),
        ];
        let stage_map = stages(&[
            ("src/a.txt", &[(1, "aaa")]),
            ("renamed_main.txt", &[(2, "aaa")]),
            ("renamed_feature.txt", &[(3, "bbb")]),
            ("plain.txt", &[(1, "ccc"), (2, "ddd"), (3, "eee")]),
        ]);
        let merge_out = "CONFLICT (rename/rename): src/a.txt renamed to renamed_main.txt in HEAD and to renamed_feature.txt in feature.\nCONFLICT (content): Merge conflict in plain.txt.";

        let groups = build_conflict_groups(merge_out, &conflicted, &stage_map);
        assert_eq!(groups.len(), 2);

        let rename = groups
            .iter()
            .find(|g| g.kind == ConflictKind::RenameRename)
            .expect("rename group");
        assert_eq!(rename.paths.len(), 3);
        assert!(rename.contains("src/a.txt"));
        assert!(rename.contains("renamed_feature.txt"));
        assert!(rename.stage_blobs.contains_key("renamed_feature.txt"));

        let plain = groups
            .iter()
            .find(|g| g.contains("plain.txt"))
            .expect("plain");
        assert!(plain.is_single());
        assert_eq!(plain.kind, ConflictKind::Content);
    }

    #[test]
    fn build_conflict_groups_file_location_keeps_non_unmerged_member() {
        let conflicted = vec!["dst/new.txt".to_string()];
        let stage_map = stages(&[("dst/new.txt", &[(2, "abc")])]);
        let merge_out = "CONFLICT (file location): src/new.txt added in HEAD inside a directory that was renamed in feature, suggesting it should perhaps be moved to dst/new.txt.";

        let groups = build_conflict_groups(merge_out, &conflicted, &stage_map);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].kind, ConflictKind::FileLocation);
        assert_eq!(groups[0].paths, vec!["src/new.txt", "dst/new.txt"]);
    }

    #[test]
    fn unparseable_output_falls_back_to_single_path_groups() {
        let conflicted = vec!["a.txt".to_string(), "b.txt".to_string()];
        let groups = build_conflict_groups("", &conflicted, &stages(&[]));
        assert_eq!(groups.len(), 2);
        assert!(groups.iter().all(|g| g.is_single()));
    }

    #[test]
    fn derive_group_roles_from_rename_rename_stages() {
        let group = ConflictGroup {
            kind: ConflictKind::RenameRename,
            paths: vec![
                "src/a.txt".to_string(),
                "renamed_main.txt".to_string(),
                "renamed_feature.txt".to_string(),
            ],
            stage_blobs: stages(&[
                ("src/a.txt", &[(1, "aaa")]),
                ("renamed_main.txt", &[(2, "aaa")]),
                ("renamed_feature.txt", &[(3, "bbb")]),
            ]),
        };
        let roles = derive_group_roles(&group);
        assert_eq!(roles.old.as_deref(), Some("src/a.txt"));
        assert_eq!(roles.ours.as_deref(), Some("renamed_main.txt"));
        assert_eq!(roles.theirs.as_deref(), Some("renamed_feature.txt"));
        assert!(roles.unsighted.is_empty() && roles.both_sided.is_empty());
    }

    #[test]
    fn derive_group_roles_file_location_original_is_unsighted() {
        let group = ConflictGroup {
            kind: ConflictKind::FileLocation,
            paths: vec!["src/new.txt".to_string(), "dst/new.txt".to_string()],
            stage_blobs: stages(&[("dst/new.txt", &[(2, "abc")])]),
        };
        let roles = derive_group_roles(&group);
        assert_eq!(roles.ours.as_deref(), Some("dst/new.txt"));
        assert_eq!(roles.unsighted, vec!["src/new.txt"]);
        assert!(roles.old.is_none());
    }

    #[test]
    fn group_actions_lists_for_each_kind() {
        let rename = ConflictGroup {
            kind: ConflictKind::RenameRename,
            paths: vec![
                "src/a.txt".to_string(),
                "renamed_main.txt".to_string(),
                "renamed_feature.txt".to_string(),
            ],
            stage_blobs: stages(&[
                ("src/a.txt", &[(1, "aaa")]),
                ("renamed_main.txt", &[(2, "aaa")]),
                ("renamed_feature.txt", &[(3, "bbb")]),
            ]),
        };
        let acts = group_actions(&rename);
        assert_eq!(
            acts.iter().map(|a| a.0).collect::<Vec<_>>(),
            vec![
                GroupAction::TakeTheirs,
                GroupAction::KeepOurs,
                GroupAction::KeepBothNames,
                GroupAction::Manual,
            ]
        );

        let content = ConflictGroup {
            kind: ConflictKind::Content,
            paths: vec!["x.txt".to_string()],
            stage_blobs: stages(&[]),
        };
        assert!(group_actions(&content).is_empty());
    }

    #[test]
    fn group_actions_modify_delete_covers_deletion_side() {
        // UD: base + ours stages, deleted in theirs.
        let group = ConflictGroup {
            kind: ConflictKind::ModifyDelete,
            paths: vec!["f.txt".to_string()],
            stage_blobs: stages(&[("f.txt", &[(1, "b"), (2, "o")])]),
        };
        let acts = group_actions(&group);
        assert_eq!(
            acts.iter().map(|a| a.0).collect::<Vec<_>>(),
            vec![
                GroupAction::KeepOurs,
                GroupAction::KeepDeleted,
                GroupAction::Manual
            ]
        );

        // DU: base + theirs stages.
        let group = ConflictGroup {
            kind: ConflictKind::ModifyDelete,
            paths: vec!["f.txt".to_string()],
            stage_blobs: stages(&[("f.txt", &[(1, "b"), (3, "t")])]),
        };
        let acts = group_actions(&group);
        assert_eq!(
            acts.iter().map(|a| a.0).collect::<Vec<_>>(),
            vec![
                GroupAction::TakeTheirs,
                GroupAction::KeepDeleted,
                GroupAction::Manual
            ]
        );
    }

    fn make_rename_rename_merge(repo: &std::path::Path) -> TestResult<Vec<String>> {
        // Keep content highly similar on both sides so git reports
        // rename/rename rather than unrelated add+delete.
        let base = "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\n";
        test_helpers::write_file(repo, "src/a.txt", base)?;
        test_helpers::commit_all(repo, "base")?;
        test_helpers::git(repo, &["checkout", "-b", "feature"])?;
        test_helpers::git(repo, &["mv", "src/a.txt", "renamed_feature.txt"])?;
        test_helpers::git(repo, &["commit", "-m", "feature renames"])?;
        test_helpers::git(repo, &["checkout", "main"])?;
        test_helpers::git(repo, &["mv", "src/a.txt", "renamed_main.txt"])?;
        test_helpers::git(repo, &["commit", "-m", "main renames differently"])?;
        test_helpers::git(repo, &["merge", "--no-ff", "--no-commit", "feature"]).ok();
        let conflicted =
            test_helpers::with_repo_cwd(repo, super::super::git_ops::conflicted_files)?;
        Ok(conflicted)
    }

    #[test]
    fn apply_take_theirs_settles_rename_rename_group() -> TestResult<()> {
        let repo = test_helpers::init_repo()?;
        let conflicted = make_rename_rename_merge(&repo)?;
        let stage_map =
            test_helpers::with_repo_cwd(&repo, super::super::git_ops::conflict_stage_map)?;

        let msg = "CONFLICT (rename/rename): src/a.txt renamed to renamed_main.txt in HEAD and to renamed_feature.txt in feature.";
        let groups = build_conflict_groups(msg, &conflicted, &stage_map);
        let group = groups
            .iter()
            .find(|g| g.kind == ConflictKind::RenameRename)
            .expect("rename group");

        let settled = test_helpers::with_repo_cwd(&repo, || {
            apply_group_action(group, GroupAction::TakeTheirs)
        })?;
        assert_eq!(settled.len(), 3);

        let still = test_helpers::with_repo_cwd(&repo, super::super::git_ops::conflicted_files)?;
        assert!(still.is_empty(), "group fully settled, leftover: {still:?}");
        assert!(repo.join("renamed_feature.txt").exists());
        assert!(!repo.join("renamed_main.txt").exists());
        assert!(!repo.join("src/a.txt").exists());
        // Assert the staged blob, not the worktree copy: on hosts with
        // core.autocrlf=true the checkout smudge turns LF into CRLF while
        // the index holds the exact blob.
        let content = test_helpers::git(&repo, &["show", ":renamed_feature.txt"])?;
        assert!(content.starts_with("l1\n"), "content: {content}");
        Ok(())
    }

    #[test]
    fn apply_keep_ours_settles_rename_rename_group() -> TestResult<()> {
        let repo = test_helpers::init_repo()?;
        let conflicted = make_rename_rename_merge(&repo)?;
        let stage_map =
            test_helpers::with_repo_cwd(&repo, super::super::git_ops::conflict_stage_map)?;
        let msg = "CONFLICT (rename/rename): src/a.txt renamed to renamed_main.txt in HEAD and to renamed_feature.txt in feature.";
        let groups = build_conflict_groups(msg, &conflicted, &stage_map);
        let group = groups
            .iter()
            .find(|g| g.kind == ConflictKind::RenameRename)
            .expect("rename group");

        test_helpers::with_repo_cwd(&repo, || apply_group_action(group, GroupAction::KeepOurs))?;

        let still = test_helpers::with_repo_cwd(&repo, super::super::git_ops::conflicted_files)?;
        assert!(still.is_empty(), "leftover: {still:?}");
        assert!(repo.join("renamed_main.txt").exists());
        assert!(!repo.join("renamed_feature.txt").exists());
        let content = test_helpers::git(&repo, &["show", ":renamed_main.txt"])?;
        assert!(content.starts_with("l1\n"), "content: {content}");
        Ok(())
    }

    #[test]
    fn apply_keep_both_names_keeps_both_and_drops_old() -> TestResult<()> {
        let repo = test_helpers::init_repo()?;
        let conflicted = make_rename_rename_merge(&repo)?;
        let stage_map =
            test_helpers::with_repo_cwd(&repo, super::super::git_ops::conflict_stage_map)?;
        let msg = "CONFLICT (rename/rename): src/a.txt renamed to renamed_main.txt in HEAD and to renamed_feature.txt in feature.";
        let groups = build_conflict_groups(msg, &conflicted, &stage_map);
        let group = groups
            .iter()
            .find(|g| g.kind == ConflictKind::RenameRename)
            .expect("rename group");

        test_helpers::with_repo_cwd(&repo, || {
            apply_group_action(group, GroupAction::KeepBothNames)
        })?;

        let still = test_helpers::with_repo_cwd(&repo, super::super::git_ops::conflicted_files)?;
        assert!(still.is_empty(), "leftover: {still:?}");
        assert!(repo.join("renamed_main.txt").exists());
        assert!(repo.join("renamed_feature.txt").exists());
        assert!(!repo.join("src/a.txt").exists());
        Ok(())
    }

    fn rr_group() -> ConflictGroup {
        ConflictGroup {
            kind: ConflictKind::RenameRename,
            paths: vec![
                "src/a.txt".to_string(),
                "renamed_main.txt".to_string(),
                "renamed_feature.txt".to_string(),
            ],
            stage_blobs: stages(&[
                ("src/a.txt", &[(1, "aaa")]),
                ("renamed_main.txt", &[(2, "aaa")]),
                ("renamed_feature.txt", &[(3, "bbb")]),
            ]),
        }
    }

    #[test]
    fn decide_group_action_maps_modes_for_rename_rename() {
        use crate::models::GroupMode;
        let g = rr_group();
        assert_eq!(
            decide_group_action(&g, GroupMode::Theirs),
            Some(GroupAction::TakeTheirs)
        );
        assert_eq!(
            decide_group_action(&g, GroupMode::Ours),
            Some(GroupAction::KeepOurs)
        );
        assert_eq!(
            decide_group_action(&g, GroupMode::Both),
            Some(GroupAction::KeepBothNames)
        );
        assert_eq!(
            decide_group_action(&g, GroupMode::Delete),
            Some(GroupAction::KeepDeleted)
        );
        assert_eq!(decide_group_action(&g, GroupMode::Tool), None);
    }

    #[test]
    fn decide_group_action_content_groups_fall_through() {
        use crate::models::GroupMode;
        let content = ConflictGroup {
            kind: ConflictKind::Content,
            paths: vec!["x.txt".to_string()],
            stage_blobs: stages(&[("x.txt", &[(1, "a"), (2, "b"), (3, "c")])]),
        };
        assert_eq!(decide_group_action(&content, GroupMode::Theirs), None);
        assert_eq!(decide_group_action(&content, GroupMode::Ours), None);
        assert_eq!(decide_group_action(&content, GroupMode::Both), None);
        assert_eq!(decide_group_action(&content, GroupMode::Delete), None);
    }

    #[test]
    fn decide_group_action_file_location_ours_is_original() {
        use crate::models::GroupMode;
        let g = ConflictGroup {
            kind: ConflictKind::FileLocation,
            paths: vec!["src/new.txt".to_string(), "dst/new.txt".to_string()],
            stage_blobs: stages(&[("dst/new.txt", &[(2, "abc")])]),
        };
        assert_eq!(
            decide_group_action(&g, GroupMode::Ours),
            Some(GroupAction::KeepOriginal)
        );
        assert_eq!(
            decide_group_action(&g, GroupMode::Theirs),
            Some(GroupAction::AcceptSuggested)
        );
        assert_eq!(
            decide_group_action(&g, GroupMode::Both),
            Some(GroupAction::KeepBothNames)
        );
    }

    #[test]
    fn decide_group_action_modify_delete_uses_surviving_side() {
        use crate::models::GroupMode;
        // UD (ours content, theirs deleted)
        let ud = ConflictGroup {
            kind: ConflictKind::ModifyDelete,
            paths: vec!["f.txt".to_string()],
            stage_blobs: stages(&[("f.txt", &[(1, "b"), (2, "o")])]),
        };
        assert_eq!(
            decide_group_action(&ud, GroupMode::Theirs),
            Some(GroupAction::KeepDeleted)
        );
        assert_eq!(
            decide_group_action(&ud, GroupMode::Ours),
            Some(GroupAction::KeepOurs)
        );
        assert_eq!(
            decide_group_action(&ud, GroupMode::Both),
            Some(GroupAction::KeepOurs)
        );

        // DU (theirs content, ours deleted)
        let du = ConflictGroup {
            kind: ConflictKind::ModifyDelete,
            paths: vec!["f.txt".to_string()],
            stage_blobs: stages(&[("f.txt", &[(1, "b"), (3, "t")])]),
        };
        assert_eq!(
            decide_group_action(&du, GroupMode::Theirs),
            Some(GroupAction::TakeTheirs)
        );
        assert_eq!(decide_group_action(&du, GroupMode::Ours), None);
    }

    #[test]
    fn expand_slices_absorbs_group_members() {
        let groups = vec![ConflictGroup {
            kind: ConflictKind::RenameRename,
            paths: vec![
                "src/a.txt".to_string(),
                "renamed_main.txt".to_string(),
                "renamed_feature.txt".to_string(),
            ],
            stage_blobs: BTreeMap::new(),
        }];
        let mut explicit = vec![
            vec!["src/a.txt".to_string()],
            vec!["renamed_feature.txt".to_string()],
        ];

        expand_slices_to_groups(&mut explicit, &groups);

        // All members land in the lowest-numbered owning slice; the other is emptied.
        assert_eq!(explicit[0].len(), 3);
        assert!(explicit[1].is_empty());
    }
}
