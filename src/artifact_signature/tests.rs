#![allow(clippy::unwrap_used)] // tests fail loudly on purpose

use super::*;
use ed25519_dalek::{Signer as _, SigningKey};

const CURRENT: [u8; 32] = [7; 32];
const PREVIOUS: [u8; 32] = [9; 32];
const FOREIGN: [u8; 32] = [3; 32];

fn key(seed: [u8; 32]) -> SigningKey {
    SigningKey::from_bytes(&seed)
}

fn public(seed: [u8; 32]) -> String {
    B64.encode(key(seed).verifying_key().to_bytes())
}

fn keys() -> ArtifactKeys {
    ArtifactKeys::parse(
        "ARTIFACT__PUBLIC_KEY",
        Some(&format!("2026-01={}", public(CURRENT))),
        Some(&format!("2025-06={}", public(PREVIOUS))),
    )
    .unwrap()
}

fn entries() -> Vec<(String, Vec<u8>)> {
    vec![
        (
            ".config/project.json".to_owned(),
            b"{\"version\":\"2\"}".to_vec(),
        ),
        (
            "credit-application".to_owned(),
            b"{\"contentType\":\"graph\"}".to_vec(),
        ),
    ]
}

/// A signature entry over `entries`, signed by `seed` under `key_id`.
fn signed(seed: [u8; 32], key_id: &str, entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let digest = digest(entries);
    let signing = key(seed);
    let signature = signing.sign(&hex_bytes(&digest).unwrap());
    serde_json::to_vec(&serde_json::json!({
        "format": FORMAT,
        "keyId": key_id,
        "digest": digest,
        "signature": B64.encode(signature.to_bytes()),
    }))
    .unwrap()
}

#[test]
fn a_valid_artifact_verifies_under_either_key() {
    let entries = entries();
    for (seed, id) in [(CURRENT, "2026-01"), (PREVIOUS, "2025-06")] {
        assert_eq!(
            verify(&keys(), &entries, Some(&signed(seed, id, &entries))),
            Verdict::Verified,
            "{id}"
        );
    }
}

#[test]
fn an_unsigned_artifact_is_refused_when_keys_are_configured() {
    assert_eq!(
        verify(&keys(), &entries(), None),
        Verdict::Refused("the artifact is not signed".to_owned())
    );
}

#[test]
fn an_altered_artifact_is_refused() {
    let mut altered = entries();
    let signature = signed(CURRENT, "2026-01", &altered);
    altered[1].1 = b"{\"contentType\":\"graph\",\"tampered\":true}".to_vec();
    match verify(&keys(), &altered, Some(&signature)) {
        Verdict::Refused(reason) => assert!(reason.contains("digest"), "{reason}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_foreign_key_is_refused() {
    let entries = entries();
    let signature = signed(FOREIGN, "2024-01", &entries);
    match verify(&keys(), &entries, Some(&signature)) {
        Verdict::Refused(reason) => assert!(reason.contains("does not trust"), "{reason}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn without_keys_nothing_is_refused_and_nothing_is_checked() {
    let keys = ArtifactKeys::default();
    assert_eq!(verify(&keys, &entries(), None), Verdict::NotEnforced);
}

#[test]
fn a_bad_key_value_names_its_variable() {
    let err = ArtifactKeys::parse("ARTIFACT__PUBLIC_KEY", Some("no-separator"), None).unwrap_err();
    assert!(err.contains("ARTIFACT__PUBLIC_KEY"), "{err}");
}
