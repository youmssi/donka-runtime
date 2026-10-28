//! An expression that reads `$`, the object its own node is building (DNK-62). Before
//! zen-engine 2.1.2 the node stored that object inside itself and serialising the answer
//! overflowed the stack, which aborts the whole process. The Runtime must answer such a request
//! and keep serving the next ones.

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

fn project() -> PathBuf {
    let root = std::env::temp_dir().join(format!("donka-self-reference-{}", std::process::id()));
    let credit = root.join("credit");
    write(
        &credit,
        "score.json",
        &json!({
            "contentType": "graph",
            "nodes": [
                { "id": "in", "name": "Request", "type": "inputNode", "position": { "x": 0, "y": 0 } },
                {
                    "id": "score", "name": "Score", "type": "expressionNode", "position": { "x": 0, "y": 0 },
                    "content": { "expressions": [
                        { "id": "e1", "key": "points", "value": "income / 1000" },
                        { "id": "e2", "key": "snapshot", "value": "$" }
                    ]}
                },
                { "id": "out", "name": "Response", "type": "outputNode", "position": { "x": 0, "y": 0 } }
            ],
            "edges": [
                { "id": "e1", "type": "edge", "sourceId": "in", "targetId": "score" },
                { "id": "e2", "type": "edge", "sourceId": "score", "targetId": "out" }
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
    root
}

async fn evaluate(router: &Router, context: Value) -> (StatusCode, Value) {
    let request = Request::post("/api/projects/credit/evaluate/score.json")
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
async fn an_expression_reading_its_own_node_is_answered_and_the_runtime_keeps_serving() {
    let config = EnvironmentConfig {
        provider: ProviderConfig::Filesystem(FilesystemProviderConfig {
            root_dir: project().to_string_lossy().into_owned(),
        }),
        ..Default::default()
    };
    let agent = app::create_agent(config.clone(), Default::default()).await;
    let router = app::create_app(agent, config).await;

    for income in [250_000, 400_000] {
        let (status, body) = evaluate(&router, json!({ "income": income })).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        // `$` is the node's output as it stood when the expression ran: a copy, not a cycle.
        let points = income / 1000;
        assert_eq!(
            body["result"],
            json!({ "points": points, "snapshot": { "points": points } }),
            "{body}"
        );
    }
}
