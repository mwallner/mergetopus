#[derive(Debug, Clone)]
pub struct SlicePlanItem {
    pub path: String,
    pub branch: String,
}

/// The conflict topology git's merge strategy reported for a set of paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictKind {
    /// Ordinary content conflict on a single path (possibly add/add).
    Content,
    /// One path, deleted on one side and modified on the other.
    ModifyDelete,
    /// Two paths: old name and one new name; deleted on the other side.
    RenameDelete,
    /// Three paths: old name plus ours/theirs rename targets.
    RenameRename,
    /// File added under a directory that was renamed on the other side;
    /// paths are the added (old-location) and suggested (new-location) paths.
    FileLocation,
}

/// An unmerged index stage entry: the blob OID plus its index mode
/// (e.g. `100644`, `100755`, `120000`, `160000`). The mode is preserved so
/// materialized stages keep the exec bit and symlink-ness of the original.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageBlob {
    pub mode: String,
    pub oid: String,
}

/// One logical conflict and every index path that participates in it.
/// Rename-related kinds keep their correlated paths together so slicing and
/// assignment treat the group as a unit.
#[derive(Debug, Clone)]
pub struct ConflictGroup {
    pub kind: ConflictKind,
    pub paths: Vec<String>,
    /// Captured unmerged index stages (`path -> stage -> blob`) for this
    /// group, taken before "restore ours" wiped the conflict stages.
    pub stage_blobs:
        std::collections::BTreeMap<String, std::collections::BTreeMap<usize, StageBlob>>,
}

impl ConflictKind {
    /// Human-readable tag for the conflict topology, used in TUI rows.
    pub fn label(&self) -> &'static str {
        match self {
            ConflictKind::Content => "content",
            ConflictKind::ModifyDelete => "modify/delete",
            ConflictKind::RenameDelete => "rename/delete",
            ConflictKind::RenameRename => "rename/rename",
            ConflictKind::FileLocation => "file location",
        }
    }
}

impl ConflictGroup {
    pub fn is_single(&self) -> bool {
        self.paths.len() <= 1
    }

    pub fn contains(&self, path: &str) -> bool {
        self.paths.iter().any(|p| p == path)
    }

    /// One-line label joining all member paths; multi-path groups carry the
    /// conflict topology as a leading tag so it survives pane truncation.
    pub fn display_label(&self) -> String {
        if self.is_single() {
            return self.paths.first().cloned().unwrap_or_default();
        }
        format!("[{}] {}", self.kind.label(), self.paths.join(" \u{21c4} "))
    }
}

#[derive(Debug, Clone)]
pub struct PathProvenance {
    pub source_ref: String,
    pub source_commit: String,
    pub path: String,
    pub path_commit: Option<String>,
    pub author_name: Option<String>,
    pub author_email: Option<String>,
    pub author_date: Option<String>,
}

/// How conflicted files that were not assigned to an explicit slice are turned
/// into slice branches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UnassignedPolicy {
    /// One slice branch per unassigned file.
    #[default]
    Separate,
    /// All unassigned files share a single slice branch.
    Single,
}

impl UnassignedPolicy {
    /// Whether every unassigned file gets its own slice branch.
    pub fn is_separate(&self) -> bool {
        matches!(self, UnassignedPolicy::Separate)
    }
}

/// Non-interactive (and explicit) policy for settling rename/delete conflict
/// groups during `mergetopus resolve`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GroupMode {
    /// Apply the slice's resolved decision (take their side / accept rename /
    /// accept deletion). Default for `--quiet`.
    #[default]
    Theirs,
    /// Keep the integration side's state (our content / original location /
    /// reject the rename).
    Ours,
    /// Preserve every side's content: both names/locations kept, old dropped.
    Both,
    /// Settle the group as a deletion.
    Delete,
    /// No group decision; fall through to the per-file merge tool.
    Tool,
}

impl GroupMode {
    pub fn is_tool(self) -> bool {
        self == GroupMode::Tool
    }
}

impl std::str::FromStr for GroupMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "theirs" => Ok(GroupMode::Theirs),
            "ours" => Ok(GroupMode::Ours),
            "both" => Ok(GroupMode::Both),
            "delete" => Ok(GroupMode::Delete),
            "tool" => Ok(GroupMode::Tool),
            _ => Err(format!(
                "invalid group mode '{s}'; use 'theirs', 'ours', 'both', 'delete', or 'tool'"
            )),
        }
    }
}

impl std::fmt::Display for GroupMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            GroupMode::Theirs => "theirs",
            GroupMode::Ours => "ours",
            GroupMode::Both => "both",
            GroupMode::Delete => "delete",
            GroupMode::Tool => "tool",
        };
        f.write_str(s)
    }
}

impl std::str::FromStr for UnassignedPolicy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "separate" | "s" => Ok(UnassignedPolicy::Separate),
            "single" | "one" => Ok(UnassignedPolicy::Single),
            _ => Err(format!(
                "invalid unassigned mode '{}'; use 'separate' or 'single'",
                s
            )),
        }
    }
}

impl std::fmt::Display for UnassignedPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UnassignedPolicy::Separate => write!(f, "separate"),
            UnassignedPolicy::Single => write!(f, "single"),
        }
    }
}
