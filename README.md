# mergetopus

[![Build Linux](https://img.shields.io/github/actions/workflow/status/mwallner/mergetopus/build-linux.yml?branch=main&label=Build%20Linux)](https://github.com/mwallner/mergetopus/actions/workflows/build-linux.yml)
[![Build MacOS](https://img.shields.io/github/actions/workflow/status/mwallner/mergetopus/build-macos.yml?branch=main&label=Build%20macOS)](https://github.com/mwallner/mergetopus/actions/workflows/build-macos.yml)
[![Build Windows](https://img.shields.io/github/actions/workflow/status/mwallner/mergetopus/build-windows.yml?branch=main&label=Build%20Windows)](https://github.com/mwallner/mergetopus/actions/workflows/build-windows.yml)

![mergetopus logo](assets/mergetopus-logo.svg)

Ever started a merge only to find yourself with 100 conflicts that would require specific domain knowledge (from coworkers)? `mergetopus` splits one large merge into team-sized tasks so multiple people can resolve them in parallel. Written in Rust, works on Windows/macOS/Linux, has a TUI for interactive use and a `--quiet` mode for CI/CD.

## The problem

A long-running branch eventually has to be merged back. You run `git merge`. Git stops with hundreds of conflicts. Now the whole merge is blocked on one person, resolving everything by hand, in one working tree, one file at a time:

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

Nobody else can help, and the merge blocks the release until it's done.

## The idea

The idea is simple: most files don't actually conflict. So split the merge accordingly:

- **Integration branch**: takes everything Git could merge automatically.
- **Slices**: small, independent branches for groups of conflicting files. Different people can resolve these in parallel, with whatever merge tool they prefer.
- **Kokomeco branch**: a consolidated snapshot once all slices are done, with proper ancestry so `git blame` still works correctly.

```text
integration branch:
_mmm/main/feature/integration   C---M (only the auto-merged files)

slice branches (one per conflict group):
_mmm/main/feature/slice1   files A and B   -> developer 1
_mmm/main/feature/slice2   file C          -> developer 2
_mmm/main/feature/slice3   file D          -> developer 3

after resolution, slices are merged back into integration:
_mmm/main/feature/integration   C---M---S1---S2---S3
```

`mergetopus` sets this structure up, tracks it, and removes the temporary branches when you're done. The actual resolving is still you and your merge tool, one slice at a time.

## Quick tour

```bash
# intentionally init the mergetopus workflow from your target branch:
mergetopus feature/very-large-change

# already started a merge and hit conflicts? hand over the rest:
mergetopus HERE

# push so the team can collaborate:
mergetopus push origin

# each dev resolves a slice:
mergetopus resolve _mmm/main/feature_very-large-change_c6e0/slice1

# check who's done what:
mergetopus status

# clean up when everything's merged:
mergetopus cleanup
```

What each of these commands does in detail:

## Installing

Grab a binary for your platform from the [GitHub Releases](https://github.com/mwallner/mergetopus/releases) page and put it on your `PATH`.

On Windows, with [Chocolatey](https://chocolatey.org/):

```bash
choco install mergetopus.portable
```

Anywhere, with Cargo:

```bash
cargo install --git https://github.com/mwallner/mergetopus.git
```

You also need Git. A couple of advanced behaviors want newer versions:

- Git >= 2.42 for the criss-cross handling in [tricky histories](#tricky-histories)
- Git >= 2.25 for the bulk fast paths in [large repositories](#very-large-repositories)

Older Git still works; the fallbacks are noted where relevant.

## Starting a merge

From the branch you want to merge *into* (say `main`), with a clean working tree:

```bash
mergetopus feature/very-large-change
```

What happens, in order:

1. Checks that you're in a Git repo with nothing uncommitted.
2. Creates the integration branch: `_mmm/main/feature_very-large-change_c6e0/integration` (names get sanitized, see [Branch names](#branch-names)).
3. Runs `git merge --no-commit` and commits everything that merged cleanly onto the integration branch. Conflicted files are reset to *your* side for now; they'll be dealt with in slices.
4. Lets you group the conflicts into slices: interactively via TUI, or non-interactively with `--select-paths`. Conflicts you don't group get one slice each by default. Want them all in one shared slice instead? Use `--unassigned single`; the TUI asks.
5. Creates one slice branch per group, starting from the merge base, carrying the source-side version of each file, with a commit message noting where it came from.

Then everyone picks a slice and resolves it (see [Resolving conflicts](#resolving-conflicts)).

## Working as a team

The commands are meant to be used together as a repeatable process:

1. **Plan and split**: one person runs `mergetopus <source>`.
2. **Share the plan**: `mergetopus push` pushes the integration branch, all slice branches, and the kokomeco branch if it exists. Everything goes out with `--force-with-lease`, and the command checks that source and target are on the remote first. One remote configured? Auto-selected. Multiple? TUI picker.

   ```bash
   mergetopus push            # single remote: auto-selected
   mergetopus push origin     # explicit
   ```

3. **Resolve in parallel**: each developer runs `mergetopus resolve <slice>`. Add `--commit` to commit automatically once everything is staged.
4. **Track progress**: anyone can ask what's done and what's next:

   ```bash
   mergetopus status feature/very-large-change   # one plan in detail
   mergetopus status                             # all plans, local + remotes
   ```

   Status reports merged vs pending slices and suggests the next commands.
5. **Finish**: once all slices are back in integration, create the kokomeco snapshot, merge the result into your target branch using normal Git policy, then `mergetopus cleanup` removes the completed workflow's temporary branches. To abandon an incomplete workflow, use `mergetopus discard` instead.

## Resolving conflicts

`mergetopus resolve <slice>` merges that slice into its integration branch and walks you through the conflicts:

1. Derives the integration branch from the slice name.
2. Checks out the integration branch, runs `git merge --no-commit <slice>`.
3. For each conflicted file, pulls three versions out of Git's history:
   - `LOCAL`: the integration branch's version
   - `BASE`: the common ancestor's version
   - `REMOTE`: the slice branch's version
4. Writes them to temp files and launches your merge tool with `LOCAL`, `BASE`, `REMOTE` and `MERGED` environment variables. That's the exact convention `git mergetool` uses, so your existing tool config just works. `MERGED` points at the real file in the working tree; whatever your tool writes *is* the resolution.
5. Stages each resolved file. With `--commit`, creates the merge commit. Before committing it checks for leftover conflict markers, and it honors `mergetool.<tool>.trustExitCode` if you set one.

For rename/delete and modify/delete groups, `--on-group` can choose the action without asking: `theirs`, `ours`, `both`, `delete`, or `tool`.

```bash
mergetopus resolve --on-group theirs _mmm/main/feature/slice1
```

Configure the tool like you would for git:

```bash
git config merge.tool vimdiff
git config mergetool.vimdiff.cmd 'vimdiff "$LOCAL" "$BASE" "$REMOTE" -c "wincmd J" "$MERGED"'
```

Some ready-made ones, for any tool that reads the three inputs and writes `$MERGED`:

| Tool           | Example `mergetool.<name>.cmd`                               |
| -------------- | ------------------------------------------------------------ |
| vimdiff        | `vimdiff "$LOCAL" "$BASE" "$REMOTE" -c "wincmd J" "$MERGED"` |
| nvim           | `nvim -d "$LOCAL" "$REMOTE" "$MERGED"`                       |
| code (VS Code) | `code --wait --merge "$LOCAL" "$REMOTE" "$BASE" "$MERGED"`   |
| meld           | `meld "$LOCAL" "$BASE" "$REMOTE" --output "$MERGED"`         |
| kdiff3         | `kdiff3 "$BASE" "$LOCAL" "$REMOTE" -o "$MERGED"`             |

The `LOCAL`/`BASE`/`REMOTE`/`MERGED` convention is the same on all platforms. On Windows the tool is invoked directly (no `cmd /c`, which avoids quoting/whitespace problems), and mergetopus sets `core.longpaths=true` for the repository so deep path merges stay usable; on macOS and Linux commands run via `sh -c`.

While grouping conflicts in the selector, `F3` diffs the selected file: your configured `diff.tool` if you have one, otherwise a built-in 3-way view.

```bash
git config diff.tool vscode
git config difftool.vscode.cmd 'code --wait --diff "$LOCAL" "$REMOTE"'
```

## Branch names

Every branch mergetopus creates shares one pattern:

```text
_mmm/<target>/<source>/integration    the merge result so far
_mmm/<target>/<source>/slice<N>       one conflict group (N = 1, 2, 3, ...)
_mmm/<target>/<source>/kokomeco       final consolidated snapshot
```

Branch names contain characters Git reserves for ref separators, like the `/` in `feature/auth`. So each component gets *sanitized*: anything outside `[0-9A-Za-z._-]` becomes `_`. Sanitizing can make two different names collide, so a short hash of the original is appended whenever a character was replaced. `feature/very-large-change` becomes `feature_very-large-change_c6e0`. Names that are already safe (`feature`, `LTS_v42`) are used as they are.

Two guard rails:

- Slice branches are filtered out of the branch picker. They're inputs for `resolve`, never sources for a new merge.
- If you pick an integration branch as a source anyway, mergetopus recognizes the running merge and asks: show its status, or start a fresh merge of the real source into the real target.

## Finishing: kokomeco and cleanup

Once all slices are merged back, you can *consolidate*: create a `kokomeco` branch holding a single merge commit that represents the finished merge.

The name stands for **K**Orrekt **KO**nsoliderter **ME**rge **CO**mmit: the correctly consolidated merge commit.

Why bother? The obvious way to finish a messy merge is to squash everything into one commit. That works, but it destroys authorship: hundreds of lines Wendy and Gideon wrote now look like whoever ran the squash wrote them. `git blame` points at the integrator and the information is gone.

Kokomeco avoids this by *not* rewriting anything. It's a separate branch whose single commit is an ordinary merge commit with two parents:

- parent 1: the target branch's head from before the merge started
- parent 2: the source branch's tip

...and whose tree is copied from the finished integration branch. Because the parentage is honest, `git blame` can still follow unchanged lines through parent 2 back to the original authors:

```text
git blame _mmm/LTS_v42/LTS_v17/kokomeco -- config.toml
  max_connections = 200   -> Wendy Corduroy
  timeout = 60            -> Gideon Gleeful
```

Nothing is destructive: integration history stays intact, and consolidation asks first. Under `--quiet` it is skipped and the run tells you so; pass `--yes` to create it unattended. Create the snapshot explicitly whenever you want it:

```bash
mergetopus consolidate feature/refactor-auth
```

`mergetopus cleanup` removes integration and slice branches after a kokomeco branch exists. It keeps the kokomeco branch, lists what it will delete, and waits for confirmation. To abandon a workflow before consolidation, use `mergetopus discard`; it removes the associated integration and slice branches instead. Both accept `--close-prs`, and neither runs under `--quiet`.

Before you promote the kokomeco branch, `mergetopus verify` gives you one safeguard: it fails if the integration branch carries commits with a committer timestamp newer than the kokomeco commit, which catches work added after consolidation. It is a timestamp comparison, not a content diff.

```bash
mergetopus verify feature/refactor-auth     # one plan
mergetopus verify --global                  # every plan with a kokomeco
```

## Worked example: an LTS cascade

A realistic setting. Five developers, several long-term stable branches, and Stan, who merges old LTS versions into newer ones and eventually upstream to `main`.

| Developer      | Role                 | Active branches          |
| -------------- | -------------------- | ------------------------ |
| Wendy Corduroy | Developer            | LTS_v17, LTS_v32         |
| Gideon Gleeful | Developer            | LTS_v17, LTS_v32         |
| Dipper Pines   | Developer            | LTS_v32, main            |
| Mabel Pines    | Team lead (Gideon's) | LTS_v32, main            |
| Stan Pines     | Senior integrator    | LTS_v42 (merges forward) |

Wendy and Gideon made *overlapping* changes to `config.toml` and `engine.rs` in LTS_v17. Stan faces LTS_v17 into LTS_v42, then LTS_v32 into LTS_v42, then LTS_v42 into `main`.

Step 1, from `LTS_v42`:

```bash
mergetopus LTS_v17 --select-paths config.toml,engine.rs
```

The explicitly grouped files become slice1 - that's where Wendy's and Gideon's changes collide. Mabel resolves it, since she knows that code. The remaining conflicts each get their own slice, resolved by Dipper. Once all slices are back:

```text
_mmm/LTS_v42/LTS_v17/kokomeco
    parent 1: original LTS_v42 HEAD
    parent 2: LTS_v17 tip
    tree:     final resolved state
```

Step 2 promotes that snapshot into `LTS_v42`, then repeats for LTS_v32:

```bash
git checkout LTS_v42 && git merge _mmm/LTS_v42/LTS_v17/kokomeco
mergetopus LTS_v32 --select-paths engine.rs,utils.rs
```

Step 3 merges LTS_v42 into `main`. By now most of it merges cleanly; only two files still conflict.

After the full cascade, `git blame` on each kokomeco still traces every line to the developer who wrote it, because every step kept real merge ancestry. No squashes, no rewrites. The integration test `lts_cascade_merge_preserves_authorship_in_kokomeco` in `tests/test_suite_d.rs` exercises exactly this scenario.

## HERE: take over a manual merge

You already ran `git merge`, Git stopped with conflicts, and you resolved a few files by hand before realizing this is too big for one person. `mergetopus HERE` hands the remainder to the workflow. Your already-resolved files are kept, the manual merge is aborted, and the standard integration/slice structure is built. Only the *unresolved* conflicts become slices.

```bash
mergetopus HERE
mergetopus --quiet --select-paths src/big/file1.cs,src/big/file2.cs HERE
```

It needs an active merge (`MERGE_HEAD`), and then it's business as usual.

## Tricky histories

**Multiple merge bases (criss-cross).** Occasionally something subtle happens in Git history. Both sides contain the other's history through *different* merge commits, so the branches share several equally-good common ancestors. A plain `git merge` doesn't consult any of them directly. It builds a *virtual* merge base from all of them, and that can report conflicts no single real base would produce.

Mergetopus anchors everything (slice branches, the `BASE` input to your merge tool, the diff view) to *one concrete* base. So when several exist, it asks you to choose one. The default is what `git merge-base A B` reports - the same single-base view GitHub/GitLab PRs show.

The conflict list is then re-evaluated against your chosen base with `git merge-tree --merge-base=...`. That uses git's own ort semantics, rename detection included, so files a single-base merge would auto-resolve are settled immediately and never become slices. Non-interactive mode (`--quiet`, plus `--yes` where the TUI is suppressed) warns and takes the default. Requires Git >= 2.42; older versions fall back to the raw conflict list.

## Very large repositories

A merge with thousands of conflicted paths is slow for a tool that calls Git once per file, because every subprocess has a cost. Mergetopus therefore doesn't call Git per file:

- Path lists stream over stdin (`--pathspec-from-file=-`, `update-index --index-info`, `checkout-index --stdin`) instead of hitting command-line length limits (bounded argv chunks on Git < 2.25).
- Files are classified and fetched in single batched walks (`ls-tree`, `cat-file --batch`); merge-tool inputs come from one `cat-file --batch` process.
- "Is this slice merged yet?" is answered with one `git branch --merged` instead of an ancestry test per branch.

The remaining cost is proportional to the number of *slices* (each needs a branch and a commit), not the number of files. With a four-digit conflict count, group them into fewer slices with `--select-paths`. Or let the leftovers share one slice with `--unassigned single`.

**Worktrees.** If your repository already uses worktrees, mergetopus prefers running branch operations in a worktree per branch. New worktrees are created next to your existing ones; the base directory is inferred from current worktree paths, falling back to the repo root's parent. If you don't use worktrees, nothing changes for you.

## Command and flag reference

```bash
mergetopus <source>                # start (or continue) a merge workflow
mergetopus                         # same, with an interactive branch picker
mergetopus resolve                 # resolve a slice, interactive picker
mergetopus resolve <slice>         # resolve a specific slice
mergetopus resolve --commit <slice>  # ... and commit when done
mergetopus status                  # overview of all merge plans (local + remotes)
mergetopus status <source>         # detailed status of one plan
mergetopus status <source> --pr    # ... plus pull/merge request URLs
mergetopus verify <source>         # integration still matches its kokomeco?
mergetopus verify --global         # check every plan that has a kokomeco
mergetopus push [remote]           # share the plan on a remote
mergetopus HERE                    # take over an in-progress manual merge
mergetopus consolidate [source]    # create the kokomeco branch
mergetopus cleanup [--close-prs]   # remove temporary branches after kokomeco
mergetopus discard [--close-prs] <workflow>  # abandon a workflow and remove its branches
mergetopus license                 # bundled third-party license text
```

`resolve --on-group <mode>` supports `theirs`, `ours`, `both`, `delete`, and `tool` for rename/delete and modify/delete conflict groups.

Global flags:

| Flag | Meaning |
| ---- | ------- |
| `--quiet`  | No TUI, no prompts. For CI/CD. `SOURCE` must be given explicitly; ambiguous choices fall back to documented defaults (with a warning for the merge-base choice). Branch deletion and consolidation prompts are skipped, not auto-approved. |
| `--yes`    | Auto-confirm prompts (consolidation, `discard`). Not a `cleanup` flag. |
| `--select-paths <paths>` | Comma-separated paths that form one explicit slice group. |
| `--unassigned <separate\|single>` | What to do with conflicts you didn't group: `separate` (default) gives each its own slice, `single` puts them all in one shared slice. |
| `--color <auto\|always\|never>` | Color output control. |

## Pull request integration

Mergetopus can create and manage pull/merge requests for the integration and slice branches, so each slice can be reviewed as its own PR. The forge (GitHub, GitLab, Bitbucket Data Center, Forgejo) is detected from the remote URL:

| Remote URL | Detected forge |
| ---------- | -------------- |
| `https://github.com/owner/repo.git`, `git@github.com:...` | GitHub |
| `https://gitlab.com/owner/repo.git` | GitLab |
| `https://bitbucket.company.com/scm/PROJ/repo.git`, `ssh://git@bitbucket...` | Bitbucket Data Center |
| `https://codeberg.org/owner/repo.git` | Forgejo |

> Bitbucket **Cloud** (bitbucket.org) is not supported; only the Data Center REST API (`/rest/api/latest`) is. Cloud remotes get an explicit error. For self-hosted instances (e.g. `gitlab.internal.example.com`), declare the forge: `git config mergetopus.forge-type gitlab`.

Authentication is a token, read from git config with an environment variable fallback:

| Forge     | Git config key               | Environment variable |
| --------- | ---------------------------- | -------------------- |
| GitHub    | `mergetopus.github-token`    | `GITHUB_TOKEN`       |
| GitLab    | `mergetopus.gitlab-token`    | `GITLAB_TOKEN`       |
| Bitbucket | `mergetopus.bitbucket-token` | `BITBUCKET_TOKEN` (Data Center PAT, sent as `Bearer`) |
| Forgejo   | `mergetopus.forgejo-token`   | `FORGEJO_TOKEN` or `CODEBERG_TOKEN` |

```bash
mergetopus pr create            # draft PRs for integration + slice branches
mergetopus pr create <source>   # ... for one specific plan
mergetopus pr sync              # refresh PR titles/descriptions
mergetopus pr list              # show PR URLs and states
mergetopus push --pr            # push + create PRs in one step
mergetopus status --pr          # PR links inside status output
mergetopus cleanup --close-prs  # close PRs when removing branches
```

## The interactive selector (TUI)

Without `--quiet`, mergetopus shows terminal dialogs: to pick a source branch, to group conflicts into slices, and to pick a slice for `resolve`.

Conflict selector keys:

| Key | Action |
| --- | ------ |
| Up/Down | move the cursor |
| Tab | switch between the conflict list and the slice list |
| `n` | create a new slice group |
| Space | assign (or move) the highlighted conflict into the selected slice |
| `u` | unassign the highlighted conflict |
| `d` | delete the selected slice (its files become unassigned) |
| `F3` | diff the selected file (external `diff.tool`, or the built-in 3-way view) |
| Enter | apply; if files are still unassigned, you're asked whether they get one slice each or one shared slice |
| Esc / `q` | close overlay / cancel |

Branch pickers: type to filter, Up/Down to move, Enter to select, Esc or `q` to cancel. The built-in diff view scrolls with Up/Down, PageUp/PageDown, Home/End, and closes with Esc.

## Commit metadata

Slice commits carry informational trailers recording where each file came from: `Source-Ref`, `Source-Commit`, `Source-Path`, `Source-Path-Commit`, plus a `Co-authored-by` line when author info is available. These are for humans and for `mergetopus status`; nothing structural depends on them. Authorship in the final history comes from kokomeco's real merge parents, not from trailers.

## Safety, in one page

- Nothing is rewritten or reset automatically. No history rewriting, ever.
- Deletions are confirmed interactively. `cleanup` and `discard` refuse to run under `--quiet`; `discard --yes` and `mergetopus --yes` are the unattended variants.
- Consolidation under `--quiet` is skipped rather than approved. Use `consolidate` or `--yes`.
- The integration branch is not force-changed by default.
- `mergetopus verify` lets you confirm the snapshot still matches before promoting it.

## Glossary

| Term | Meaning |
| ---- | ------- |
| Integration branch | Temporary branch holding the auto-merged part of the result. |
| Slice branch | Temporary branch holding one group of conflicted files, for one developer to resolve. |
| Kokomeco branch | The consolidated final merge commit, with honest merge parents. |
| Merge base | The common ancestor a merge compares both sides against. |
| Unassigned conflicts | Conflicts not grouped into an explicit slice; one slice each, or one shared slice with `--unassigned single`. |
| Sanitized name | A branch name with unsafe characters replaced by `_`, used inside `_mmm/...` names. |

## License

MIT. See `LICENSE.txt`.
