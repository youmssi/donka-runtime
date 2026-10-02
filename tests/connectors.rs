//! Connector nodes evaluated by the Runtime against a fake bureau. The connector
//! handler is a process-wide singleton, so these tests have their own binary.

use agent::app;
use agent::config::{
    ConnectorsConfig, EnvironmentConfig, FilesystemProviderConfig, ProviderConfig,
};
use axum::body::{Body, to_bytes};
use axum::http::{HeaderMap, Request, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;
use tower::ServiceExt;

const SECRET: &str = "bureau-secret-value";

/// Answers with a score when the API key is right, 401 otherwise; `/down` always fails.
async fn bureau() -> String {
    let app = Router::new()
        .route(
            "/score",
            post(|headers: HeaderMap, Json(body): Json<Value>| async move {
                if headers.get("x-api-key").and_then(|v| v.to_str().ok()) != Some(SECRET) {
                    return (StatusCode::UNAUTHORIZED, Json(json!({})));
                }
                let score = if body["id"] == json!("A1") { 712 } else { 480 };
                (StatusCode::OK, Json(json!({ "score": score })))
            }),
        )
        .route("/down", post(|| async { StatusCode::BAD_GATEWAY }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

fn graph(custom: Value) -> Value {
    json!({
        "contentType": "graph",
        "nodes": [
            { "id": "in", "name": "Request", "type": "inputNode", "position": { "x": 0, "y": 0 } },
            {
                "id": "custom", "name": "Bureau", "type": "customNode", "position": { "x": 0, "y": 0 },
                "content": custom
            },
            { "id": "out", "name": "Response", "type": "outputNode", "position": { "x": 0, "y": 0 } }
        ],
        "edges": [
            { "id": "e1", "type": "edge", "sourceId": "in", "targetId": "custom" },
            { "id": "e2", "type": "edge", "sourceId": "custom", "targetId": "out" }
        ]
    })
}

fn connector(url: String, extra: Value) -> Value {
    let mut config = json!({
        "preset": "bureau-score",
        "url": url,
        "auth": { "type": "header", "header": "X-Api-Key", "secret": "BUREAU_KEY" },
        "body": { "id": "{{ applicant.id }}" },
        "outputKey": "bureau",
        "retries": 0,
        "mock": { "score": 1 }
    });
    for (key, value) in extra.as_object().unwrap() {
        config[key] = value.clone();
    }
    json!({ "kind": "donka.connector", "config": config })
}

/// A project directory holding the given decisions.
fn project(files: &[(&str, Value)]) -> PathBuf {
    let root = std::env::temp_dir().join(format!("donka-connectors-{}", std::process::id()));
    let dir = root.join("bureau");
    std::fs::create_dir_all(&dir).unwrap();
    for (name, content) in files {
        std::fs::write(dir.join(name), content.to_string()).unwrap();
    }
    root
}

async fn router(root: PathBuf) -> Router {
    let config = EnvironmentConfig {
        provider: ProviderConfig::Filesystem(FilesystemProviderConfig {
            root_dir: root.to_string_lossy().into_owned(),
        }),
        connectors: ConnectorsConfig {
            timeout: Duration::from_millis(500),
            ..Default::default()
        },
        ..Default::default()
    };
    let agent = app::create_agent(config.clone(), Default::default()).await;
    app::create_app(agent, config).await
}

async fn evaluate(router: &Router, uri: &str, context: Value) -> (StatusCode, Value) {
    let request = Request::post(uri)
        .header("Content-Type", "application/json")
        .body(Body::from(
            json!({ "context": context, "trace": true }).to_string(),
        ))
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn connector_nodes_call_the_service_with_the_secret_from_the_environment() {
    // SAFETY: set before the handler reads it; no other test in this binary touches it.
    unsafe { std::env::set_var("DONKA_SECRET_BUREAU_KEY", SECRET) };
    let base = bureau().await;
    let root = project(&[
        (
            "score.json",
            graph(connector(format!("{base}/score"), json!({}))),
        ),
        (
            "fallback.json",
            graph(connector(
                format!("{base}/down"),
                json!({ "onError": "fallback", "fallback": { "score": null } }),
            )),
        ),
        (
            "failing.json",
            graph(connector(format!("{base}/down"), json!({}))),
        ),
        (
            "other.json",
            graph(json!({ "kind": "acme.other", "config": {} })),
        ),
    ]);
    let router = router(root.clone()).await;
    let applicant = json!({ "applicant": { "id": "A1" } });

    let (status, body) = evaluate(
        &router,
        "/api/projects/bureau/evaluate/score.json",
        applicant.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let result = &body["result"];
    assert_eq!(result["bureau"], json!({ "score": 712 }));
    assert_eq!(result["applicant"], json!({ "id": "A1" }));
    let trace = &body["trace"]["custom"]["traceData"];
    assert_eq!(trace["mode"], json!("live"));
    assert_eq!(trace["outcome"], json!("ok"));
    assert!(
        !body.to_string().contains(SECRET),
        "the secret stays out of the response"
    );

    let (status, body) = evaluate(
        &router,
        "/api/projects/bureau/evaluate/fallback.json",
        applicant.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["result"]["bureau"], json!({ "score": null }));
    assert_eq!(
        body["trace"]["custom"]["traceData"]["outcome"],
        json!("fallback")
    );

    let (status, body) = evaluate(
        &router,
        "/api/projects/bureau/evaluate/failing.json",
        applicant.clone(),
    )
    .await;
    assert_ne!(status, StatusCode::OK);
    assert!(
        body.to_string().contains("the service answered 502"),
        "{body}"
    );
    assert!(!body.to_string().contains(SECRET));

    // Custom nodes other than connectors keep upstream's answer.
    let (status, body) =
        evaluate(&router, "/api/rules/bureau/evaluate/other.json", applicant).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], json!("evaluate.integrationsNotSupported"));

    std::fs::remove_dir_all(root).ok();
}
