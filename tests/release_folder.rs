//! Studio's release folder (DNK-64): the Zip provider with `watch` reads Studio's artifacts,
//! which have no extension, and reloads the folder when they change.

use agent::app;
use agent::config::{EnvironmentConfig, ProviderConfig, ZipProviderConfig};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A fresh, empty folder for one test.
fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "donka-release-folder-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Writes `source` as `name` the way Studio does: a temporary file, then a rename.
fn publish(dir: &Path, name: &str, source: &str) {
    let temp = dir.join(format!("{name}#1"));
    std::fs::copy(format!("tests/data/{source}"), &temp).unwrap();
    std::fs::rename(&temp, dir.join(name)).unwrap();
}

fn config(dir: &Path) -> EnvironmentConfig {
    EnvironmentConfig {
        provider: ProviderConfig::Zip(ZipProviderConfig {
            root_dir: dir.to_string_lossy().into_owned(),
            watch: true,
        }),
        poll_interval: Duration::from_millis(1_000),
        ..Default::default()
    }
}

/// Waits up to ten seconds for `check` to hold.
async fn eventually(mut check: impl FnMut() -> bool) -> bool {
    for _ in 0..50 {
        if check() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    false
}

#[tokio::test]
async fn studio_artifacts_are_served_and_reloaded_when_they_change() {
    let dir = folder("reload");
    publish(&dir, "credit-pme", "sample-project.zip");
    // Not artifacts: a hidden file and a write in progress.
    std::fs::write(dir.join(".DS_Store"), b"x").unwrap();
    std::fs::write(dir.join("retail#tmp"), b"partial").unwrap();

    let agent = app::create_agent(config(&dir), Default::default()).await;
    let first = agent
        .project("credit-pme")
        .expect("served at start")
        .content_hash
        .clone();
    assert!(agent.project("retail").is_none());

    // A new deployment replaces the file: the new release is served without a restart.
    tokio::time::sleep(Duration::from_millis(20)).await;
    publish(&dir, "credit-pme", "SampleProject.zip");
    assert!(
        eventually(|| agent
            .project("credit-pme")
            .is_some_and(|p| p.content_hash != first))
        .await,
        "the new artifact was not loaded"
    );

    // A new project appears; one whose file is gone is removed.
    publish(&dir, "retail", "sample-project.zip");
    assert!(eventually(|| agent.project("retail").is_some()).await);
    std::fs::remove_file(dir.join("credit-pme")).unwrap();
    assert!(eventually(|| agent.project("credit-pme").is_none()).await);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_broken_artifact_is_skipped_and_others_keep_serving() {
    let dir = folder("broken");
    publish(&dir, "credit-pme", "sample-project.zip");
    std::fs::write(dir.join("broken"), b"not a zip").unwrap();

    let agent = app::create_agent(config(&dir), Default::default()).await;
    assert!(agent.project("credit-pme").is_some());
    assert!(agent.project("broken").is_none());

    // Fixed later: the corrected file is loaded.
    tokio::time::sleep(Duration::from_millis(20)).await;
    publish(&dir, "broken", "sample-project.zip");
    assert!(eventually(|| agent.project("broken").is_some()).await);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_missing_folder_serves_nothing_until_studio_creates_it() {
    let dir = folder("missing");
    std::fs::remove_dir_all(&dir).unwrap();

    let agent = app::create_agent(config(&dir), Default::default()).await;
    assert!(agent.project("credit-pme").is_none());

    std::fs::create_dir_all(&dir).unwrap();
    publish(&dir, "credit-pme", "sample-project.zip");
    assert!(eventually(|| agent.project("credit-pme").is_some()).await);
    let _ = std::fs::remove_dir_all(&dir);
}
