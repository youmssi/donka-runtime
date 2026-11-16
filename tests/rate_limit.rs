//! Per-token request limits (DNK-22) through the real router.

use agent::app;
use agent::config::{EnvironmentConfig, FilesystemProviderConfig, ProviderConfig, RateLimitConfig};
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tower::ServiceExt;

/// `dnk_test_token`, as docs/artifact-format.md in Studio gives it.
const TOKEN: &str = "dnk_test_token";
const TOKEN_HASH: &str = "d4d813b79f07c455e68458c955824329d902dd5f8e7b0c250fb3f05b3f68c840";
const OTHER: &str = "dnk_other_token";
const OTHER_HASH: &str = "fbeef5817bc570c1cda81d984e5c95464c945e26fe792451d50c63543197f963";

fn write(dir: &Path, name: &str, content: &Value) {
    std::fs::create_dir_all(dir.join(name).parent().unwrap()).unwrap();
    std::fs::write(dir.join(name), content.to_string()).unwrap();
}

fn project(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("donka-rate-limit-{}-{name}", std::process::id()));
    let credit = root.join("credit");
    write(
        &credit,
        "limit.json",
        &json!({
            "contentType": "graph",
            "nodes": [
                { "id": "in", "name": "Request", "type": "inputNode", "position": { "x": 0, "y": 0 } },
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
    write(
        &credit,
        ".config/project.json",
        &json!({
            "version": "2",
            "project": { "id": "p-credit", "key": "credit", "name": "Credit" },
            "accessTokenHashes": [
                { "id": "t-1", "algorithm": "sha256", "hash": TOKEN_HASH },
                { "id": "t-2", "algorithm": "sha256", "hash": OTHER_HASH }
            ]
        }),
    );
    root
}

async fn router(name: &str, requests: Option<u32>) -> Router {
    let config = EnvironmentConfig {
        provider: ProviderConfig::Filesystem(FilesystemProviderConfig {
            root_dir: project(name).to_string_lossy().into_owned(),
        }),
        rate_limit: RateLimitConfig {
            requests,
            window: Duration::from_secs(60),
        },
        ..Default::default()
    };
    let agent = app::create_agent(config.clone(), Default::default()).await;
    app::create_app(agent, config).await
}

struct Answer {
    status: StatusCode,
    retry_after: Option<u64>,
    body: Value,
}

async fn call(router: &Router, method: &str, uri: &str, token: Option<&str>) -> Answer {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("Content-Type", "application/json");
    if let Some(token) = token {
        request = request.header("X-Access-Token", token);
    }
    let body = if method == "POST" {
        Body::from(json!({ "context": { "amount": 250 } }).to_string())
    } else {
        Body::empty()
    };
    let response = router
        .clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let retry_after = response
        .headers()
        .get(header::RETRY_AFTER)
        .map(|value| value.to_str().unwrap().parse().unwrap());
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    Answer {
        status,
        retry_after,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    }
}

const EVALUATE: &str = "/api/projects/credit/evaluate/limit.json";
const RULES: &str = "/api/rules/credit/evaluate/limit.json";

#[tokio::test]
async fn a_token_past_its_limit_gets_429_with_retry_after() {
    let router = router("limited", Some(2)).await;

    assert_eq!(
        call(&router, "POST", EVALUATE, Some(TOKEN)).await.status,
        StatusCode::OK
    );
    assert_eq!(
        call(&router, "POST", RULES, Some(TOKEN)).await.status,
        StatusCode::OK
    );

    let refused = call(&router, "POST", EVALUATE, Some(TOKEN)).await;
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused.retry_after, Some(30));
    assert!(
        refused.body["message"]
            .as_str()
            .unwrap()
            .contains("retry in 30 s")
    );

    // The /rules surface answers in its own error shape.
    let refused = call(&router, "POST", RULES, Some(TOKEN)).await;
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        refused.body,
        json!({ "code": "rateLimit.exceeded", "retryAfter": 30 })
    );
    // Reading the project's rules counts too.
    assert_eq!(
        call(&router, "GET", "/api/rules/credit", Some(TOKEN))
            .await
            .status,
        StatusCode::TOO_MANY_REQUESTS
    );

    // Another token still has its whole allowance.
    assert_eq!(
        call(&router, "POST", EVALUATE, Some(OTHER)).await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn health_is_never_limited() {
    let router = router("health", Some(1)).await;
    call(&router, "POST", EVALUATE, Some(TOKEN)).await;
    assert_eq!(
        call(&router, "POST", EVALUATE, Some(TOKEN)).await.status,
        StatusCode::TOO_MANY_REQUESTS
    );
    for _ in 0..5 {
        assert_eq!(
            call(&router, "GET", "/api/health", Some(TOKEN))
                .await
                .status,
            StatusCode::OK
        );
        assert_eq!(
            call(&router, "GET", "/api/version", Some(TOKEN))
                .await
                .status,
            StatusCode::OK
        );
    }
}

#[tokio::test]
async fn a_wrong_token_gets_401_and_uses_no_allowance() {
    let router = router("wrong", Some(1)).await;
    for _ in 0..3 {
        assert_eq!(
            call(&router, "POST", EVALUATE, Some("dnk_wrong"))
                .await
                .status,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call(&router, "POST", EVALUATE, None).await.status,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        call(&router, "POST", EVALUATE, Some(TOKEN)).await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn no_limit_unless_configured() {
    let router = router("unlimited", None).await;
    for _ in 0..20 {
        assert_eq!(
            call(&router, "POST", EVALUATE, Some(TOKEN)).await.status,
            StatusCode::OK
        );
    }
}
