use std::env;
use std::fs::File;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use crate::Agent;
use crate::config::{GlobalAgentConfig, ZipProviderConfig};
use crate::immutable_loader::{ImmutableLoader, ProtectedZipArchive};
use crate::provider::{
    AgentData, AgentDataProvider, FailedProjectsRegistry, Project, ProjectData, ProjectDiff,
};
use anyhow::Context;
use dashmap::DashMap;
use itertools::Itertools;
use tokio::task;
use walkdir::WalkDir;
use zip::ZipArchive;

#[derive(Debug)]
pub struct ZipProvider {
    root_dir: PathBuf,
    global_config: Arc<GlobalAgentConfig>,
    watch: bool,
}

impl ZipProvider {
    pub fn new(config: &ZipProviderConfig, global_config: Arc<GlobalAgentConfig>) -> Self {
        let root = env::current_dir()
            .expect("Current directory is available")
            .join(config.root_dir.as_str())
            .to_path_buf();

        Self {
            root_dir: root,
            global_config,
            watch: config.watch,
        }
    }

    /// Donka: the folder is reloaded every poll interval (DNK-64).
    pub fn watches(&self) -> bool {
        self.watch
    }
}

impl AgentDataProvider for ZipProvider {
    fn load_data(
        &self,
        data: Arc<AgentData>,
    ) -> impl Future<Output = anyhow::Result<Vec<ProjectDiff>>> + Send + 'static {
        let root = self.root_dir.clone();
        let password = self.global_config.release_zip_password.clone();
        let watch = self.watch;

        async move {
            if watch {
                return load_changed(root, password, data).await;
            }
            let projects = task::spawn_blocking(|| load_from_directory(root, password)).await?;
            let diff = projects
                .iter()
                .map(|project| ProjectDiff::Created(project.key().to_string()))
                .collect();

            projects.into_iter().for_each(|(key, project)| {
                let _ = data.projects.insert(key, project);
            });

            Ok(diff)
        }
    }
}

fn load_from_directory(root: PathBuf, password: Option<Arc<str>>) -> DashMap<String, Arc<Project>> {
    let files = match WalkDir::new(root.clone())
        .max_depth(1)
        .into_iter()
        .filter_ok(|d| d.file_type().is_file())
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(dirs) => dirs,
        Err(error) => {
            tracing::error!(
                "[Zip -Skip all] Failed to read directory {}: {}",
                root.display(),
                error
            );
            return DashMap::new();
        }
    };

    let projects = files
        .iter()
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|s| s.ends_with(".zip"))
        })
        .filter_map(|entry| {
            let file_reader = match File::open(entry.path()).context("failed to open file") {
                Ok(file_reader) => file_reader,
                Err(err) => {
                    tracing::error!(
                        "[Zip -Skip] failed to open zip file {}: {}",
                        entry.path().display(),
                        err
                    );
                    return None;
                }
            };
            let path = match entry.path().strip_prefix(&root) {
                Ok(stripped) => stripped
                    .to_string_lossy()
                    .trim_end_matches(".zip")
                    .to_string(),
                Err(err) => {
                    tracing::error!(
                        "[Zip -Skip] failed to strip prefix on {}: {}",
                        entry.path().display(),
                        err
                    );
                    return None;
                }
            };

            let archive = ProtectedZipArchive {
                archive: match ZipArchive::new(file_reader) {
                    Ok(archive) => archive,
                    Err(err) => {
                        tracing::error!(
                            "[Zip -Skip] failed unpack zip archive {}: {}",
                            entry.path().display(),
                            err
                        );
                        return None;
                    }
                },
                password: password.clone(),
            };

            Some((
                path,
                Arc::new(Project {
                    engine: match ImmutableLoader::try_from(archive) {
                        Ok(loader) => loader.into_engine(),
                        Err(err) => {
                            tracing::error!(
                                "[Zip -Skip] failed load into engine {}: {}",
                                entry.path().display(),
                                err
                            );
                            return None;
                        }
                    },
                    content_hash: None,
                    rules_spec: OnceLock::new(),
                }),
            ))
        })
        .collect::<DashMap<_, _>>();

    projects
}

/// Donka (DNK-64): a release folder Studio publishes to, such as a disk volume both share.
/// Studio writes each project's artifact as one file named after the project, with no
/// extension, and replaces it atomically; only files that changed since the last poll are read
/// again, and a project whose file is gone is removed.
async fn load_changed(
    root: PathBuf,
    password: Option<Arc<str>>,
    data: Arc<AgentData>,
) -> anyhow::Result<Vec<ProjectDiff>> {
    let listed = task::spawn_blocking(move || list_artifacts(&root)).await?;
    let current = listed
        .iter()
        .filter(|artifact| !FailedProjectsRegistry::has_failed(Some(&artifact.hash)))
        .map(|artifact| ProjectData {
            key: artifact.key.clone(),
            content_hash: Some(artifact.hash.clone()),
        })
        .collect();
    let diff = data.calculate_diff(current);
    let to_load = Agent::get_refresh_list(&diff);
    let loaded = task::spawn_blocking(move || {
        listed
            .into_iter()
            .filter(|artifact| to_load.contains(&artifact.key))
            .filter_map(|artifact| {
                let project = load_artifact(&artifact, password.clone());
                if project.is_none() {
                    FailedProjectsRegistry::insert(artifact.hash.clone());
                }
                project.map(|project| (artifact.key, Arc::new(project)))
            })
            .collect::<DashMap<_, _>>()
    })
    .await?;
    Ok(Agent::get_diff_result(data, diff, loaded))
}

/// A project's artifact in the release folder.
struct Artifact {
    key: String,
    path: PathBuf,
    /// Changes whenever the file is replaced: its name, size and modification time.
    hash: Vec<u8>,
}

/// The artifacts directly in `root`: files named after their project, with no extension, or
/// with `.zip`. Hidden files and the temporary files of a write in progress are skipped.
fn list_artifacts(root: &Path) -> Vec<Artifact> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(err) => {
            tracing::warn!("[Zip - Skip all] cannot read {}: {}", root.display(), err);
            return Vec::new();
        }
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let key = name.strip_suffix(".zip").unwrap_or(&name);
            if key.is_empty() || key.starts_with('.') || key.contains(['.', '#']) {
                return None;
            }
            let meta = entry.metadata().ok().filter(|meta| meta.is_file())?;
            let modified = meta
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_nanos();
            Some(Artifact {
                key: key.to_owned(),
                path: entry.path(),
                hash: format!("{name}:{}:{modified}", meta.len()).into_bytes(),
            })
        })
        .collect()
}

fn load_artifact(artifact: &Artifact, password: Option<Arc<str>>) -> Option<Project> {
    let file = File::open(&artifact.path)
        .map_err(|err| {
            tracing::error!(
                "[Zip - Skip] cannot open {}: {}",
                artifact.path.display(),
                err
            )
        })
        .ok()?;
    let archive = ZipArchive::new(file)
        .map_err(|err| {
            tracing::error!(
                "[Zip - Skip] not a zip archive {}: {}",
                artifact.path.display(),
                err
            )
        })
        .ok()?;
    let loader = ImmutableLoader::try_from(ProtectedZipArchive { archive, password })
        .map_err(|err| {
            tracing::error!(
                "[Zip - Skip] failed load into engine {}: {}",
                artifact.path.display(),
                err
            )
        })
        .ok()?;
    Some(Project {
        engine: loader.into_engine(),
        content_hash: Some(artifact.hash.clone()),
        rules_spec: OnceLock::new(),
    })
}
