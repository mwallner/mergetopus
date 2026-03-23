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
