use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "mergetopus")]
#[command(about = "Split complex merges into integration + selectable conflict slice branches")]
#[command(
    long_about = "Mergetopus turns a regular git merge into an integration branch plus optional per-conflict slice branches.\n\nWorkflow:\n  1) Create/reset an integration branch from your current HEAD\n  2) Merge SOURCE into it with --no-commit\n  3) Keep auto-merged files in integration\n  4) Optionally group selected conflicted paths into one explicit slice branch via --select-paths\n\nIf SOURCE is omitted, an interactive branch picker is shown (unless --quiet is set)."
)]
#[command(
    after_help = "Examples:\n  mergetopus origin/main\n  mergetopus release/1.4 --select-paths 'src/a.rs,src/b.rs'\n  mergetopus hotfix --yes\n  mergetopus origin/main --quiet"
)]
pub struct Args {
    #[command(subcommand)]
    pub command: Option<Commands>,

    #[arg(
        value_name = "SOURCE",
        help = "Source branch/ref to merge into the integration branch",
        long_help = "Source branch/ref/commit-ish to merge.\n\nAccepted forms include local branches (feature/foo), remote-tracking refs (origin/main), tags, and commit SHAs.\nIf omitted, Mergetopus opens an interactive branch picker unless --quiet is set."
    )]
    pub source: Option<String>,

    #[arg(
        long,
        value_name = "CSV_PATHS",
        help = "Comma-separated conflicted file paths to group into one explicit slice",
        long_help = "Comma-separated list of conflicted file paths to include in a single explicit slice group.\n\nAny conflicted file not listed here is handled as a default one-file slice branch.\nExample: --select-paths 'src/lib.rs,src/main.rs,README.md'"
    )]
    pub select_paths: Option<String>,

    #[arg(
        long,
        default_value_t = false,
        help = "Run non-interactively and never open TUI screens",
        long_help = "Run in non-interactive mode suitable for CI/CD.\n\nBehavior changes:\n- SOURCE must be provided explicitly (no source picker)\n- Consolidation prompts are skipped unless --yes is provided\n- Conflict grouping comes only from --select-paths (no interactive conflict selector)"
    )]
    pub quiet: bool,

    #[arg(
        long,
        default_value_t = false,
        help = "Auto-confirm prompts when safe to proceed",
        long_help = "Assume 'yes' for non-destructive confirmation prompts.\n\nCurrently used when an existing integration branch already has all slices merged and Mergetopus asks whether to create a consolidated merge-commit branch."
    )]
    pub yes: bool,
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
    Resolve {
        /// Slice branch to resolve.  When omitted an interactive TUI picker is shown.
        #[arg(value_name = "BRANCH")]
        branch: Option<String>,
    },
}
