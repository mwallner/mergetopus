# Plan: Rename/Delete Edge Cases in Conflict Slicing

**Status:** implemented (Phases 1-4). Tests: `tests/test_suite_f.rs`,
`src/git_ops/mod.rs` (restore_ours + blob-sentinel unit tests),
`src/planner.rs` (parser/grouping/expand unit tests).
**Not committed** — working-tree changes only.

## Root causes (verified experimentally)

| # | Scenario | Git behavior | Mergetopus behavior today |
|---|----------|--------------|---------------------------|
| 1a | rename/rename 1→2 | 3 unmerged paths: old path stage-1 only (`DD`), ours path stage 2, theirs path stage 3 | **Crash** in `cmd_merge_workflow` restore_ours loop: `git restore --source=HEAD` fails with `path is unmerged` for paths without an ours stage |
| 1b | modify/delete, deleted by **target** | unmerged path, stages 1+3 (`DU`) | **Crash**, same call site |
| 1c | modify/delete, deleted by **source** | unmerged path, stages 1+2 (`UD`) | Works: planner's `path_exists_in_ref==false` → `git rm` deletion slice |
| 2 | deleted side vs zero-byte side in resolve | — | `write_blob_to_path` writes an **empty file** for a missing path; merge tool cannot distinguish "deleted" from "empty" (`cmd_resolve.rs` LOCAL/BASE/REMOTE temps) |
| 3 | dir-rename "file location" conflict (ours adds under dir theirs renamed) | unmerged path at the *suggested new* location, absent from HEAD | **Crash** (same call site). If slicing ever proceeded, planner would `git rm` a path that doesn't exist at the slice base → "Skipped: no staged changes" → **user file silently dropped** |
| 4 | both-deleted with divergent content | ort auto-resolves to deletion, no unmerged entry | Correct result, but **zero trace**: merge reports "Conflict count: 0", status never shows the applied deletion or any auto-applied rename |

Single crash cause for 1a/1b/3: `restore_ours(path)` assumes every conflicted
path has content at `HEAD:path`. True for `UU`/`AA`/`UD`, false for
`DU`/`UA`/`DD`/file-location conflicts.

## Phase 1 — Survive the crash (make `restore_ours` total)

Fix in `src/git_ops/mod.rs` (`restore_ours`) + call site
`src/commands/cmd_merge_workflow.rs:219`.

1. If `path_exists_in_ref("HEAD", path)` → current restore behavior.
2. Else (ours has no such path — deletion/rename-away/file-location): record
   "ours = absent" by running `git rm --cached -- <path>` and deleting the
   worktree copy if present. This resolves the index entry for the partial
   merge commit without losing information (the path is still in the conflict
   list handed to the planner and to the user).

Semantics note: for file-location conflicts this drops the *suggested* copy
from integration pending slicing — acceptable, because Phase 2 makes the slice
carry the content, so nothing is lost end-to-end.

**Tests to flip:** `rename_rename_conflict_aborts_workflow_currently`,
`deleted_by_target_conflict_aborts_workflow_currently`,
`directory_rename_location_conflict_aborts_workflow_currently` → extend to
assert slices are created.

## Phase 2 — Rename-aware slicing

Goal: correlated paths become ONE slice group with rename semantics.

1. New `git_ops::conflict_entries()`: parse `git ls-files -u` (stage + path)
   and `git status --porcelain=v2 -z` (`u` records, which carry all three
   stages and rename origin). Return per-path `ConflictInfo { kind, stages,
   renames_from: Option<(String, String)> }`.
