use crate::Revision;

#[derive(Debug, Clone)]
pub struct BuildInfo {
    pub revision: Revision,
    pub version: Option<String>,
    pub id: String,
}

impl BuildInfo {
    pub fn revision(revision: Revision) -> Self {
        Self {
            revision,
            version: None,
            id: revision.to_string(),
        }
    }

    pub fn version(version: String, revision: Revision) -> Self {
        Self {
            revision,
            version: Some(version.clone()),
            id: version,
        }
    }
}
