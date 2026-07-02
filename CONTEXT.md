# Mergetopus - Architecture Overview

**Version:** 0.6.0  
**Language:** Rust (Edition 2024)  
**Purpose:** Git merge orchestration tool for splitting large, conflict-heavy merges into manageable, parallelizable tasks

---

## Table of Contents

1. [Project Overview](#project-overview)
2. [Technology Stack](#technology-stack)
3. [Architecture](#architecture)
4. [Project Structure](#project-structure)
5. [Core Modules](#core-modules)
6. [Data Flow](#data-flow)
7. [Build and Deployment](#build-and-deployment)
8. [Testing Infrastructure](#testing-infrastructure)
9. [Key Concepts](#key-concepts)

---

## Project Overview

Mergetopus transforms high-risk, large-scale Git merges into manageable tasks by:

1. **Splitting** a single risky merge into:
   - One integration branch for non-conflicting auto-merged files
   - Multiple slice branches for isolated conflict groups

2. **Parallelizing** conflict resolution:
   - Multiple developers resolve different slices concurrently
   - Each slice focuses on semantically related files
   - Progress is visible and testable incrementally

3. **Preserving history:**
   - Kokomeco (KOrrekt KOnsoliderter MErge COmmit) branch for proper merge ancestry
   - Maintains authorship through git trailers

### Core Problem Solved

Traditional Git merges with hundreds or thousands of conflicts block development and force a single person to resolve all conflicts sequentially. Mergetopus removes this "single merge hero" bottleneck by enabling parallel conflict resolution distributed to domain experts.

### Use Cases

- **Long-lived branch merges** (months/years of divergence)
- **LTS version cascade merges** (forward-porting fixes across versions)
- **Large team merges** (distributing work by domain expertise)
- **Risky refactorings** (breaking down structural changes)

---

## Technology Stack

### Core Dependencies

- **CLI Framework:** `clap` v4 - command-line argument parsing with derive macros
- **Error Handling:** `anyhow` v1 - flexible error handling
- **TUI Framework:** `ratatui` v0.30 - terminal user interface components
- **Terminal Control:** `crossterm` v0.29 - cross-platform terminal manipulation
- **Serialization:** `serde` v1 + `serde_json` v1 - JSON handling
- **Error Types:** `thiserror` v2 - custom error type derivation
- **Colors:** `owo-colors` v4 - terminal color styling
- **Streaming:** `anstream` v0.6 - ANSI stream handling

### Build & Distribution

- **Build System:** Cargo + PowerShell InvokeBuild
- **License Bundling:** `cargo-bundle-licenses` - generates THIRDPARTY.json at build time
- **Packaging:** NuGet/Chocolatey (Windows), tar.gz archives (Linux/macOS)
- **CI/CD:** GitHub Actions (platform-specific workflows)

---

## Architecture

### Design Principles

- **Non-destructive:** Never rewrites history; uses separate branches
- **Git-native:** Works with standard Git tools and workflows
- **Cross-platform:** Windows, macOS, Linux support
- **Team-friendly:** Interactive TUI for humans, `--quiet` mode for CI/CD
- **Expertise-routing:** Slices can be assigned to domain experts
- **Incremental progress:** Test and validate as slices are resolved

### Execution Flow

```
main()
  → Args::parse()                    // Parse CLI arguments with clap
  → ColorConfig::new()               // Initialize color configuration
  → commands::run()                  // Dispatch to command handler
    → match subcommand:
      - License → cmd_license::print_license()
      - Resolve → cmd_resolve::resolve_command()
      - Status → cmd_status::status_command()
      - Cleanup → cmd_cleanup::cleanup_command()
      - Verify → cmd_verify::verify_command()
      - Here → cmd_here::here_command()
      - Push → cmd_push::push_command()
      - None → cmd_merge_workflow::run_merge_workflow()
```

### Branch Structure

```
main (target)                      A---B---C
                                        \
source branch                            D---E---F
                             
Integration branch:
_mmm/main/source/integration       C---M (partial: auto-merged only)
                                   |
Slice branches (from merge-base):
_mmm/main/source/slice1            B---S1 (conflicts: file1, file2)
_mmm/main/source/slice2            B---S2 (conflicts: file3)

After resolution:
_mmm/main/source/integration       C---M---(merge S1)---(merge S2)

Kokomeco (consolidation):
_mmm/main/source/kokomeco          parents: (C, F), tree: final integration
```

### Branch Naming Conventions

- **Integration:** `_mmm/<target>/<source>/integration`
- **Slices:** `_mmm/<target>/<source>/slice<N>`
- **Kokomeco:** `_mmm/<target>/<source>/kokomeco`

All branch names are sanitized to ensure Git compatibility.

---

## Project Structure

```
mergetopus/
├── src/                      # Rust source code
│   ├── main.rs              # Application entry point
│   ├── cli.rs               # Command-line interface (clap)
│   ├── commands/            # Command implementations
│   │   ├── mod.rs           # Command dispatcher
│   │   ├── cmd_merge_workflow.rs  # Main merge initialization
│   │   ├── cmd_resolve.rs   # Slice conflict resolution
│   │   ├── cmd_status.rs    # Status reporting
│   │   ├── cmd_cleanup.rs   # Branch cleanup
│   │   ├── cmd_verify.rs    # Integration verification
│   │   ├── cmd_push.rs      # Push to remotes
│   │   ├── cmd_here.rs      # Take over manual merges
│   │   └── cmd_license.rs   # License display
│   ├── git_ops/             # Git operations (organized by concern)
│   │   ├── mod.rs           # Core git command execution
│   │   ├── branch.rs        # Branch management
│   │   ├── checkout.rs      # Checkout operations
│   │   ├── commit.rs        # Commit operations
│   │   ├── diff.rs          # Diff operations
│   │   ├── merge.rs         # Merge operations
│   │   ├── refs.rs          # Git reference operations
│   │   └── worktree.rs      # Worktree management
│   ├── planner.rs           # Branch naming and slice planning
│   ├── tui.rs               # Terminal UI components
│   ├── models.rs            # Core data structures
│   ├── helpers.rs           # Utility functions
│   ├── color.rs             # Colored output configuration
│   └── test_support.rs      # Test utilities (conditional compilation)
├── tests/                   # Integration tests
│   ├── test_helpers.rs      # Common test utilities
│   └── test_suite_*.rs      # Test suites (a-e)
├── assets/                  # Logo and branding (SVG files)
├── docs/                    # Documentation
│   └── adr/                 # Architecture Decision Records (empty)
├── nuget/                   # NuGet/Chocolatey packaging
├── ext/                     # External resources
├── third_party/             # Third-party dependencies
├── .github/workflows/       # CI/CD workflows
├── build.rs                 # Build-time script (license bundling)
├── Cargo.toml              # Rust package manifest
└── mergetopus.build.ps1    # PowerShell build script
```

---

## Core Modules

### 1. Git Operations Module (`src/git_ops/`)

Organized by functional area, this module wraps all Git interactions:

#### `mod.rs` - Core Operations
- `run_git()` - Execute git commands with error handling
- `ensure_git_context()` - Verify clean working tree
- Path provenance tracking
- File operations
- Slice branch discovery

#### `branch.rs` - Branch Management
- Branch existence checks (local, remote, anywhere)
- Current branch detection
- Remote tracking branch operations

#### `checkout.rs` - Checkout Operations
- Safe checkout operations
- Branch creation/reset logic

#### `commit.rs` - Commit Operations
- Standard commits
- Slice commits with authorship preservation
- Staged changes detection

#### `diff.rs` - Diff Operations
- File content retrieval at specific refs
- Three-way diff generation

#### `merge.rs` - Merge Operations
- `merge_no_commit()` - Non-committing merge with conflict detection
- Merge-in-progress detection
- Merge base calculation

#### `refs.rs` - Reference Operations
- HEAD manipulation
- Ref resolution
- Remote ref enumeration

#### `worktree.rs` - Worktree Management
- Worktree detection and path inference
- Automatic worktree creation for branch operations

### 2. Commands Module (`src/commands/`)

Each command has its own module:

#### `cmd_merge_workflow.rs` - Main Workflow
- Source branch selection/validation
- Integration branch creation
- Conflict slicing and slice branch creation
- Main entry point for the merge orchestration process

#### `cmd_resolve.rs` - Conflict Resolution
- Merge tool invocation with LOCAL/BASE/REMOTE/MERGED env vars
- Conflict marker detection
- Trust exit code handling
- Resolves conflicts in individual slice branches

#### `cmd_status.rs` - Status Reporting
- Global MMM overview across branches
- Per-integration detailed status
- Shows pending/merged slices

#### `cmd_cleanup.rs` - Branch Cleanup
- Cleanup of temporary branches after merge completion
- Interactive branch selection

#### `cmd_verify.rs` - Integration Verification
- Validates that integration hasn't drifted from kokomeco
- Ensures merge integrity

#### `cmd_push.rs` - Remote Push
- Push integration plans to remotes
- Share merge state with team

#### `cmd_here.rs` - Takeover
- Takes over in-progress manual merges
- Converts existing merge state to MMM workflow

#### `cmd_license.rs` - License Display
- Displays bundled third-party license information
- Embedded at build time via build.rs

### 3. Planning Module (`src/planner.rs`)

Branch naming and slice planning logic:

- **Branch Sanitization:** Converts arbitrary branch names to safe identifiers
- **Branch Naming Conventions:** Generates MMM branch names
- **Slice Creation:** Materializes slice branches from conflict groups
- **Branch Pattern Matching:** Parses and validates MMM branch names

### 4. TUI Module (`src/tui.rs`)

Terminal user interface components using ratatui:

- **Branch Picker:** Interactive branch selection with filtering
- **Conflict Selector:** Multi-pane interface for grouping conflicts into slices
  - File list navigation
  - Slice group management
  - F3 difftool integration
  - 3-way diff overlay viewer
- **Option Picker:** Generic selection dialog
- **Terminal Guard:** RAII pattern for terminal mode management (raw mode cleanup)

### 5. CLI Module (`src/cli.rs`)

Command-line interface definition using clap:

- Main `Args` struct with global flags:
  - `--quiet` - Suppress interactive prompts
  - `--yes` - Auto-confirm prompts
  - `--select-paths` - Filter specific paths
  - `--color` - Color output control
- Subcommands enum defining all available commands
- Help text and documentation strings

### 6. Supporting Modules

#### `models.rs` - Data Structures
- `SlicePlanItem` - Represents a planned slice with file paths
- `PathProvenance` - Tracks file origin (target, source, both)

#### `helpers.rs` - Utility Functions
- Command parsing (e.g., `parse_difftool_command`)
- Trailer extraction from commit messages
- Common string manipulation

#### `color.rs` - Colored Output
- `ColorConfig` - Auto-detection based on terminal capabilities
- Wrappers for styled output with owo-colors

#### `test_support.rs` - Test Helpers
- Test-only functions with conditional compilation
- Utilities for test suite operations

---

## Data Flow

### Merge Workflow (Primary Flow)

1. **Initialization** (`cmd_merge_workflow.rs`)
   - User runs `mergetopus <source>`
   - Validate source branch exists
   - Validate clean working tree
   - Compute target branch (current HEAD)

2. **Conflict Detection** (`git_ops/merge.rs`)
   - Attempt merge with `--no-commit`
   - Capture conflicting file paths
   - If no conflicts: complete merge normally
   - If conflicts: proceed to slicing

3. **Slice Planning** (`tui.rs` + `planner.rs`)
   - Launch interactive TUI (unless `--quiet`)
   - User groups conflicts into slices
   - Generate slice plan items

4. **Branch Materialization** (`planner.rs` + `git_ops/`)
   - Create integration branch at merge-base
   - Cherry-pick non-conflicting merge to integration
   - Create slice branches for each conflict group
   - Reset integration to target (remove partial merge)

5. **Resolution** (`cmd_resolve.rs`)
   - User runs `mergetopus resolve <slice>`
   - Checkout slice branch
   - Merge source with conflict isolation
   - Invoke merge tool for conflicts
   - Validate resolution (no conflict markers)
   - Commit with authorship preservation

6. **Integration** (`cmd_merge_workflow.rs` + `git_ops/merge.rs`)
   - After all slices resolved
   - Merge each slice into integration branch
   - Create kokomeco branch with proper ancestry

7. **Verification** (`cmd_verify.rs`)
   - Compare integration tree with kokomeco tree
   - Ensure no drift in merge result

8. **Cleanup** (`cmd_cleanup.rs`)
   - Remove temporary slice branches
   - Remove integration/kokomeco branches
   - Restore clean repository state

### Module Dependencies

```
main.rs
  ↓
cli.rs (clap) + color.rs
  ↓
commands/mod.rs
  ↓
  ├─→ cmd_merge_workflow.rs → planner.rs → git_ops/*
  ├─→ cmd_resolve.rs → helpers.rs → git_ops/*
  ├─→ cmd_status.rs → git_ops/*
  ├─→ cmd_cleanup.rs → tui.rs → git_ops/*
  ├─→ cmd_verify.rs → git_ops/*
  ├─→ cmd_push.rs → tui.rs → git_ops/*
  ├─→ cmd_here.rs → planner.rs → git_ops/*
  └─→ cmd_license.rs (embedded data)
```

---

## Build and Deployment

### Build System

#### PowerShell Build Script (`mergetopus.build.ps1`)

Uses InvokeBuild for task orchestration:

- **Build** - Standard cargo build (release mode)
- **BuildWin** - Cross-compilation for Windows target
- **Test** - Run cargo test suite
- **Pack** - Create NuGet/Chocolatey packages
- **Clean** - Remove build artifacts

#### Rust Build Process

1. **Build Script** (`build.rs`):
   - Runs `cargo bundle-licenses` to generate THIRDPARTY.json
   - Embeds license data into binary as static strings
   - Escapes content to prevent injection issues

2. **Compilation:**
   - `cargo build --release`
   - Edition 2024 Rust features enabled

### CI/CD Workflows

Three platform-specific GitHub Actions workflows (`.github/workflows/`):

#### `build-linux.yml`
- Ubuntu runner
- Installs InvokeBuild, cargo-bundle-licenses
- Runs Build and Test tasks
- Produces `mergetopus-linux-x64.tar.gz` with SHA256 checksum

#### `build-macos.yml`
- macOS runner
- Same structure as Linux
- Produces `mergetopus-macos-x64.tar.gz` with SHA256 checksum

#### `build-windows.yml`
- Windows runner
- Runs Build, Test, and Pack tasks
- Produces `.nupkg` (NuGet package) and `.zip` artifacts
- Generates SHA256 checksum for .exe

**Triggers:** Pull requests to main/develop or manual workflow dispatch

### Distribution Channels

1. **GitHub Releases** - Manual releases with platform-specific artifacts
2. **Chocolatey** - Windows package manager: `choco install mergetopus.portable`
3. **Cargo** - Rust package manager: `cargo install --git https://github.com/mwallner/mergetopus.git`

---

## Testing Infrastructure

### Test Organization

Located in `/tests/` directory:

#### `test_helpers.rs` - Common Utilities
- Temporary repo creation with unique IDs
- Git command wrappers for tests
- Mergetopus binary invocation
- Standard repo setup scenarios
- Worktree and remote setup helpers
- Cross-platform path handling

#### Test Suites
- **`test_suite_a.rs`** - Core merge workflow behavior (138 tests)
- **`test_suite_b.rs`** - Additional workflow tests
- **`test_suite_c.rs`** - Extended scenarios
- **`test_suite_d.rs`** - LTS cascade merge, authorship preservation
- **`test_suite_e.rs`** - Edge cases and special scenarios

### Test Infrastructure Features

- **Parallel Isolation:** Atomic unique ID generation for test repos
- **Platform Support:** Conditional compilation for Windows vs Unix
- **Git Configuration:** Disables GPG signing, sets test user
- **Path Handling:** Symlink resolution on macOS, UNC paths on Windows
- **Cleanup Strategy:** OS-managed temp directory cleanup
- **Concurrency Control:** `CWD_LOCK` mutex for directory-changing tests

### Running Tests

```bash
cargo test                    # All tests
cargo test -- --nocapture     # With output
cargo test test_suite_a       # Specific suite
```

### Test Coverage Areas

- Git operations (branches, merges, worktrees)
- Workflow commands (merge, resolve, status, verify, cleanup, push)
- Edge cases (HERE takeover, integration redirection)
- Authorship preservation in kokomeco branches
- Platform-specific behavior (Windows paths, merge tool invocation)
- Interactive TUI components (mocked in tests)

---

## Key Concepts

### 1. Three-Branch Pattern

**Integration Branch** - Contains non-conflicting auto-merged changes  
**Slice Branches** - Each contains a subset of conflicts to resolve  
**Kokomeco Branch** - Final consolidation with proper merge ancestry

### 2. Path Provenance

Tracks file origin during conflict detection:
- **Target:** File exists only in target branch
- **Source:** File exists only in source branch
- **Both:** File exists in both (modified on both sides)

### 3. Authorship Preservation

Slice commits preserve original author information through git trailers:
- `Mergetopus-Source-Branch:` - Source branch name
- `Mergetopus-Source-Commit:` - Source commit SHA
- Additional metadata for audit trail

### 4. Worktree Support

Automatically detects and creates worktrees for:
- Parallel slice resolution by multiple developers
- Isolation of resolution environments
- Clean separation of working states

### 5. Interactive TUI vs Quiet Mode

**Interactive Mode (default):**
- Branch picker for source selection
- Conflict selector for slice planning
- Diff viewer for file inspection
- Progress indicators and prompts

**Quiet Mode (`--quiet`):**
- Non-interactive operation for CI/CD
- Uses sane defaults
- Requires explicit selections via flags
- Error on ambiguous choices

### 6. Branch Sanitization

Converts arbitrary branch names to Git-safe identifiers:
- Replaces invalid characters
- Handles collisions with suffixes
- Maintains readability where possible

### 7. Conflict Isolation

Each slice branch:
- Starts from merge-base
- Contains only its assigned conflicts
- Can be resolved independently
- Merges cleanly into integration when resolved

### 8. Merge Tool Integration

Respects `merge.tool` configuration:
- Invokes configured tool with standard env vars
- Supports trust exit code (`mergetool.*.trustExitCode`)
- Fallback conflict marker detection
- F3 hotkey for difftool during slice planning

---

## Command Reference

### Primary Commands

```bash
# Initialize merge workflow
mergetopus <source>

# Resolve conflicts in a slice
mergetopus resolve <slice>

# Check merge status
mergetopus status [source]

# Verify integration integrity
mergetopus verify [source]

# Push merge state to remote
mergetopus push [remote]

# Clean up after merge completion
mergetopus cleanup

# Take over in-progress manual merge
mergetopus HERE

# Display license information
mergetopus license
```

### Global Flags

- `--quiet` - Suppress interactive prompts, use defaults
- `--yes` - Auto-confirm all prompts
- `--select-paths <paths>` - Filter specific paths during merge
- `--color <mode>` - Control color output (auto/always/never)

---

## Design Patterns

### RAII Pattern
- **Terminal Guard** - Ensures raw mode cleanup on panic
- **Git Context** - Validates and restores working tree state

### Command Pattern
- Subcommands as separate modules
- Dispatcher routes to appropriate handler
- Consistent error handling via anyhow

### Repository Pattern
- `git_ops` module abstracts Git CLI interactions
- Testable through command injection
- Platform-agnostic interface

### State Machine
- Merge workflow progresses through defined states
- Branch naming encodes workflow stage
- Status command queries current state

---

## Future Considerations

### Known Limitations (from TODO.md)

1. Temp directory cleanup in resolve command
2. Branch name collision via sanitization (rare)
3. HERE command ambiguity with multiple refs at same commit
4. Cleanup only checks local kokomeco branches (not remote)
5. Command parsing doesn't handle backslash-escaped quotes
6. Overlay scroll position u16 truncation (very long diffs)
7. File content trimming in show_file_at

### Extensibility Points

- **Additional merge strategies** - Beyond conflict slicing
- **Plugin system** - Custom slice grouping algorithms
- **Remote collaboration** - Push/pull slice assignments
- **Analytics** - Merge complexity metrics
- **Integration** - GitHub/GitLab PR integration

---

## References

- **Repository:** https://github.com/mwallner/mergetopus
- **License:** MIT
- **Rust Edition:** 2024
- **Minimum Rust Version:** (Check Cargo.toml for MSRV)

---

**Last Updated:** 2026-06-12  
**For LLMs:** This document provides comprehensive context for code understanding, modification, and extension tasks.
