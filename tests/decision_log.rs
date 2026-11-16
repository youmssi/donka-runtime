//! Evaluations reach a fake Studio decision log. The feed is a process-wide
//! singleton, so these tests have their own binary.

use agent::app;
use agent::config::{
    DecisionLogConfig, EnvironmentConfig, FilesystemProviderConfig, ProviderConfig,
};
use axum::body::{Body, to_bytes};
use axum::extract::State;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tower::ServiceExt;

/// `dnk_test_token`, as docs/artifact-format.md in Studio gives it.
const TOKEN: &str = "dnk_test_token";
const TOKEN_HASH: &str = "d4d813b79f07c455e68458c955824329d902dd5f8e7b0c250fb3f05b3f68c840";
const LOG_TOKEN: &str = "dnk_log_secret";

type Received = Arc<Mutex<Vec<Value>>>;

/// Keeps every record it is sent, when the token is right.
async fn studio() -> (String, Received) {
    let received = Received::default();
    let app =
        Router::new()
            .route(
                "/api/v1/decision-log/records",
                post(
                    |State(received): State<Received>,
                     headers: HeaderMap,
                     Json(body): Json<Value>| async move {
                        let expected = format!("Bearer {LOG_TOKEN}");
                        if headers.get("authorization").and_then(|v| v.to_str().ok())
                            != Some(expected.as_str())
                        {
                            return StatusCode::UNAUTHORIZED;
                        }
                        let mut received = received.lock().unwrap();
                        received.extend(body["records"].as_array().unwrap().iter().cloned());
                        StatusCode::ACCEPTED
                    },
                ),
            )
            .with_state(received.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/api/v1/decision-log/records",
        listener.local_addr().unwrap()
    );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, received)
}

fn graph() -> Value {
    graph_with("amount < 1000")
}

fn graph_with(expression: &str) -> Value {
    json!({
        "contentType": "graph",
        "nodes": [
            { "id": "in", "name": "Request", "type": "inputNode", "position": { "x": 0, "y": 0 } },
            {
                "id": "limit", "name": "Limit", "type": "expressionNode", "position": { "x": 0, "y": 0 },
                "content": { "expressions": [{ "id": "e1", "key": "approved", "value": expression }] }
            },
            { "id": "out", "name": "Response", "type": "outputNode", "position": { "x": 0, "y": 0 } }
        ],
        "edges": [
            { "id": "e1", "type": "edge", "sourceId": "in", "targetId": "limit" },
            { "id": "e2", "type": "edge", "sourceId": "limit", "targetId": "out" }
        ]
    })
}

fn write(dir: &Path, name: &str, content: &Value) {
    std::fs::create_dir_all(dir.join(name).parent().unwrap()).unwrap();
    std::fs::write(dir.join(name), content.to_string()).unwrap();
}

/// `credit`, released by Studio to production; `legacy`, an artifact that
/// does not name its release or environment.
fn projects() -> PathBuf {
    let root = std::env::temp_dir().join(format!("donka-decision-log-{}", std::process::id()));
    let credit = root.join("credit");
    write(&credit, "limit.json", &graph());
    write(&credit, "broken.json", &graph_with("amount +"));
    write(
        &credit,
        ".config/project.json",
        &json!({
            "version": "2",
            "project": { "id": "p-credit", "key": "credit", "name": "Credit" },
            "release": { "id": "r-7", "version": "1.2.0", "name": "1.2.0", "status": "published" },
            "environment": { "id": "e-prod", "key": "production", "name": "Production" },
            "accessTokenHashes": [
                { "id": "t-1", "environment": "production", "algorithm": "sha256", "hash": TOKEN_HASH }
            ]
        }),
    );
    write(&root.join("legacy"), "limit.json", &graph());
    root
}

async fn router(root: &Path, url: String) -> Router {
    let config = EnvironmentConfig {
        provider: ProviderConfig::Filesystem(FilesystemProviderConfig {
            root_dir: root.to_string_lossy().into_owned(),
        }),
        decision_log: DecisionLogConfig {
            url: Some(url),
            token: Some(LOG_TOKEN.into()),
            flush_interval: Duration::from_millis(20),
            ..Default::default()
        },
        ..Default::default()
    };
    let agent = app::create_agent(config.clone(), Default::default()).await;
    app::create_app(agent, config).await
}

struct Answer {
    status: StatusCode,
    decision_id: Option<String>,
    body: Value,
}

