# mergetopus

[![Build Linux](https://img.shields.io/github/actions/workflow/status/mwallner/mergetopus/build-linux.yml?branch=main&label=Build%20Linux)](https://github.com/mwallner/mergetopus/actions/workflows/build-linux.yml)
[![Build Windows](https://img.shields.io/github/actions/workflow/status/mwallner/mergetopus/build-windows.yml?branch=main&label=Build%20Windows)](https://github.com/mwallner/mergetopus/actions/workflows/build-windows.yml)

![mergetopus logo](assets/mergetopus-logo.svg)

`mergetopus` is a tool that helps teams follow a structured workflow for very large merges by splitting one risky merge into parallelizable tasks:

- one integration branch for trivial/non-conflicting merge results
- optional slice branches for selected conflicted files

It follows and extends the workflow in `ext/Invoke-TheMergetopus.ps1`.

The core idea is collaborative merge execution:

- one person initializes the merge plan (`mergetopus`)
- multiple developers resolve different slice branches in parallel (`mergetopus resolve`)
- a coordinator monitors progress and next actions (`mergetopus status`)
- once promoted, temporary branches are cleaned up (`mergetopus cleanup`)

## What It Does

1. Validates Git context:
	- inside a Git worktree
	- clean working tree
	- valid merge source
2. Creates integration branch: `_mmm/<safe-current>/<safe-source>/integration`
3. Attempts merge with `--no-commit`
4. Keeps auto-merged files in integration commit
5. Resets conflicted files to ours in integration commit
6. Uses interactive TUI conflict selection by default (including 3-way diff), or groups explicit conflicted paths from `--select-paths` into one explicit slice
7. Creates one branch per explicit slice group from merge base (current HEAD vs source)
8. Creates default one-file slice branches for every conflict not explicitly assigned by the user
9. Applies source-side version of each affected file and commits with provenance trailers

Once slice branches exist, run `mergetopus resolve` to drive conflict resolution on a slice branch using your configured merge tool (see [Resolving Conflicts](#resolving-conflicts)).

## Collaborative Workflow (Large Merges)

Use the commands together as a repeatable team process.

1. Plan and split the merge (`mergetopus`).

```bash
# from your target branch (for example: main)
mergetopus feature/very-large-change
```

What this does:

- creates `_mmm/<target>/<source>/integration`
- records non-conflicting merge results
- creates per-conflict slice branches (`_mmm/<target>/<source>/sliceN`)

2. Resolve slices in parallel (`mergetopus resolve`).

```bash
# each developer picks a slice and resolves it
mergetopus resolve _mmm/main/feature_very-large-change/slice1
mergetopus resolve --commit _mmm/main/feature_very-large-change/slice1
```

Each slice is merged into the integration branch with `--no-commit`, resolved with your merge tool, and optionally committed.

3. Track progress and next actions (`mergetopus status`).

```bash
mergetopus status feature/very-large-change
```

Status reports merged vs pending slices and suggests what to do next.

4. Promote and clean up (`mergetopus cleanup`).

- after all slices are merged, optionally create `kokomeco` snapshot branch
- merge the chosen final branch into your target branch using normal Git policy
- run cleanup to remove obsolete integration/slice branches

```bash
mergetopus cleanup
```

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
_mmm/main/feature/integration   C---M(partial: only non-conflicting files)
                         |
                         +-- conflicted files reset to ours in M

slice branches from merge-base(C,F):
_mmm/main/feature/slice1   B---S1 (explicit group: fileA,fileB)
_mmm/main/feature/slice2   B---S2 (explicit group: fileC)
_mmm/main/feature/slice3   B---S3 (auto singleton for unassigned fileD)
_mmm/main/feature/slice4   B---S4 (auto singleton for unassigned fileE)
```

3. After resolution and optional consolidation:

```text
integration after merging slices:
_mmm/main/feature/integration   C---M---(merge S1)---(merge S2)---(merge S3)---(merge S4)

optional non-destructive consolidation output:
_mmm/main/feature/kokomeco
                      \---MC (single merge-commit snapshot branch)

notes:
- integration history stays intact
- consolidated branch is created for review/promotion
```

## Existing Topology Handling

If the integration branch already exists, `mergetopus`:

1. discovers related slice branches (`_mmm/<safe-current>/<safe-source>/sliceN`)
2. checks whether each slice is merged into the integration branch
3. reports merged/pending status
4. when all slices are merged, offers optional consolidation

Consolidation is non-destructive: it creates a separate branch `_mmm/<safe-current>/<safe-source>/kokomeco` with a single merge commit snapshot.

### Why "kokomeco" Exists

`kokomeco` stands for **KOrrekt KOnsoliderter MErge COmmit** (German: "correctly consolidated merge commit").

The name is intentional: the consolidation step is not a squash and not a history rewrite of the integration branch. Instead, Mergetopus creates a separate merge-commit snapshot branch with merge parents derived from:

- the remembered target-branch head (before the merge workflow started)
- the original merge source commit

and with the final resolved tree copied from the integration branch.

Why this matters for `git blame`:

- A plain squash-style consolidation would collapse ownership and often attribute many lines to the integrator commit.
- Kokomeco keeps a proper merge ancestry edge to the original source side, so line-blame can continue to follow where unchanged lines actually came from.
- Temporary integration/slice execution history can be cleaned up later, while the promoted branch still retains useful provenance in the final merge topology.

## Installation

```bash
choco install mergetopus.portable
```

## Platform Support

`mergetopus` works on **Windows**, **macOS**, and **Linux**.

During `resolve`, Mergetopus sets the same environment variables (`LOCAL`, `BASE`, `REMOTE`, `MERGED`) across platforms so merge tool configuration stays portable.

On Windows, merge tools are invoked directly (without `cmd /c`) to avoid quoting/whitespace proxy issues.
On Unix-like systems (macOS, Linux), command execution uses `sh -c`.

When running inside a Git worktree on Windows, mergetopus ensures `core.longpaths=true`
for the repository so deep path merges remain usable.

## Worktree Behavior

Mergetopus supports Git worktrees in a conditional way:

- If your repository has no additional worktrees, Mergetopus keeps the existing default behavior (no automatic worktree creation).
- If your repository already uses worktrees, Mergetopus prefers running branch operations in branch-specific worktree directories.
- When Mergetopus needs to create a branch worktree and no suitable branch worktree exists yet, it infers a base directory from existing worktree paths.
- If a common base cannot be inferred, Mergetopus falls back to the parent of the repository root.

This keeps non-worktree workflows stable while improving branch checkout ergonomics in repositories that already use worktrees.

## Branch Naming Conventions

An understanding of branch naming helps prevent accidental misuse:

- **Integration branches** follow the pattern `_mmm/<safe-original-branch>/<safe-source-branch>/integration`.
  These are temporary working branches that hold the merge result with auto-merged
  files staged and conflicted files reset to "ours".
  - Example: `_mmm/main/feature/integration`, `_mmm/develop/release_v1/integration`

- **Slice branches** follow the pattern `_mmm/<safe-original-branch>/<safe-source-branch>/slice<N>` where `N` is a number (1, 2, 3, ...).
  These are temporary per-conflict branches for resolving individual conflict groups.
  - Example: `_mmm/main/feature/slice1`, `_mmm/main/feature/slice2`

- **Kokomeco branches** follow the pattern `_mmm/<safe-original-branch>/<safe-source-branch>/kokomeco`.
  These are optional output branches created after all slices are merged, containing
  a single merge-commit snapshot.
  - Example: `_mmm/main/feature/kokomeco`

The `safe-*` components use the same sanitization rules as before: characters outside `[0-9A-Za-z._-]` are replaced with `_`.

### Branch Filtering

When selecting a source branch for `mergetopus`:
- **Slice branches** (`_mmm/.../slice<N>`) are automatically filtered out from the branch picker.
  They should only be used with `mergetopus resolve`, never as a source for a new merge.
- Only non-slice branches are available for selection, reducing accidental misuse.

### Integration Branch Redirection

If you accidentally select an integration branch as the source:
- `mergetopus` automatically detects this and redirects to the correct operation.
- It extracts the original branch and source from the integration branch name.
- It checks out the original branch and performs the merge with the actual source.
- Example: if you select `_mmm/main/feature/integration`, mergetopus will:
  1. Detect it's an integration branch
  2. Check out `main`
  3. Merge `feature` instead
  4. Create a fresh `_mmm/main/feature/integration` branch

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

Interactive conflict grouping (with `F3` opening your configured `diff.tool`, or the inline 3-way view when no `diff.tool` is set) when `--select-paths` is not provided:

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

# Show slice/integration progress status
mergetopus status feature/refactor-auth

# Cleanup temporary integration/slice branches (interactive confirmation)
mergetopus cleanup

# Take over an already in-progress manual merge
mergetopus HERE
```

Takeover mode for an in-progress merge:

```bash
# Optional: non-interactive explicit grouping for remaining conflicts
mergetopus --quiet --select-paths src/big/file1.cs,src/big/file2.cs HERE
```

## Status Reporting

Use `mergetopus status` to inspect an integration branch and its slice progress.

```bash
# Status by source ref
mergetopus status feature/refactor-auth

# Status by integration branch name
mergetopus status _mmm/main/feature_refactor-auth/integration
```

The status output includes:

- integration branch
- source ref and source SHA (when derivable from integration history)
- merged/pending slice counts
- pending slice details with detected affected paths (when available)
- suggested next commands

## Resolving Conflicts

After mergetopus has created slice branches, use `resolve` to merge a selected
slice into its corresponding integration branch with `--no-commit`, then open
each conflicted file in your configured merge tool one-by-one:

```bash
# Interactive slice branch picker (TUI)
mergetopus resolve

# Resolve a specific slice branch directly
mergetopus resolve _mmm/main/feature/slice1

# Non-interactive (--quiet requires an explicit branch)
mergetopus resolve --quiet _mmm/main/feature/slice1

# Create a commit automatically after staging
mergetopus resolve --commit _mmm/main/feature/slice1
```

### What resolve does

1. Derives the corresponding integration branch from the slice branch name.
2. Checks out the integration branch and starts `git merge --no-commit <slice>`.
3. For each currently conflicted file, derives from the Git graph:
  - `LOCAL` as the integration branch `HEAD` before the merge
  - `REMOTE` as the slice branch tip
  - `BASE` as `merge-base(LOCAL, REMOTE)`
4. Writes three temporary files per conflicted path:
  - `LOCAL`  — the file at the integration branch side
  - `BASE`   — the file at the common ancestor (merge-base)
  - `REMOTE` — the file at the slice branch side
5. Executes the configured merge tool with `LOCAL`, `BASE`, `REMOTE`, and `MERGED`
   set as environment variables (same convention as `git mergetool`). The command
   is executed via the appropriate shell:
  - **Windows**: invoked directly without `cmd /c`
   - **Unix-like systems** (macOS, Linux): `sh -c <cmd>`
   
  `MERGED` points to the conflicted working-tree file on the integration branch,
  so the tool writes the resolution directly into the repository.
6. Stages each resolved file.
7. Optional: if `--commit` is passed, creates the merge commit on the integration branch.

## Take Over In-Progress Merge (`HERE`)

Use `mergetopus HERE` when you already started a regular `git merge` manually,
resolved some conflicts, and want Mergetopus to take over only the remaining
unresolved conflicts.

Typical scenario:

1. You run `git merge <source>` on your target branch.
2. Git stops with conflicts.
3. You manually resolve some files.
4. You run `mergetopus HERE` to continue using slice workflow for what remains.

Behavior of `HERE`:

- requires an active merge (`MERGE_HEAD` must exist)
- preserves already-resolved file content from your working tree/index
- aborts the manual merge and rebuilds canonical Mergetopus integration state
- creates slices only for still-unresolved conflict paths
- opens normal conflict grouping flow (or uses `--select-paths` in quiet mode)

Command examples:

```bash
# Interactive takeover
mergetopus HERE

# Quiet takeover with explicit grouping for remaining conflicts
mergetopus --quiet --select-paths src/module/a.rs,src/module/b.rs HERE
```

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

| Tool           | Example `mergetool.<name>.cmd`                               |
| -------------- | ------------------------------------------------------------ |
| vimdiff        | `vimdiff "$LOCAL" "$BASE" "$REMOTE" -c "wincmd J" "$MERGED"` |
| nvim           | `nvim -d "$LOCAL" "$REMOTE" "$MERGED"`                       |
| code (VS Code) | `code --wait --merge "$LOCAL" "$REMOTE" "$BASE" "$MERGED"`   |
| meld           | `meld "$LOCAL" "$BASE" "$REMOTE" --output "$MERGED"`         |
| kdiff3         | `kdiff3 "$BASE" "$LOCAL" "$REMOTE" -o "$MERGED"`             |

Any tool that reads `$LOCAL`, `$BASE`, `$REMOTE` and writes its result to
`$MERGED` will work.

### Configuring optional F3 diff tool

In the conflict selector, `F3` behaves as follows:

- if `git config diff.tool` is set, `F3` launches that difftool for the selected conflicted file
- if `diff.tool` is not set, `F3` opens the built-in inline 3-way diff overlay

Example:

```bash
git config diff.tool vscode
git config difftool.vscode.cmd 'code --wait --diff "$LOCAL" "$REMOTE"'
```

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
- `F3`: open configured difftool for selected file (or inline 3-way diff if `diff.tool` is not set)
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
