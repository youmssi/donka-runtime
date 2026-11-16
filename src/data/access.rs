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
#[path = "access/tests.rs"]
mod tests;
