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
