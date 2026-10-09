//! Input contracts (DNK-37): a request that breaks the decision's input schema is refused with
//! `400`, naming the field, through the real router.

use agent::app;
use agent::config::{EnvironmentConfig, FilesystemProviderConfig, ProviderConfig};
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tower::ServiceExt;

/// `dnk_test_token`, as docs/artifact-format.md in Studio gives it.
const TOKEN: &str = "dnk_test_token";
const TOKEN_HASH: &str = "d4d813b79f07c455e68458c955824329d902dd5f8e7b0c250fb3f05b3f68c840";

fn write(dir: &Path, name: &str, content: &Value) {
    std::fs::create_dir_all(dir.join(name).parent().unwrap()).unwrap();
    std::fs::write(dir.join(name), content.to_string()).unwrap();
}

fn contract() -> Value {
    json!({
        "type": "object",
        "required": ["applicant", "amount"],
        "properties": {
            "applicant": {
                "type": "object",
                "required": ["age"],
                "properties": { "age": { "type": "integer", "minimum": 18 } }
            },
            "amount": { "type": "number", "x-donka": { "label": { "en": "Amount", "fr": "Montant" } } }
        }
    })
}

fn project(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("donka-contract-{}-{name}", std::process::id()));
    let credit = root.join("credit");
    write(
        &credit,
        "limit.json",
        &json!({
            "contentType": "graph",
            "nodes": [
                {
                    "id": "in", "name": "Request", "type": "inputNode", "position": { "x": 0, "y": 0 },
                    // Studio's editor stores the schema as a string.
                    "content": { "schema": contract().to_string() }
                },
                {
                    "id": "limit", "name": "Limit", "type": "expressionNode", "position": { "x": 0, "y": 0 },
                    "content": { "expressions": [{ "id": "e1", "key": "approved", "value": "amount < 1000" }] }
                },
                { "id": "out", "name": "Response", "type": "outputNode", "position": { "x": 0, "y": 0 } }
            ],
            "edges": [
                { "id": "e1", "type": "edge", "sourceId": "in", "targetId": "limit" },
                { "id": "e2", "type": "edge", "sourceId": "limit", "targetId": "out" }
            ]
        }),
    );
    // A decision without a contract that fails for another reason: its sub-decision is missing.
    write(
        &credit,
        "broken.json",
        &json!({
            "contentType": "graph",
            "nodes": [
                { "id": "in", "name": "Request", "type": "inputNode", "position": { "x": 0, "y": 0 } },
                {
                    "id": "call", "name": "Missing", "type": "decisionNode", "position": { "x": 0, "y": 0 },
                    "content": { "key": "does-not-exist" }
                },
                { "id": "out", "name": "Response", "type": "outputNode", "position": { "x": 0, "y": 0 } }
            ],
            "edges": [
                { "id": "e1", "type": "edge", "sourceId": "in", "targetId": "call" },
                { "id": "e2", "type": "edge", "sourceId": "call", "targetId": "out" }
            ]
        }),
    );
    write(
        &credit,
        ".config/project.json",
        &json!({
            "version": "2",
            "project": { "id": "p-credit", "key": "credit", "name": "Credit" },
            "accessTokenHashes": [{ "id": "t-1", "algorithm": "sha256", "hash": TOKEN_HASH }]
        }),
    );
    // What Studio writes beside the decisions: never loaded as a decision.
    write(
        &credit,
        ".config/contracts/limit/input.schema.json",
        &contract(),
    );
    root
}

async fn router(name: &str) -> Router {
    let config = EnvironmentConfig {
        provider: ProviderConfig::Filesystem(FilesystemProviderConfig {
            root_dir: project(name).to_string_lossy().into_owned(),
        }),
        ..Default::default()
    };
    let agent = app::create_agent(config.clone(), Default::default()).await;
    app::create_app(agent, config).await
}

async fn evaluate(router: &Router, context: Value) -> (StatusCode, Value) {
    evaluate_key(router, "limit.json", context).await
}

async fn evaluate_key(router: &Router, key: &str, context: Value) -> (StatusCode, Value) {
    let request = Request::post(format!("/api/projects/credit/evaluate/{key}"))
        .header("Content-Type", "application/json")
        .header("X-Access-Token", TOKEN)
        .body(Body::from(json!({ "context": context }).to_string()))
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn a_request_that_breaks_the_contract_is_refused_naming_the_field() {
    let router = router("refused").await;
    for (context, field, message) in [
        (
            json!({ "applicant": { "age": 12 }, "amount": 500 }),
            "applicant.age",
            "12 is less than the minimum of 18",
        ),
        (
            json!({ "applicant": { "age": 30 } }),
            "amount",
            r#""amount" is a required property"#,
        ),
        (
            json!({ "applicant": {}, "amount": 500 }),
            "applicant.age",
            r#""age" is a required property"#,
        ),
        (
            json!({ "applicant": { "age": 30 }, "amount": "five" }),
            "amount",
            r#""five" is not of type "number""#,
        ),
    ] {
        let (status, body) = evaluate(&router, context.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{context}: {body}");
        assert_eq!(body["contract"]["field"], field, "{context}: {body}");
        assert_eq!(body["contract"]["message"], message, "{context}: {body}");
        assert!(body["message"].is_string(), "{body}");
    }
}

#[tokio::test]
async fn a_request_within_the_contract_is_answered() {
    let router = router("answered").await;
    let (status, body) = evaluate(
        &router,
        json!({ "applicant": { "age": 30 }, "amount": 500 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["result"]["approved"], true, "{body}");
}

#[tokio::test]
async fn other_failures_name_no_field() {
    let router = router("other").await;
    let (status, body) = evaluate_key(&router, "broken.json", json!({ "anything": 1 })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.get("contract").is_none(), "{body}");
}
