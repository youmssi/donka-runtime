use crate::data::release_data::{MetaKind, ReleaseData};
use serde::Serialize;
use std::sync::Arc;

/// The `meta` object appended to successful /rules evaluate responses — the
/// agent-side subset of BRMS `EvaluateMeta` (rules.handler.ts). An agent
/// serves a release deployed to an environment, so the only kinds it can
/// truthfully claim are `environment` (configs carrying the environment
/// block) and `release` (older configs without it).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EvaluationMeta {
    #[serde(rename = "type")]
    pub kind: MetaKind,
    pub path: Arc<str>,
    pub project: MetaProject,
    pub release: MetaRelease,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<MetaEnvironment>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MetaProject {
    pub id: Arc<str>,
    pub key: Option<Arc<str>>,
    pub name: Option<Arc<str>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaRelease {
    pub id: Arc<str>,
    pub name: Option<Arc<str>>,
    pub version: Option<Arc<str>>,
    pub status: Option<Arc<str>>,
    pub commit_id: Option<Arc<str>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MetaEnvironment {
    pub id: Arc<str>,
    pub key: Option<Arc<str>>,
    pub name: Option<Arc<str>>,
}

impl EvaluationMeta {
    /// Unknown metadata serializes as explicit `null`s (BRMS null-coalesces
    /// every field), except `environment`, which is omitted entirely for
    /// `release` kind — the published schema requires it only for
    /// `environment` kind.
    pub fn from_release_data(release_data: Option<&ReleaseData>, path: &str) -> Option<Self> {
        let rd = release_data?;
        let kind = rd.meta_kind()?;
        let project = rd.project.as_ref()?;
        let release = rd.release.as_ref()?;

        let environment = match kind {
            MetaKind::Environment => {
                let environment = rd.environment.as_ref()?;
                Some(MetaEnvironment {
                    id: environment.id.clone()?,
                    key: environment.key.clone(),
                    name: environment.name.clone(),
                })
            }
            MetaKind::Release => None,
        };

        Some(Self {
            kind,
            path: Arc::from(path),
            project: MetaProject {
                id: project.id.clone()?,
                key: project.key.clone(),
                name: project.name.clone(),
            },
            release: MetaRelease {
                id: release.id.clone()?,
                name: release.name.clone(),
                version: release.version.clone(),
                status: release.status.clone(),
                commit_id: release.commit_id.clone(),
            },
            environment,
        })
    }
}

#[cfg(test)]
#[path = "evaluation_meta/tests.rs"]
mod tests;