async fn evaluate(router: &Router, uri: &str, reference: Option<&str>, body: Value) -> Answer {
    let mut request = Request::post(uri)
        .header("Content-Type", "application/json")
        .header("X-Access-Token", TOKEN);
    if let Some(reference) = reference {
        request = request.header("X-Donka-Reference", reference);
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let decision_id = response
        .headers()
        .get("x-decision-id")
        .map(|v| v.to_str().unwrap().to_owned());
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    Answer {
        status,
        decision_id,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    }
}

/// The record with this id, once it has arrived.
async fn record(received: &Received, id: &str) -> Value {
    for _ in 0..500 {
        if let Some(record) = received
            .lock()
            .unwrap()
            .iter()
            .find(|r| r["id"] == json!(id))
        {
            return record.clone();
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("record {id} never reached Studio");
}

#[tokio::test]
async fn every_evaluation_reaches_the_decision_log() {
    let (url, received) = studio().await;
    let root = projects();
    let router = router(&root, url).await;

    // A decision, with the caller's reference; the caller did not ask for the trace.
    let answer = evaluate(
        &router,
        "/api/projects/credit/evaluate/limit.json",
        Some("APP-2026-0042"),
        json!({ "context": { "amount": 250 } }),
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    assert_eq!(answer.body["result"]["approved"], json!(true));
    assert!(answer.body.get("trace").is_none(), "{}", answer.body);
    let id = answer.decision_id.expect("X-Decision-Id");
    let logged = record(&received, &id).await;
    assert_eq!(logged["projectId"], json!("p-credit"));
    assert_eq!(logged["releaseId"], json!("r-7"));
    assert_eq!(logged["environment"], json!("production"));
    assert_eq!(logged["decisionKey"], json!("limit.json"));
    assert_eq!(logged["reference"], json!("APP-2026-0042"));
    assert_eq!(logged["status"], json!("succeeded"));
    assert_eq!(logged["input"], json!({ "amount": 250 }));
    assert_eq!(logged["output"]["approved"], json!(true));
    assert_eq!(logged["trace"]["limit"]["output"]["approved"], json!(true));
    assert!(logged["evaluatedAt"].is_string());
    assert!(logged["durationUs"].is_u64());
    assert!(
        !logged.to_string().contains(TOKEN),
        "no access token in the record"
    );

    // The /rules surface keeps the trace when asked for it.
    let answer = evaluate(
        &router,
        "/api/rules/credit/evaluate/limit.json",
        None,
        json!({ "context": { "amount": 5000 }, "trace": true }),
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    assert!(answer.body.get("trace").is_some());
    let logged = record(&received, &answer.decision_id.expect("X-Decision-Id")).await;
    assert_eq!(logged["output"]["approved"], json!(false));
    assert!(logged.get("reference").is_none());

    // A failed evaluation is logged with the error the caller received.
    let answer = evaluate(
        &router,
        "/api/rules/credit/evaluate/missing.json",
        Some("APP-2026-0043"),
        json!({ "context": {} }),
    )
    .await;
    assert_eq!(answer.status, StatusCode::BAD_REQUEST);
    let logged = record(&received, &answer.decision_id.expect("X-Decision-Id")).await;
    assert_eq!(logged["status"], json!("failed"));
    assert_eq!(logged["error"], answer.body);
    assert!(logged.get("output").is_none());

    // A node error is logged with the trace up to the failing node; error
    // answers never carry it, as upstream.
    for uri in [
        "/api/projects/credit/evaluate/broken.json",
        "/api/rules/credit/evaluate/broken.json",
    ] {
        let answer = evaluate(
            &router,
            uri,
            None,
            json!({ "context": { "amount": 1 }, "trace": true }),
        )
        .await;
        assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{uri}");
        let logged = record(&received, &answer.decision_id.expect("X-Decision-Id")).await;
        assert_eq!(logged["status"], json!("failed"));
        assert_eq!(logged["error"], answer.body, "{uri}");
        assert_eq!(
            logged["trace"]["in"]["output"],
            json!({ "amount": 1 }),
            "{uri}: {logged}"
        );
    }

    // A reference that is not 1 to 200 visible characters is refused on both surfaces.
    let long = "x".repeat(201);
    for uri in [
        "/api/projects/credit/evaluate/limit.json",
        "/api/rules/credit/evaluate/limit.json",
    ] {
        let answer = evaluate(&router, uri, Some(&long), json!({ "context": {} })).await;
        assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{uri}");
        assert!(answer.decision_id.is_none());
    }

    // An artifact that does not name its release and environment is answered, not logged.
    let answer = evaluate(
        &router,
        "/api/projects/legacy/evaluate/limit.json",
        None,
        json!({ "context": { "amount": 1 } }),
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    assert!(answer.decision_id.is_none());

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(received.lock().unwrap().len(), 5);
    std::fs::remove_dir_all(root).ok();
}
