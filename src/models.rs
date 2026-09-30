#[derive(Debug, Clone)]
pub struct SlicePlanItem {
    pub path: String,
    pub branch: String,
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
