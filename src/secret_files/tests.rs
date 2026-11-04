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
    let error = resolve(&vars(&[("HTTP_SSL__KEY_FILE", "/nonexistent/donka-key")])).unwrap_err();
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
