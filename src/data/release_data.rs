use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::data::access::{AccessTokenHash, lenient_hashes};

/// `.config/project.json` as shipped by BRMS `deployReleaseToEnv`.
///
/// Parsing is deliberately lenient: BRMS writes explicit `null`s
/// (`project.key` is nullable), pre-GRL-614 releases lack the newer fields
/// entirely, and hand-written configs may carry only a subset. A missing
/// field must degrade that field, never the whole config — a failed
/// whole-config parse disables token auth (`can_access` treats "no config"
/// as open access).
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseData {
    pub version: Option<Arc<str>>,
    pub project: Option<ReleaseDataProject>,
    #[serde(default)]
    pub access_tokens: Vec<Arc<str>>,
    /// Donka: hashes of the tokens issued for this environment (DNK-13).
    #[serde(default, deserialize_with = "lenient_hashes")]
    pub access_token_hashes: Vec<AccessTokenHash>,
    pub release: Option<ReleaseDataRelease>,
    pub environment: Option<ReleaseDataEnvironment>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseDataProject {
    pub id: Option<Arc<str>>,
    pub key: Option<Arc<str>>,
    pub name: Option<Arc<str>>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseDataRelease {
    pub id: Option<Arc<str>>,
    pub version: Option<Arc<str>>,
    pub name: Option<Arc<str>>,
    /// Opaque pass-through: BRMS's ReleaseStatus has more values than the
    /// advertised `draft | published`; the agent does not validate it.
    pub status: Option<Arc<str>>,
    pub commit_id: Option<Arc<str>>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseDataEnvironment {
    pub id: Option<Arc<str>>,
    pub key: Option<Arc<str>>,
    pub name: Option<Arc<str>>,
}

/// Which `meta.type` the /rules surface can truthfully claim for a config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MetaKind {
    Environment,
    Release,
}

impl ReleaseData {
    pub fn project_id(&self) -> Option<&Arc<str>> {
        self.project.as_ref()?.id.as_ref()
    }

    pub fn project_key(&self) -> Option<&Arc<str>> {
        self.project.as_ref()?.key.as_ref()
    }

    pub fn release_id(&self) -> Option<&Arc<str>> {
        self.release.as_ref()?.id.as_ref()
    }

    pub fn release_version(&self) -> Option<&Arc<str>> {
        self.release.as_ref()?.version.as_ref()
    }

    pub fn environment_id(&self) -> Option<&Arc<str>> {
        self.environment.as_ref()?.id.as_ref()
    }

    /// The meta emission decision shared by the evaluate response and the
    /// OpenAPI document: `None` without both identities (no meta at all),
    /// `Environment` when the deployed environment is known, `Release`
    /// otherwise.
    pub fn meta_kind(&self) -> Option<MetaKind> {
        self.project_id()?;
        self.release_id()?;
        Some(match self.environment_id() {
            Some(_) => MetaKind::Environment,
            None => MetaKind::Release,
        })
    }

    /// Lenient-parse `.config/project.json`. `None` (with a warning) only
    /// for unusable input — non-JSON or wrong-typed fields. A `None` here
    /// disables token auth, so it must never pass silently.
    pub fn from_json_reader(reader: impl std::io::Read) -> Option<Self> {
        match serde_json::from_reader::<_, ReleaseData>(reader) {
            Ok(data) => Some(data),
            Err(error) => {
                tracing::warn!(
                    ?error,
                    "Failed to parse .config/project.json; release metadata and access tokens will be ignored"
                );
                None
            }
        }
    }
}

#[cfg(test)]
#[path = "release_data/tests.rs"]
mod tests;
