//! Function nodes have no network access (DNK-83). The engine's setting is process-wide, so
//! this test has its own binary.

use agent::app;
use agent::config::{EnvironmentConfig, FilesystemProviderConfig, ProviderConfig};
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tower::ServiceExt;

#[tokio::test]
async fn a_function_node_cannot_call_an_address() {
    // A local service that counts the connections it receives and answers like an internal API.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/internal", listener.local_addr().unwrap());
    let hits = Arc::new(AtomicUsize::new(0));
    let counted = hits.clone();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            counted.fetch_add(1, Ordering::SeqCst);
            let _ = stream.read(&mut [0; 1024]);
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
            );
        }
    });
    let source = format!(
        "import http from 'http';\nexport const handler = async () => {{\n  const response = await http.get('{url}');\n  return {{ data: response.data }};\n}};"
    );
    let graph = json!({
        "contentType": "graph",
        "nodes": [
            { "id": "in", "name": "Request", "type": "inputNode", "position": { "x": 0, "y": 0 } },
            { "id": "fn", "name": "Call", "type": "functionNode", "position": { "x": 0, "y": 0 }, "content": { "source": source } },
            { "id": "out", "name": "Response", "type": "outputNode", "position": { "x": 0, "y": 0 } }
        ],
        "edges": [
            { "id": "e1", "type": "edge", "sourceId": "in", "targetId": "fn" },
            { "id": "e2", "type": "edge", "sourceId": "fn", "targetId": "out" }
        ]
    });
    let root = std::env::temp_dir().join(format!("donka-function-nodes-{}", std::process::id()));
    std::fs::create_dir_all(root.join("rules")).unwrap();
    std::fs::write(root.join("rules").join("call.json"), graph.to_string()).unwrap();
    let config = EnvironmentConfig {
        provider: ProviderConfig::Filesystem(FilesystemProviderConfig {
            root_dir: root.to_string_lossy().into_owned(),
        }),
        ..Default::default()
    };
    let agent = app::create_agent(config.clone(), Default::default()).await;
    let router = app::create_app(agent, config).await;

    let request = Request::post("/api/projects/rules/evaluate/call.json")
        .header("Content-Type", "application/json")
        .body(Body::from(json!({ "context": {} }).to_string()))
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    let status = response.status();
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
            .unwrap_or(Value::Null);

    assert_ne!(status, StatusCode::OK, "{body}");
    assert!(
        body.to_string()
            .contains("HTTP requests from function nodes are disabled"),
        "{body}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}
