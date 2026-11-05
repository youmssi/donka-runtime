//! The connector host lists as the Runtime reads them from its environment (DNK-84).

use agent::config::EnvironmentConfig;
use config::{Config, Environment};
use std::collections::HashMap;

fn read(variables: &[(&str, &str)]) -> EnvironmentConfig {
    let source = variables
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect::<HashMap<_, _>>();
    Config::builder()
        .add_source(
            Environment::default()
                .separator("__")
                .try_parsing(true)
                .source(Some(source)),
        )
        .set_default("provider.type", "Zip")
        .unwrap()
        .build()
        .unwrap()
        .try_deserialize()
        .unwrap()
}

#[test]
fn hosts_are_read_per_secret_and_for_every_call() {
    let config = read(&[
        (
            "CONNECTORS__ALLOWED_HOSTS",
            "api.bureau.cm, cbs.bank.internal:8443,",
        ),
        ("CONNECTORS__HOSTS__BUREAU_KEY", "api.bureau.cm"),
        (
            "CONNECTORS__HOSTS__CBS_CLIENT_SECRET",
            "cbs.bank.internal:8443,login.bank.internal",
        ),
    ]);
    assert_eq!(
        config.connectors.allowed_hosts,
        ["api.bureau.cm", "cbs.bank.internal:8443"]
    );
    assert_eq!(config.connectors.hosts["BUREAU_KEY"], ["api.bureau.cm"]);
    assert_eq!(
        config.connectors.hosts["CBS_CLIENT_SECRET"],
        ["cbs.bank.internal:8443", "login.bank.internal"]
    );
}

#[test]
fn without_settings_no_call_is_allowed() {
    let config = read(&[]);
    assert!(config.connectors.allowed_hosts.is_empty());
    assert!(config.connectors.hosts.is_empty());
}