2. Grouping in `cmd_merge_workflow` before the TUI:
   - Merge `DD`-old-path + `UA`/`AU` new paths sharing the same stage-1 blob /
     rename origin into a single logical conflict with `member_paths`.
   - TUI conflict selector shows the group as one entry ("rename a.txt →
     renamed_main.txt / renamed_feature.txt"); default (quiet) slicing keeps
     groups together in one slice.
3. Slice materialization for rename groups (`planner.rs`):
   - `git mv` semantics: remove old path, add theirs-side path with source
     content, so the slice commit *is* the rename decision when merged.
   - File-location conflicts: slice carries the added file at BOTH old path
     (as added by target) and theirs-suggested path, letting resolution pick.
4. `--unassigned separate` must not split a rename group across slices;
   groups are the unit of assignment.

## Phase 3 — Distinguish "absent" from "empty" in resolve

In `src/commands/cmd_resolve.rs` (temp file preparation) +
`git_ops::write_blob_to_path`:

1. New variant `write_blob_or_signal(reference, path, dest) -> SideState {
   Present, Absent }`; only write the temp file when `Present`.
2. When a side is `Absent`, pass `/dev/null` (unix) or a zero-length sentinel
   named `<file>.DELETED` (windows) — git's own mergetool uses `/dev/null`
   for deleted files; mirror that convention so configured tools behave as
   they do under `git mergetool`.
3. Print per-file status: `Resolving 'f.txt' (LOCAL deleted, REMOTE modified)`.

**Test to flip:** `deleted_and_empty_paths_yield_identical_blob_content` →
assert `Absent` is reported distinctly.

## Phase 4 — Surface silent decisions (both-deleted, auto-rename)

1. In `cmd_merge_workflow`, after the partial merge, compute
   `git diff --name-status -M <target> <integration>` restricted to
   `D`/`R` records caused by the source side (compare against target) →
   "auto-applied" list: deletions taken from source, rename/location decisions.
2. Print it in the run summary (`Auto-applied deletions: N`, paths) and in
   `cmd_status` per-integration detail; embed the list (paths only) as a
   trailer in the partial-merge commit message for auditability and for
   `HERE` takeover.
3. No behavior change — visibility only.

**Test to flip:** `both_deleted_file_silently_vanishes_and_is_unreported` →
assert `gone.txt` IS reported by status and in the run summary.

## Sequencing / risk

- Phase 1 is a small, self-contained bugfix (worth shipping alone).
- Phase 2 is the largest change (models, TUI, planner); keep `--porcelain=v1`
  fallback parsing for older git (file-location conflicts and `-z` u-records
  require git ≥ 2.7; dir rename detection requires ort, git ≥ 2.34 — bump
  MSRV-adjacent git version check in `ensure_git_context` if needed).
- Phase 3/4 are independent, low risk.
- All four phases must keep existing suites A–E green; kokomeco consolidation
  already handles deletions correctly (`read-tree --reset`), no changes there.

## Implementation notes (deviations from plan)

- Phase 3: uses a zero-length `*.DELETED` sentinel on ALL platforms instead of
  `/dev/null`, because the non-`$MERGED` tool path re-reads `$BASE` as the tool
  output channel; `/dev/null` would break that. Resolve now prints per-side
  state (`LOCAL: deleted`, `REMOTE: present`, ...) so the deletion signal is
  visible even if the tool ignores file names.
- Phase 2 (TUI follow-up, DONE): the conflict selector now renders each group
  as ONE row (`tui::conflict_rows`): label `[rename/rename] old ⇄ ours ⇄ theirs`
  with the topology tag leading so it survives pane truncation; Space/`u`
  assign or unassign the whole group; F3 stacks the 3-way diffs of every
  member path (or runs the external difftool per member).
  `planner::expand_slices_to_groups` remains as defense-in-depth for
  `--select-paths` input.
- Group-aware resolve UX (DONE): `mergetopus resolve` now parses the
  slice-merge `CONFLICT (...)` lines and settles whole groups in ONE
  decision instead of a tool run per unrelated path:
  - interactive: a per-group menu (`tui::pick_option`) offering take-theirs /
    keep-ours / keep-both (topology-specific labels naming each path), plus
    "Run merge tool per file (manual)" as escape hatch;
  - non-interactive: `--on-group {theirs,ours,both,delete,tool}`; `--quiet`
    defaults to `theirs` (apply the slice's resolved decision), `tool` forces
    the legacy per-file loop;
  - plain content conflicts are never group-decided — always merge tool;
  - `planner::decide_group_action` validates the mode against the group's
    stage-derived roles (`GroupRoles`), `apply_group_action` stages the
    outcome (`write_oid_to_staged_path` / `resolve_path_as_deleted`).
- `git_ops::auto_applied_entries` diffs merge-base→source (not target→result)
  so both-deleted files are reported even though the result tree shows no
  delta against target.
