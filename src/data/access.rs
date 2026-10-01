//! Donka access tokens (DNK-13): `.config/project.json` carries SHA-256
//! hashes of the tokens issued for one environment, never the tokens.
//!
//! A token is a long random string, so an unsalted SHA-256 is enough to keep
//! it out of the artifact; comparisons run in constant time. An artifact
//! deployed to an environment only accepts tokens issued for that
//! environment, so a staging token never opens production. Plain
//! `accessTokens` from older artifacts keep working.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::Arc;

use crate::data::release_data::ReleaseData;

/// The only hash algorithm artifacts use today; an entry naming another never matches.
pub const SHA256: &str = "sha256";

/// One issued token, as the artifact names it.
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessTokenHash {
    /// The token's id in Studio, to tell tokens apart without revealing them.
    pub id: Option<Arc<str>>,
    /// The environment the token was issued for (`staging`, `production`).
    pub environment: Option<Arc<str>>,
    pub algorithm: Option<Arc<str>>,
    /// Lowercase hex digest of the token.
    pub hash: Option<Arc<str>>,
}

/// The SHA-256 of a token as artifacts store it: lowercase hex.
pub fn hash_token(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Reads `accessTokenHashes` without ever failing the whole config: a config
/// that fails to parse disables token checks, so a malformed entry is skipped
/// (and logged) instead.
pub fn lenient_hashes<'de, D>(deserializer: D) -> Result<Vec<AccessTokenHash>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    let Value::Array(entries) = value else {
        if !value.is_null() {
            tracing::warn!("accessTokenHashes is not a list; it is ignored");
        }
        return Ok(Vec::new());
    };
    Ok(entries
        .into_iter()
        .filter_map(|entry| match serde_json::from_value(entry) {
            Ok(hash) => Some(hash),
            Err(error) => {
                tracing::warn!(?error, "Skipping a malformed accessTokenHashes entry");
                None
            }
        })
        .collect())
}

impl ReleaseData {
    /// Whether `token` may use this release: one of the plain tokens of an
    /// older artifact, or a token whose hash the artifact lists for its own
    /// environment.
    pub fn grants(&self, token: &str) -> bool {
        if token.is_empty() {
            return false;
        }
        let plain = self
            .access_tokens
            .iter()
            .any(|known| same(known.as_bytes(), token.as_bytes()));
        if plain {
            return true;
        }
        let environment = self.environment.as_ref().and_then(|env| env.key.as_deref());
        let digest = hash_token(token);
        self.access_token_hashes.iter().any(|entry| {
            entry.algorithm.as_deref() == Some(SHA256)
                && entry
                    .hash
                    .as_deref()
                    .is_some_and(|hash| same(hash.to_ascii_lowercase().as_bytes(), digest.as_bytes()))
                // An artifact deployed to an environment only takes that environment's tokens.
                && environment.is_none_or(|env| entry.environment.as_deref() == Some(env))
        })
    }
}

/// Compares two secrets in time that depends only on their length.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config(value: Value) -> ReleaseData {
        ReleaseData::from_json_reader(value.to_string().as_bytes()).expect("config must parse")
    }

    fn entry(environment: &str, token: &str) -> Value {
        json!({ "id": format!("t-{environment}"), "environment": environment,
                "algorithm": "sha256", "hash": hash_token(token) })
    }

    #[test]
    fn hashes_are_lowercase_hex_sha256() {
        // The vector Studio's artifact-format document publishes.
        assert_eq!(
            hash_token("dnk_test_token"),
            "d4d813b79f07c455e68458c955824329d902dd5f8e7b0c250fb3f05b3f68c840"
        );
    }

    #[test]
    fn a_token_opens_only_its_own_environment() {
        let production = config(json!({
            "version": "2",
            "environment": { "key": "production" },
            "accessTokenHashes": [entry("production", "prod-token"), entry("staging", "staging-token")]
        }));
        assert!(production.grants("prod-token"));
        assert!(
            !production.grants("staging-token"),
            "a staging token never opens production"
        );
        assert!(
            !production.grants(&hash_token("prod-token")),
            "the hash is not the token"
        );
        assert!(!production.grants(""));
        assert!(!production.grants("other"));
    }

    #[test]
    fn hashes_compare_in_any_case_and_need_a_known_algorithm() {
        let upper = config(json!({
            "accessTokenHashes": [
                { "environment": "staging", "algorithm": "sha256",
                  "hash": hash_token("tok").to_uppercase() },
                { "environment": "staging", "algorithm": "md5", "hash": hash_token("other") }
            ]
        }));
        assert!(
            upper.grants("tok"),
            "an artifact without an environment takes any entry"
        );
        assert!(!upper.grants("other"), "an unknown algorithm never matches");
    }

    #[test]
    fn an_entry_without_environment_does_not_open_an_environment() {
        let production = config(json!({
            "environment": { "key": "production" },
            "accessTokenHashes": [{ "algorithm": "sha256", "hash": hash_token("tok") }]
        }));
        assert!(!production.grants("tok"));
    }

    #[test]
    fn plain_tokens_of_older_artifacts_keep_working() {
        let old = config(json!({ "version": "1", "accessTokens": ["secret-token"] }));
        assert!(old.grants("secret-token"));
        assert!(!old.grants("secret"));
    }

    #[test]
    fn malformed_entries_are_skipped_never_the_whole_config() {
        let data = config(json!({
            "accessTokenHashes": [{ "hash": 5 }, "nope", entry("staging", "tok")],
            "accessTokens": []
        }));
        assert_eq!(data.access_token_hashes.len(), 1);
        assert!(data.grants("tok"));
        let not_a_list = config(json!({ "accessTokenHashes": "nope", "accessTokens": ["a"] }));
        assert!(not_a_list.access_token_hashes.is_empty());
        assert!(not_a_list.grants("a"));
    }
}
