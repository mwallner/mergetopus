use clap::{Parser, Subcommand};
use crate::color::ColorMode;

const CLI_LONG_ABOUT: &str = "\
Mergetopus turns a regular git merge into an integration branch plus optional per-conflict slice branches.

Workflow:
  1) Create/reset an integration branch from your current HEAD
  2) Merge SOURCE into it with --no-commit
  3) Keep auto-merged files in integration
  4) Optionally group selected conflicted paths into one explicit slice branch via --select-paths

  If SOURCE is omitted, an interactive branch picker is shown (unless --quiet is set).
";

const CLI_AFTER_HELP: &str = "\
Examples:
  mergetopus origin/main
  mergetopus --source resolve
  mergetopus release/1.4 --select-paths 'src/a.rs,src/b.rs'
  mergetopus hotfix --yes
  mergetopus origin/main --quiet
  mergetopus resolve --commit _mmm/main/feature/slice1
  mergetopus status feature/refactor-auth
  mergetopus verify _mmm/main/feature/integration
  mergetopus HERE
";

const SOURCE_LONG_HELP: &str = "\
Source branch/ref/commit-ish to merge.

Accepted forms include local branches (feature/foo), remote-tracking refs (origin/main), tags, and commit SHAs.
If omitted, Mergetopus opens an interactive branch picker unless --quiet is set.";

const SOURCE_OPT_LONG_HELP: &str = "\
Optional named source branch/ref/commit-ish to merge.

Use this to disambiguate branch names that collide with subcommands (for example: resolve, here, license).
Example: --source resolve";

const SELECT_PATHS_LONG_HELP: &str = "\
Comma-separated list of conflicted file paths to include in a single explicit slice group.

Any conflicted file not listed here is handled as a default one-file slice branch.
Example: --select-paths 'src/lib.rs,src/main.rs,README.md'";

const QUIET_LONG_HELP: &str = "\
Run in non-interactive mode suitable for CI/CD.

Behavior changes:
- SOURCE must be provided explicitly (no source picker)
- Kokomeco prompts are skipped unless --yes is provided
- Conflict grouping comes only from --select-paths (no interactive conflict selector)";

const YES_LONG_HELP: &str = "\
Assume 'yes' for non-destructive confirmation prompts.

Currently used when an existing integration branch already has all slices merged and Mergetopus asks whether to create a kokomeco merge-commit branch.";

#[derive(Parser, Debug)]
#[command(name = "mergetopus")]
#[command(about = "Split complex merges into integration + selectable conflict slice branches")]
#[command(long_about = CLI_LONG_ABOUT)]
#[command(after_help = CLI_AFTER_HELP)]
pub struct Args {
    #[command(subcommand)]
    pub command: Option<Commands>,

    #[arg(
        value_name = "SOURCE",
        help = "Source branch/ref to merge into the integration branch",
        long_help = SOURCE_LONG_HELP
    )]
    pub source: Option<String>,

    #[arg(
        long = "source",
        value_name = "SOURCE",
        conflicts_with = "source",
        help = "Optional named source branch/ref to merge (disambiguates command-like names)",
        long_help = SOURCE_OPT_LONG_HELP
    )]
    pub source_opt: Option<String>,

    #[arg(
        long,
        value_name = "CSV_PATHS",
        help = "Comma-separated conflicted file paths to group into one explicit slice",
        long_help = SELECT_PATHS_LONG_HELP
    )]
    pub select_paths: Option<String>,

    #[arg(
        long,
        default_value_t = false,
        help = "Run non-interactively and never open TUI screens",
        long_help = QUIET_LONG_HELP
    )]
    pub quiet: bool,

    #[arg(
        long,
        default_value_t = false,
        help = "Auto-confirm prompts when safe to proceed",
        long_help = YES_LONG_HELP
    )]
    pub yes: bool,

    #[arg(
        long,
        value_name = "WHEN",
        default_value = "auto",
        help = "When to use colors in output",
        long_help = "Enable colored output: 'auto' (default) auto-detects based on terminal, \
                     'always' forces colors, 'never' disables them"
    )]
    pub color: ColorMode,
}

impl Args {
    pub fn effective_source(&self) -> Option<&str> {
        self.source_opt.as_deref().or(self.source.as_deref())
    }
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Show bundled license data
    License {
        #[arg(long, default_value_t = false)]
        full: bool,
        #[arg(long, default_value_t = false)]
        json_output: bool,
    },
    /// Resolve a merge conflict on a slice branch using the configured merge tool.
    ///
    /// The command looks up the three versions of each conflicted file
    /// (LOCAL = ours/remembered-head, BASE = merge-base, REMOTE = theirs/source)
    /// and invokes the tool named by `git config merge.tool`, whose command
    /// template is taken from `git config mergetool.<tool>.cmd`.
    /// The variables LOCAL, BASE, REMOTE and MERGED are set as shell environment
    /// variables before the command is executed (same convention as git mergetool).
    ///
    /// By default, resolved changes are staged but not committed.
    /// Use --commit to create a commit after staging.
    Resolve {
        /// Slice branch to resolve.  When omitted an interactive TUI picker is shown.
        #[arg(value_name = "BRANCH")]
        branch: Option<String>,

        /// Commit staged resolution changes at the end.
        #[arg(long, default_value_t = false)]
        commit: bool,
    },

    /// Show integration branch and slice progress status.
    ///
    /// SOURCE may be either a merge source ref (e.g. feature/foo) or a full
    /// integration branch name (e.g. _mmm/main/feature_foo/integration).
    Status {
        #[arg(value_name = "SOURCE")]
        source: Option<String>,
    },

    /// Cleanup slice and integration branches once a kokomeco branch exists.
    ///
    /// Finds every integration branch (and its associated slice branches) for
    /// which a consolidated kokomeco branch already exists, lists them in an
    /// interactive confirmation TUI, and deletes them on confirmation.
    /// The kokomeco branch itself is retained.
    Cleanup,

    /// Verify integration completeness after kokomeco creation.
    ///
    /// Fails if commits exist on the integration branch with a committer
    /// timestamp newer than the kokomeco commit.
    ///
    /// SOURCE may be either a merge source ref (e.g. feature/foo) or a full
    /// integration branch name (e.g. _mmm/main/feature_foo/integration).
    Verify {
        #[arg(value_name = "SOURCE")]
        source: Option<String>,

        #[arg(
            long,
            default_value_t = false,
            conflicts_with = "source",
            help = "Validate all integration branches globally (requires no SOURCE)"
        )]
        global: bool,
    },

    /// Take over an already in-progress merge and create slices for remaining conflicts.
    ///
    /// Use this on the currently checked-out target branch while MERGE_HEAD is
    /// present. Mergetopus captures already-resolved paths, rebuilds a canonical
    /// integration branch, and opens conflict grouping for only unresolved paths.
    #[command(name = "HERE", visible_alias = "here")]
    Here,

    /// Push an initialized merge plan (integration + slices + kokomeco) to a remote.
    ///
    /// Verifies that source and target branches exist on the remote, then
    /// force-pushes all MMM-namespace branches with --force-with-lease.
    ///
    /// If the current branch is not an integration branch, a TUI picker is shown
    /// to select one (only local branches not yet pushed are listed).
    ///
    /// When multiple remotes are configured and REMOTE is not specified, a TUI
    /// picker is shown (--quiet requires an explicit REMOTE).
    Push {
        #[arg(
            value_name = "REMOTE",
            help = "Remote to push to (required in --quiet mode when multiple remotes exist)"
        )]
        remote: Option<String>,
    },
}
