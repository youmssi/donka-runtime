//! Secrets as files (DNK-65): every secret setting `X` can instead be given as `X_FILE`, the
//! path of a file holding the value, as Docker and Kubernetes secrets and vault agents
//! (OpenBao, Vault, Infisical) provide them. Trailing newlines are removed. Setting both forms,
//! or naming a file the Runtime cannot read, stops the Runtime at start and names the setting.
//!
//! The Runtime's own secrets are read here once, before the settings are parsed. Connector
//! secrets (`DONKA_SECRET_<NAME>`) are read at each call by the connectors crate, which reads
//! the file form itself: here they are only checked, so a mistake shows at start.

use donka_connectors::SECRET_ENV_PREFIX;
use std::collections::HashMap;

/// The Runtime's settings that hold a secret, as their environment variables.
pub const SECRET_SETTINGS: [&str; 10] = [
    "RELEASE_ZIP_PASSWORD",
    "PROVIDER__CONNECTION_STRING",
    "PROVIDER__BASE64_CONTENTS",
    "HTTP_SSL__KEY",
    "HTTP_SSL__CERT",
    "DECISION_LOG__TOKEN",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AZURE_CLIENT_SECRET",
];

const FILE_SUFFIX: &str = "_FILE";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SecretFileError {
    #[error("{name} and {name}_FILE are both set: keep one")]
    BothSet { name: String },
    #[error("{name}_FILE cannot be read: {reason}")]
    Unreadable { name: String, reason: String },
}

/// The secret settings to set from their files, given the process environment. Connector
/// secrets are checked but not returned.
pub fn resolve(vars: &HashMap<String, String>) -> Result<Vec<(String, String)>, SecretFileError> {
    let set = |name: &str| vars.get(name).is_some_and(|value| !value.trim().is_empty());
    let mut connector_secrets: Vec<&str> = vars
        .keys()
        .filter_map(|key| key.strip_suffix(FILE_SUFFIX))
        .filter(|name| name.starts_with(SECRET_ENV_PREFIX))
        .collect();
    connector_secrets.sort_unstable();

    let mut resolved = Vec::new();
    for name in SECRET_SETTINGS.iter().copied().chain(connector_secrets) {
        let Some(path) = vars
            .get(&format!("{name}{FILE_SUFFIX}"))
            .map(|path| path.trim())
            .filter(|path| !path.is_empty())
        else {
            continue;
        };
        if set(name) {
            return Err(SecretFileError::BothSet {
                name: name.to_owned(),
            });
        }
        let value = std::fs::read_to_string(path).map_err(|err| SecretFileError::Unreadable {
            name: name.to_owned(),
            reason: err.to_string(),
        })?;
        if SECRET_SETTINGS.contains(&name) {
            resolved.push((
                name.to_owned(),
                value.trim_end_matches(['\n', '\r']).to_owned(),
            ));
        }
    }
    Ok(resolved)
}

/// Sets each secret setting given as a file from its file. Runs first thing at start, before
/// the settings are read or any task is spawned.
pub fn load() -> Result<(), SecretFileError> {
    let vars: HashMap<String, String> = std::env::vars().collect();
    for (name, value) in resolve(&vars)? {
        // SAFETY: called at the very start of `main`, before anything else reads or writes
        // the environment from another thread.
        unsafe { std::env::set_var(name, value) };
    }
    Ok(())
}

#[cfg(test)]
#[path = "secret_files/tests.rs"]
mod tests;
