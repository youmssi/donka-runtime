//! Verification of signed release artifacts (DNK-87).
//!
//! Studio signs each artifact it publishes with the installation's Ed25519
//! key. The signature travels inside the zip as `.config/signature.json`:
//! `{ "format": "donka-artifact/1", "keyId": "…", "digest": "…", "signature": "…" }`
//! where `digest` is the SHA-256 (hex) of the artifact's canonical content and
//! `signature` is Ed25519 over those digest bytes (URL-safe base64, no pad).
//!
//! The canonical content is, for every entry except `.config/signature.json`,
//! sorted by entry name: the name's bytes, one zero byte, the entry's bytes,
//! one zero byte. A Runtime given the installation's public keys refuses an
//! artifact that is unsigned, altered, or signed by another key, and keeps
//! serving the last valid one. Without keys it behaves as before and says so
//! at start, so existing installations upgrade in order.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use ed25519_dalek::{Signature, Verifier as _, VerifyingKey};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// The signature entry inside the artifact zip.
pub const SIGNATURE_ENTRY: &str = ".config/signature.json";

/// The only signature format this version reads.
pub const FORMAT: &str = "donka-artifact/1";

/// The installation's public keys: the current one, and the previous one while
/// a rotation completes. Either may verify an artifact.
#[derive(Debug, Clone, Default)]
pub struct ArtifactKeys {
    pub current: Option<(String, VerifyingKey)>,
    pub previous: Option<(String, VerifyingKey)>,
}

impl ArtifactKeys {
    /// Parses `keyId=base64url(public key)` values (32 raw bytes). Startup
    /// fails fast on a value that is not a key, naming the variable.
    pub fn parse(
        var: &'static str,
        current: Option<&str>,
        previous: Option<&str>,
    ) -> Result<Self, String> {
        let parse_one = |value: Option<&str>| -> Result<Option<(String, VerifyingKey)>, String> {
            let Some(value) = value.filter(|v| !v.trim().is_empty()) else {
                return Ok(None);
            };
            let Some((id, encoded)) = value.split_once('=') else {
                return Err(format!(
                    "{var} must name its key, e.g. {var}={}={}",
                    "2026-01", "<base64 public key>"
                ));
            };
            let bytes = B64
                .decode(encoded.trim())
                .map_err(|_| format!("{var}: the public key is not base64"))?;
            let key = VerifyingKey::from_bytes(
                bytes
                    .as_slice()
                    .try_into()
                    .map_err(|_| format!("{var}: the public key is not 32 bytes"))?,
            )
            .map_err(|_| format!("{var}: the public key is not an Ed25519 key"))?;
            Ok(Some((id.trim().to_owned(), key)))
        };
        Ok(Self {
            current: parse_one(current)?,
            previous: parse_one(previous)?,
        })
    }

    /// Whether any key is configured: verification is enforced then.
    pub fn enforced(&self) -> bool {
        self.current.is_some() || self.previous.is_some()
    }

    fn key_for(&self, key_id: &str) -> Option<&VerifyingKey> {
        [&self.current, &self.previous]
            .into_iter()
            .flatten()
            .find(|(id, _)| id == key_id)
            .map(|(_, key)| key)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SignatureFile {
    format: String,
    key_id: String,
    digest: String,
    signature: String,
}

/// The outcome of checking one artifact.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// No keys configured: the artifact is served as before (DNK-87).
    NotEnforced,
    /// The signature verifies under the current or previous key.
    Verified,
    /// Enforced and refused; the reason is logged, never served.
    Refused(String),
}

/// SHA-256 (hex) of the canonical content of `entries` (name, bytes), the
/// signature entry excluded.
pub fn digest(entries: &[(String, Vec<u8>)]) -> String {
    let mut sorted: Vec<&(String, Vec<u8>)> = entries
        .iter()
        .filter(|(name, _)| name != SIGNATURE_ENTRY)
        .collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hasher = Sha256::new();
    for (name, body) in sorted {
        hasher.update(name.as_bytes());
        hasher.update([0]);
        hasher.update(body);
        hasher.update([0]);
    }
    hex(&hasher.finalize())
}

/// Checks `signature_entry` (the bytes of `.config/signature.json`, when the
/// artifact carries one) against `entries` under `keys`.
pub fn verify(
    keys: &ArtifactKeys,
    entries: &[(String, Vec<u8>)],
    signature_entry: Option<&[u8]>,
) -> Verdict {
    if !keys.enforced() {
        return Verdict::NotEnforced;
    }
    let Some(bytes) = signature_entry else {
        return Verdict::Refused("the artifact is not signed".to_owned());
    };
    let file: SignatureFile = match serde_json::from_slice(bytes) {
        Ok(file) => file,
        Err(err) => return Verdict::Refused(format!("its signature file cannot be read: {err}")),
    };
    if file.format != FORMAT {
        return Verdict::Refused(format!("its signature format is not {FORMAT}"));
    }
    let Some(key) = keys.key_for(&file.key_id) else {
        return Verdict::Refused(format!(
            "it is signed by key `{}`, which this installation does not trust",
            file.key_id
        ));
    };
    let expected = digest(entries);
    if file.digest != expected {
        return Verdict::Refused(
            "its content does not match the digest its signature covers".to_owned(),
        );
    }
    let Ok(digest_bytes) = hex_bytes(&expected) else {
        return Verdict::Refused("its digest is not hex".to_owned());
    };
    let Ok(signature_bytes) = B64.decode(&file.signature) else {
        return Verdict::Refused("its signature is not base64".to_owned());
    };
    let Ok(signature) = Signature::from_slice(&signature_bytes) else {
        return Verdict::Refused("its signature is not an Ed25519 signature".to_owned());
    };
    match key.verify(&digest_bytes, &signature) {
        Ok(()) => Verdict::Verified,
        Err(_) => Verdict::Refused("its signature does not verify".to_owned()),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_bytes(text: &str) -> Result<Vec<u8>, ()> {
    if text.len() % 2 != 0 {
        return Err(());
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).map_err(|_| ()))
        .collect()
}

#[cfg(test)]
#[path = "artifact_signature/tests.rs"]
mod tests;
