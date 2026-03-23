# mergetopus

[![Build Pack (Linux)](https://img.shields.io/github/actions/workflow/status/mwallner/mergetopus/build-pack.yml?branch=main&job=pack-linux&label=Build%20Pack%20(Linux))](https://github.com/mwallner/mergetopus/actions/workflows/build-pack.yml)
[![Build Pack (Windows)](https://img.shields.io/github/actions/workflow/status/mwallner/mergetopus/build-pack.yml?branch=main&job=pack-windows&label=Build%20Pack%20(Windows))](https://github.com/mwallner/mergetopus/actions/workflows/build-pack.yml)

![mergetopus logo](assets/mergetopus-logo.svg)

`mergetopus` is a Rust TUI tool that helps split difficult Git merges into:

- one integration branch for trivial/non-conflicting merge results
- optional slice branches for selected conflicted files

It follows and extends the workflow in `ext/Invoke-TheMergetopus.ps1`.

## What It Does

1. Validates Git context:
	- inside a Git worktree
	- clean working tree
	- valid merge source
2. Creates integration branch: `<current>_mw_int_<safe-source>`
3. Attempts merge with `--no-commit`
4. Keeps auto-merged files in integration commit
5. Resets conflicted files to ours in integration commit
6. Uses interactive TUI conflict selection by default (including 3-way diff), or groups explicit conflicted paths from `--select-paths` into one explicit slice
7. Creates one branch per explicit slice group from remembered HEAD
8. Creates default one-file slice branches for every conflict not explicitly assigned by the user
9. Applies source-side version of each affected file and commits with provenance trailers

Once slice branches exist, run `mergetopus resolve` to drive conflict resolution on a slice branch using your configured merge tool (see [Resolving Conflicts](#resolving-conflicts)).

## Complex Merge Diagrams

1. Hard merge without slicing (everything blocked in one place):

```text
main:      A---B---C
                 \
feature:            D---E---F

attempt:
main + feature  -> conflicts in many files at once

result:
- one large conflict-resolution task
- hard to parallelize
```

2. mergetopus split strategy (parallel conflict handling):

```text
remembered HEAD: C

main:                 A---B---C
                            \
source feature:              D---E---F

integration branch:
main_mw_int_feature   C---M(partial: only non-conflicting files)
                         |
                         +-- conflicted files reset to ours in M

slice branches from C:
main_mw_int_feature_slice1   C---S1 (explicit group: fileA,fileB)
main_mw_int_feature_slice2   C---S2 (explicit group: fileC)
main_mw_int_feature_slice3   C---S3 (auto singleton for unassigned fileD)
main_mw_int_feature_slice4   C---S4 (auto singleton for unassigned fileE)
```

3. After resolution and optional consolidation:

```text
integration after merging slices:
main_mw_int_feature   C---M---(merge S1)---(merge S2)---(merge S3)---(merge S4)

optional non-destructive consolidation output:
main_mw_int_feature_consolidated
                      \---MC (single merge-commit snapshot branch)

notes:
- integration history stays intact
- consolidated branch is created for review/promotion
```

## Existing Topology Handling

If the integration branch already exists, `mergetopus`:

1. discovers related slice branches (`<current>_mw_int_<safe-source>_sliceN`)
2. checks whether each slice is merged into the integration branch
3. reports merged/pending status
4. when all slices are merged, offers optional consolidation

Consolidation is non-destructive: it creates a separate branch `<integration>_consolidated` with a single merge commit snapshot.

## Installation

```bash
choco install mergetopus.portable
```

## Platform Support

`mergetopus` works on **Windows**, **macOS**, and **Linux**.

On Windows, the merge tool is invoked via `cmd /c`. On Unix-like systems (macOS, Linux),
it uses `sh -c`. Both approaches set the same environment variables (`LOCAL`, `BASE`, `REMOTE`, `MERGED`) 
so your merge tool configuration is cross-platform compatible.

## Branch Naming Conventions

An understanding of branch naming helps prevent accidental misuse:

- **Integration branches** follow the pattern `<original-branch>_mw_int_<source-branch>`.
  These are temporary working branches that hold the merge result with auto-merged
  files staged and conflicted files reset to "ours".
  - Example: `main_mw_int_feature`, `develop_mw_int_release_v1`

- **Slice branches** follow the pattern `<integration-branch>_slice<N>` where `N` is a number (1, 2, 3, ...).
  These are temporary per-conflict branches for resolving individual conflict groups.
  - Example: `main_mw_int_feature_slice1`, `main_mw_int_feature_slice2`

- **Consolidated branches** follow the pattern `<integration-branch>_consolidated`.
  These are optional output branches created after all slices are merged, containing
  a single merge-commit snapshot.
  - Example: `main_mw_int_feature_consolidated`

### Branch Filtering

When selecting a source branch for `mergetopus`:
- **Slice branches** (`*_slice<N>`) are automatically filtered out from the branch picker.
  They should only be used with `mergetopus resolve`, never as a source for a new merge.
- Only non-slice branches are available for selection, reducing accidental misuse.

### Integration Branch Redirection

If you accidentally select an integration branch as the source:
- `mergetopus` automatically detects this and redirects to the correct operation.
- It extracts the original branch and source from the integration branch name.
- It checks out the original branch and performs the merge with the actual source.
- Example: if you select `main_mw_int_feature`, mergetopus will:
  1. Detect it's an integration branch
  2. Check out `main`
  3. Merge `feature` instead
  4. Create a fresh `main_mw_int_feature` integration branch

This prevents confusion when re-running mergetopus on an existing integration branch.

## Usage

Interactive source selection (branch picker overlay shown):

```bash
mergetopus
```

Provide source branch explicitly:

```bash
mergetopus feature/refactor-auth
```

Explicit conflict grouping by path list:

```bash
# Put explicit paths into one grouped slice; all remaining conflicts become one-file slices
mergetopus feature/refactor-auth --select-paths src/a.rs,src/b.rs
```

Interactive conflict grouping (with 3-way diffs via `F3`) when `--select-paths` is not provided:

```bash
mergetopus feature/refactor-auth
```

Auto-confirm consolidation prompt:

```bash
mergetopus feature/refactor-auth --yes
```

Non-interactive CI/CD mode (no TUI):

```bash
# SOURCE is required in quiet mode
mergetopus feature/refactor-auth --quiet

# Quiet mode + explicit conflict grouping
mergetopus feature/refactor-auth --quiet --select-paths src/a.rs,src/b.rs

# Quiet mode + auto-consolidate when eligible
mergetopus feature/refactor-auth --quiet --yes
```

## Resolving Conflicts

After mergetopus has created slice branches, use `resolve` to open each slice
branch in your configured merge tool and commit the resolution:

```bash
# Interactive slice branch picker (TUI)
mergetopus resolve

# Resolve a specific slice branch directly
mergetopus resolve main_mw_int_feature_slice1

# Non-interactive (--quiet requires an explicit branch)
mergetopus resolve --quiet main_mw_int_feature_slice1
```

### What resolve does

1. Checks out the slice branch.
2. Reads the slice commit trailers to discover `Source-Ref`, `Source-Commit`,
   and the affected file paths.
3. Writes three temporary files per conflicted path:
   - `LOCAL`  — the file at the remembered HEAD (ours, before the merge)
   - `BASE`   — the file at the common ancestor (merge-base)
   - `REMOTE` — the file at the source commit (theirs)
4. Executes the configured merge tool with `LOCAL`, `BASE`, `REMOTE`, and `MERGED`
   set as environment variables (same convention as `git mergetool`). The command
   is executed via the appropriate shell:
   - **Windows**: `cmd /c <cmd>`
   - **Unix-like systems** (macOS, Linux): `sh -c <cmd>`
   
   `MERGED` points to the working-tree file, so the tool writes the resolution
   directly into the repository.
5. Stages the resolved file(s) and creates a new commit on the slice branch.

### Configuring the merge tool

`mergetopus resolve` reads the merge tool from Git config.  Set it once in
your global or repository config:

```bash
# Choose a tool name
git config merge.tool vimdiff

# Provide the shell command template.
# $LOCAL, $BASE, $REMOTE, $MERGED are expanded at runtime.
git config mergetool.vimdiff.cmd 'vimdiff "$LOCAL" "$BASE" "$REMOTE" -c "wincmd J" "$MERGED"'
```

Some common examples:

| Tool | Example `mergetool.<name>.cmd` |
|------|-------------------------------|
| vimdiff | `vimdiff "$LOCAL" "$BASE" "$REMOTE" -c "wincmd J" "$MERGED"` |
| nvim | `nvim -d "$LOCAL" "$REMOTE" "$MERGED"` |
| code (VS Code) | `code --wait --merge "$LOCAL" "$REMOTE" "$BASE" "$MERGED"` |
| meld | `meld "$LOCAL" "$BASE" "$REMOTE" --output "$MERGED"` |
| kdiff3 | `kdiff3 "$BASE" "$LOCAL" "$REMOTE" -o "$MERGED"` |

Any tool that reads `$LOCAL`, `$BASE`, `$REMOTE` and writes its result to
`$MERGED` will work.

## TUI Keybindings

When `--quiet` is not set, TUI is used for source branch picking (if `SOURCE`
is omitted), conflict selection (if `--select-paths` is omitted), and slice
branch selection for `resolve` (if `BRANCH` is omitted).

Conflict selector:

- `Arrow Up/Down`: move cursor
- `Tab`: switch pane
- `n`: create new explicit slice
- `Space`: assign/move highlighted conflict into currently selected slice
- `u`: unassign highlighted conflict (it will become default one-file slice)
- `d`: delete selected explicit slice (its files become unassigned)
- `F3`: open 3-way diff overlay
- `Enter`: apply selection
- `Esc`: close overlay or cancel selector
- `q`: cancel selector

Source branch picker / Slice branch picker:

- Type text to filter branch list
- `Arrow Up/Down`: move
- `Enter`: select
- `Esc` or `q`: cancel

3-way diff overlay:

- `Up/Down`: scroll
- `PageUp/PageDown`: fast scroll
- `Home/End`: jump to top/bottom
- `Esc`: close overlay

## Commit Metadata

Slice commits include:

- `Source-Ref`
- `Source-Commit`
- `Source-Path`
- `Source-Path-Commit`
- `Co-authored-by` (when source-side author info is available)

## Authorship Clarification

A squash or consolidation commit does not preserve per-commit author lineage by itself.
`mergetopus` preserves attribution context through provenance/co-author trailers and keeps original integration/slice history intact by writing consolidated output to a separate branch.

## Safety Notes

- No destructive reset/rewrite is performed automatically.
- Consolidation requires explicit confirmation (unless `--yes` is used).
- `--quiet` disables TUI interactions; provide `SOURCE` explicitly for CI/CD runs.
- Integration branch is not rewritten by default.

## License

MIT. See `LICENSE`.
