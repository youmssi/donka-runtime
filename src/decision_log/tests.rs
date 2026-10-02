use super::*;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use serde_json::json;
use std::collections::VecDeque;

/// Each request's `Authorization` header and body.
type Received = Arc<Mutex<Vec<(Option<String>, Value)>>>;

/// A stand-in for Studio's feed endpoint: answers with the queued statuses
/// (then 202) and keeps what it was sent.
#[derive(Clone, Default)]
struct Studio {
    answers: Arc<Mutex<VecDeque<StatusCode>>>,
    received: Received,
    calls: Arc<AtomicU64>,
    delay: Duration,
}

impl Studio {
    fn answering(statuses: &[StatusCode]) -> Self {
        Self {
            answers: Arc::new(Mutex::new(statuses.iter().copied().collect())),
            ..Default::default()
        }
    }

    async fn serve(self) -> String {
        let app = Router::new()
            .route(
                "/records",
                post(
                    |State(studio): State<Studio>, headers: HeaderMap, body: String| async move {
                        studio.calls.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(studio.delay).await;
                        let status = studio
                            .answers
                            .lock()
                            .unwrap()
                            .pop_front()
                            .unwrap_or(StatusCode::ACCEPTED);
                        if status.is_success() {
                            let auth = headers
                                .get("authorization")
                                .map(|v| v.to_str().unwrap().to_owned());
                            let body: Value = serde_json::from_str(&body).unwrap();
                            studio.received.lock().unwrap().push((auth, body));
                        }
                        (status, axum::Json(json!({ "accepted": 0, "rejected": [] })))
                    },
                ),
            )
            .with_state(self);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/records", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        url
    }

    /// The ids received, batch by batch.
    fn batches(&self) -> Vec<Vec<String>> {
        self.received
            .lock()
            .unwrap()
            .iter()
            .map(|(_, body)| {
                body["records"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|r| r["decisionKey"].as_str().unwrap().to_owned())
                    .collect()
            })
            .collect()
    }
}

fn config(url: String) -> DecisionLogConfig {
    DecisionLogConfig {
        url: Some(url),
        token: Some("dnk_log_test".into()),
        batch_size: 3,
        flush_interval: Duration::from_millis(50),
        ..Default::default()
    }
}

fn record(key: &str) -> Record {
    Record {
        id: Uuid::new_v4(),
        project_id: "p-1".into(),
        release_id: "r-1".into(),
        environment: "production".into(),
        decision_key: key.into(),
        reference: None,
        evaluated_at: Utc::now(),
        duration_us: 10,
        status: Status::Succeeded,
        input: json!({ "amount": 1 }),
        output: None,
        trace: None,
        error: None,
    }
}

/// Waits for `check` to hold, for up to five seconds.
async fn eventually(check: impl Fn() -> bool) {
    for _ in 0..500 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition not reached");
}

#[tokio::test]
async fn records_leave_in_batches_with_the_token() {
    let studio = Studio::default();
    let feed = Feed::from_config(&config(studio.clone().serve().await))
        .unwrap()
        .unwrap();
    for key in ["a", "b", "c", "d"] {
        feed.push(record(key));
    }

    eventually(|| studio.batches().concat().len() == 4).await;
    assert_eq!(studio.batches(), vec![vec!["a", "b", "c"], vec!["d"]]);
    let auth = studio.received.lock().unwrap()[0].0.clone();
    assert_eq!(auth.as_deref(), Some("Bearer dnk_log_test"));
}

