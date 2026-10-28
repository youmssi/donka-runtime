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
mod tests {
    use super::*;

    /// A file holding `content`, removed when the test ends.
    struct SecretFile(std::path::PathBuf);

    impl SecretFile {
        fn new(name: &str, content: &str) -> Self {
            let path = std::env::temp_dir().join(format!("donka-rt-{}-{name}", std::process::id()));
            std::fs::write(&path, content).unwrap();
            Self(path)
        }

        fn path(&self) -> String {
            self.0.to_string_lossy().into_owned()
        }
    }

    impl Drop for SecretFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn a_secret_setting_is_read_from_its_file_without_trailing_newlines() {
        let token = SecretFile::new("token", "dnk_log_abc\n\n");
        let key = SecretFile::new("aws", "minio-secret\r\n");
        let resolved = resolve(&vars(&[
            ("DECISION_LOG__TOKEN_FILE", &token.path()),
            ("AWS_SECRET_ACCESS_KEY_FILE", &key.path()),
            ("PROVIDER__TYPE", "S3"),
        ]))
        .unwrap();
        assert_eq!(
            resolved,
            vec![
                ("DECISION_LOG__TOKEN".to_owned(), "dnk_log_abc".to_owned()),
                (
                    "AWS_SECRET_ACCESS_KEY".to_owned(),
                    "minio-secret".to_owned()
                ),
            ]
        );
    }

    #[test]
    fn both_forms_or_an_unreadable_file_fail_and_name_the_setting() {
        let token = SecretFile::new("both", "dnk_log_abc");
        assert_eq!(
            resolve(&vars(&[
                ("DECISION_LOG__TOKEN", "dnk_log_other"),
                ("DECISION_LOG__TOKEN_FILE", &token.path()),
            ])),
            Err(SecretFileError::BothSet {
                name: "DECISION_LOG__TOKEN".into()
            })
        );
        let error =
            resolve(&vars(&[("HTTP_SSL__KEY_FILE", "/nonexistent/donka-key")])).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("HTTP_SSL__KEY_FILE cannot be read: "),
            "{error}"
        );
        // An empty variable counts as unset, as an empty compose default would be.
        assert!(
            resolve(&vars(&[
                ("DECISION_LOG__TOKEN", ""),
                ("DECISION_LOG__TOKEN_FILE", &token.path()),
            ]))
            .is_ok()
        );
    }

    #[test]
    fn connector_secrets_are_checked_at_start_and_left_to_the_connectors() {
        let cert = SecretFile::new("cert", "-----BEGIN CERTIFICATE-----");
        let checked = vars(&[("DONKA_SECRET_CBS_CERT_FILE", &cert.path())]);
        assert_eq!(resolve(&checked), Ok(vec![]), "read at each call, not here");
        assert_eq!(
            resolve(&vars(&[
                ("DONKA_SECRET_CBS_CERT", "inline"),
                ("DONKA_SECRET_CBS_CERT_FILE", &cert.path()),
            ])),
            Err(SecretFileError::BothSet {
                name: "DONKA_SECRET_CBS_CERT".into()
            })
        );
        assert!(matches!(
            resolve(&vars(&[("DONKA_SECRET_CBS_KEY_FILE", "/nonexistent/key")])),
            Err(SecretFileError::Unreadable { name, .. }) if name == "DONKA_SECRET_CBS_KEY"
        ));
    }
}