#[tokio::test]
async fn a_batch_is_retried_while_studio_is_unavailable() {
    let studio = Studio::answering(&[
        StatusCode::SERVICE_UNAVAILABLE,
        StatusCode::TOO_MANY_REQUESTS,
    ]);
    let feed = Feed::from_config(&config(studio.clone().serve().await))
        .unwrap()
        .unwrap();
    feed.push(record("a"));

    eventually(|| studio.batches() == vec![vec!["a".to_owned()]]).await;
    assert_eq!(studio.calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn a_refused_batch_is_dropped_and_the_next_one_still_leaves() {
    let studio = Studio::answering(&[StatusCode::UNAUTHORIZED]);
    let feed = Feed::from_config(&config(studio.clone().serve().await))
        .unwrap()
        .unwrap();
    feed.push(record("refused"));
    eventually(|| studio.calls.load(Ordering::SeqCst) == 1).await;
    feed.push(record("next"));

    eventually(|| studio.batches() == vec![vec!["next".to_owned()]]).await;
    assert_eq!(studio.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_full_queue_drops_records_instead_of_waiting() {
    let studio = Studio {
        delay: Duration::from_secs(5),
        ..Default::default()
    };
    let url = studio.clone().serve().await;
    let feed = Feed::from_config(&DecisionLogConfig {
        batch_size: 1,
        queue_capacity: 2,
        ..config(url)
    })
    .unwrap()
    .unwrap();

    let started = Instant::now();
    for i in 0..50 {
        feed.push(record(&i.to_string()));
    }
    assert!(started.elapsed() < Duration::from_millis(100));
    assert!(feed.dropped.load(Ordering::Relaxed) >= 45);
}

#[tokio::test]
async fn queued_records_are_sent_at_shutdown() {
    let studio = Studio::default();
    let url = studio.clone().serve().await;
    let feed = Feed::from_config(&DecisionLogConfig {
        batch_size: 100,
        flush_interval: Duration::from_secs(60),
        ..config(url)
    })
    .unwrap()
    .unwrap();
    feed.push(record("a"));
    feed.push(record("b"));

    feed.shutdown().await;
    assert_eq!(studio.batches().concat(), vec!["a", "b"]);
}

#[tokio::test]
async fn the_feed_needs_both_url_and_token() {
    assert!(
        Feed::from_config(&DecisionLogConfig::default())
            .unwrap()
            .is_none()
    );
    let half = DecisionLogConfig {
        url: Some("https://studio.example/records".into()),
        ..Default::default()
    };
    assert!(Feed::from_config(&half).is_err());
    let not_http = DecisionLogConfig {
        url: Some("studio.example/records".into()),
        token: Some("t".into()),
        ..Default::default()
    };
    assert!(Feed::from_config(&not_http).is_err());
}

#[test]
fn the_reference_header_is_checked() {
    let mut headers = HeaderMap::new();
    assert_eq!(reference(&headers).unwrap(), None);

    headers.insert(
        REFERENCE_HEADER,
        HeaderValue::from_static(" APP-2026-0042 "),
    );
    assert_eq!(
        reference(&headers).unwrap().as_deref(),
        Some("APP-2026-0042")
    );

    headers.insert(REFERENCE_HEADER, HeaderValue::from_static("   "));
    assert!(reference(&headers).is_err());

    let long = "x".repeat(MAX_REFERENCE + 1);
    headers.insert(REFERENCE_HEADER, HeaderValue::from_str(&long).unwrap());
    assert!(reference(&headers).is_err());

    headers.insert(
        REFERENCE_HEADER,
        HeaderValue::from_bytes(b"caf\xc3\xa9").unwrap(),
    );
    assert!(reference(&headers).is_err());
}

#[tokio::test]
async fn the_trace_is_kept_for_the_log_and_removed_from_the_answer_unless_asked() {
    let studio = Studio::default();
    let feed: &'static Feed = Box::leak(Box::new(
        Feed::from_config(&config(studio.clone().serve().await))
            .unwrap()
            .unwrap(),
    ));
    let pending = |key: &str| Pending {
        feed,
        started: Instant::now(),
        record: record(key),
    };
    let answer = || json!({ "performance": "1ms", "result": { "ok": 1 }, "trace": { "n": {} } });

    let mut body = answer();
    let (name, value) = pending("hidden").succeeded(&mut body, false);
    assert_eq!(name, DECISION_ID_HEADER);
    assert!(Uuid::parse_str(value.to_str().unwrap()).is_ok());
    assert!(body.get("trace").is_none());

    let mut body = answer();
    pending("shown").succeeded(&mut body, true);
    assert!(body.get("trace").is_some());

    let error = json!({ "code": "evaluate.failed", "details": { "type": "NodeError" } });
    pending("failed").failed(&error, Some(json!({ "n": {} })));

    eventually(|| studio.batches().concat().len() == 3).await;
    let received = studio.received.lock().unwrap().clone();
    let records: Vec<Value> = received
        .iter()
        .flat_map(|(_, body)| body["records"].as_array().unwrap().clone())
        .collect();
    for record in &records[..2] {
        assert_eq!(record["trace"], json!({ "n": {} }));
        assert_eq!(record["output"], json!({ "ok": 1 }));
        assert_eq!(record["status"], json!("succeeded"));
    }
    assert_eq!(records[2]["status"], json!("failed"));
    assert_eq!(records[2]["error"], error);
    assert_eq!(records[2]["trace"], json!({ "n": {} }));
    assert!(records[2].get("output").is_none());
}
