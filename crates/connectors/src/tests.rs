use super::*;
use std::collections::HashMap;
use std::sync::Arc;
use zen_engine::DecisionEngine;
use zen_engine::model::DecisionContent;
use zen_engine::nodes::custom::CustomDecisionNode;

fn request(config: Value, input: Value) -> CustomNodeRequest {
    CustomNodeRequest {
        input: Variable::from(input),
        node: CustomDecisionNode {
            id: "bureau".into(),
            name: "Bureau".into(),
            kind: KIND.into(),
            config: Arc::new(config),
        },
    }
}

fn mock_config() -> Value {
    json!({
        "preset": "bureau-score",
        "url": "https://bureau.example/score",
        "auth": { "type": "bearer", "secret": "BUREAU_KEY" },
        "body": { "id": "{{ applicant.id }}" },
        "outputKey": "bureau",
        "mock": { "score": 712 }
    })
}

#[tokio::test]
async fn mock_adds_the_mock_response_to_the_input() {
    let input = json!({ "applicant": { "id": "A1" }, "$nodes": { "x": 1 } });
    let response = ConnectorAdapter::mock()
        .handle(request(mock_config(), input))
        .await
        .expect("mock response");
    assert_eq!(
        response.output.to_value(),
        json!({ "applicant": { "id": "A1" }, "bureau": { "score": 712 } })
    );
    assert_eq!(
        response.trace_data.expect("trace").to_value(),
        json!({ "mode": "mock", "outcome": "ok" })
    );
}

#[tokio::test]
async fn mock_without_a_mock_response_fails_even_with_a_fallback() {
    let mut config = mock_config();
    config.as_object_mut().unwrap().remove("mock");
    config["onError"] = json!("fallback");
    config["fallback"] = json!({ "score": 0 });
    let error = ConnectorAdapter::mock()
        .handle(request(config, json!({})))
        .await
        .expect_err("no mock");
    assert_eq!(error.node_id.as_ref(), "bureau");
    assert!(error.source.to_string().contains("no mock response"));
    assert_eq!(
        error.trace.expect("trace").to_value()["error"]["code"],
        json!("no_mock")
    );
}

#[tokio::test]
async fn another_kind_is_refused() {
    let mut request = request(mock_config(), json!({}));
    request.node.kind = "other".into();
    let error = ConnectorAdapter::mock()
        .handle(request)
        .await
        .expect_err("kind");
    assert!(
        error
            .source
            .to_string()
            .contains("unknown custom node kind other")
    );
}

#[test]
fn settings_are_checked() {
    let with = |key: &str, value: Value| {
        let mut config = mock_config();
        config[key] = value;
        parse(&config)
    };
    assert!(parse(&mock_config()).is_ok());
    assert!(with("url", json!("ftp://bureau.example")).is_err());
    assert!(with("outputKey", json!("")).is_err());
    assert!(with("outputKey", json!("bureau.score")).is_err());
    assert!(with("auth", json!({ "type": "bearer", "secret": "bureau_key" })).is_err());
    assert!(with("auth", json!({ "type": "bearer", "secret": "1KEY" })).is_err());
    assert!(
        with(
            "auth",
            json!({ "type": "header", "header": "X Api", "secret": "KEY" })
        )
        .is_err()
    );
    assert!(
        with(
            "auth",
            json!({ "type": "header", "header": "X-Api-Key", "secret": "KEY" })
        )
        .is_ok()
    );
    assert!(with("onError", json!("ignore")).is_err());
    assert!(
        with("secretValue", json!("abc")).is_err(),
        "unknown fields are refused"
    );
    assert!(valid_secret_name(&"K".repeat(MAX_SECRET_NAME)));
    assert!(!valid_secret_name(&"K".repeat(MAX_SECRET_NAME + 1)));
}

#[test]
fn body_templates_render_against_the_input() {
    let body = json!({
        "id": "{{ applicant.id }}",
        "income": "{{ applicant.income * 12 }}",
        "fixed": "plain",
        "list": ["{{ applicant.id }}", 3]
    });
    let input = Variable::from(json!({ "applicant": { "id": "A1", "income": 1000 } }));
    assert_eq!(
        render(&body, &input).expect("rendered"),
        json!({ "id": "A1", "income": 12000, "fixed": "plain", "list": ["A1", 3] })
    );
}

#[tokio::test]
async fn the_engine_runs_connector_nodes_through_the_adapter() {
    let graph: DecisionContent = serde_json::from_value(json!({
        "nodes": [
            { "id": "in", "name": "Request", "type": "inputNode", "position": { "x": 0, "y": 0 } },
            {
                "id": "bureau", "name": "Bureau", "type": "customNode", "position": { "x": 0, "y": 0 },
                "content": { "kind": KIND, "config": mock_config() }
            },
            { "id": "out", "name": "Response", "type": "outputNode", "position": { "x": 0, "y": 0 } }
        ],
        "edges": [
            { "id": "e1", "type": "edge", "sourceId": "in", "targetId": "bureau" },
            { "id": "e2", "type": "edge", "sourceId": "bureau", "targetId": "out" }
        ]
    }))
    .expect("graph");
    let engine = DecisionEngine::default().with_adapter(Arc::new(ConnectorAdapter::mock()));
    let decision = engine.create_decision(Arc::new(graph)).expect("decision");
    let response = decision
        .evaluate(Variable::from(json!({ "applicant": { "id": "A1" } })))
        .await
        .expect("evaluated");
    assert_eq!(
        response.result.to_value()["bureau"],
        json!({ "score": 712 })
    );
}

#[tokio::test]
async fn replay_answers_with_the_recorded_output() {
    let recorded = HashMap::from([(
        "bureau".to_owned(),
        json!({ "applicant": { "id": "A1" }, "bureau": { "score": 640 } }),
    )]);
    let response = ConnectorAdapter::replay(recorded)
        .handle(request(
            mock_config(),
            json!({ "applicant": { "id": "A1" } }),
        ))
        .await
        .expect("replayed");
    assert_eq!(
        response.output.to_value()["bureau"],
        json!({ "score": 640 })
    );
    assert_eq!(
        response.trace_data.expect("trace").to_value(),
        json!({ "mode": "replay", "outcome": "ok" })
    );
}

#[tokio::test]
async fn replay_without_a_recording_fails_even_with_a_fallback() {
    let mut config = mock_config();
    config["onError"] = json!("fallback");
    config["fallback"] = json!({ "score": 0 });
    let error = ConnectorAdapter::replay(HashMap::new())
        .handle(request(config, json!({})))
        .await
        .expect_err("nothing recorded");
    assert_eq!(
        error.trace.expect("trace").to_value()["error"]["code"],
        json!("not_recorded")
    );
}

#[tokio::test]
async fn a_traced_decision_replays_to_the_same_result_whatever_the_mock() {
    let graph = |mock: Value| -> DecisionContent {
        let mut config = mock_config();
        config["mock"] = mock;
        serde_json::from_value(json!({
            "nodes": [
                { "id": "in", "name": "Request", "type": "inputNode", "position": { "x": 0, "y": 0 } },
                {
                    "id": "bureau", "name": "Bureau", "type": "customNode", "position": { "x": 0, "y": 0 },
                    "content": { "kind": KIND, "config": config }
                },
                { "id": "out", "name": "Response", "type": "outputNode", "position": { "x": 0, "y": 0 } }
            ],
            "edges": [
                { "id": "e1", "type": "edge", "sourceId": "in", "targetId": "bureau" },
                { "id": "e2", "type": "edge", "sourceId": "bureau", "targetId": "out" }
            ]
        }))
        .expect("graph")
    };
    let input = || Variable::from(json!({ "applicant": { "id": "A1" } }));
    let options = zen_engine::EvaluationOptions {
        trace: true,
        max_depth: 10,
    };

    // The decision as it was made: the "service" answered 712.
    let made = DecisionEngine::default()
        .with_adapter(Arc::new(ConnectorAdapter::mock()))
        .create_decision(Arc::new(graph(json!({ "score": 712 }))))
        .expect("decision")
        .evaluate_with_opts(input(), options)
        .await
        .expect("evaluated");
    let trace = serde_json::to_value(made.trace.expect("trace")).expect("trace json");

    // Replayed where the service would now answer 1: the recording wins.
    let replayed = DecisionEngine::default()
        .with_adapter(Arc::new(ConnectorAdapter::replay(recorded_outputs(&trace))))
        .create_decision(Arc::new(graph(json!({ "score": 1 }))))
        .expect("decision")
        .evaluate_with_opts(input(), options)
        .await
        .expect("replayed");
    assert_eq!(replayed.result.to_value(), made.result.to_value());
}

#[cfg(feature = "live")]
mod live {
    use super::*;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::post;
    use axum::{Json, Router};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    const SECRET: &str = "s3cr3t-value";

    #[derive(Default)]
    struct Seen {
        hits: AtomicU32,
        headers: Mutex<Vec<HeaderMap>>,
        bodies: Mutex<Vec<Value>>,
    }

    struct Fake {
        base: String,
        seen: Arc<Seen>,
    }

    impl Fake {
        fn url(&self, path: &str) -> String {
            format!("{}{path}", self.base)
        }

        fn hits(&self) -> u32 {
            self.seen.hits.load(Ordering::SeqCst)
        }
    }

    async fn fake() -> Fake {
        async fn note(seen: &Seen, headers: HeaderMap, body: Value) -> u32 {
            seen.headers.lock().unwrap().push(headers);
            seen.bodies.lock().unwrap().push(body);
            seen.hits.fetch_add(1, Ordering::SeqCst) + 1
        }
        let seen = Arc::new(Seen::default());
        let app =
            Router::new()
                .route(
                    "/score",
                    post(
                        |State(seen): State<Arc<Seen>>,
                         headers: HeaderMap,
                         Json(body): Json<Value>| async move {
                            note(&seen, headers, body).await;
                            Json(json!({ "score": 712 }))
                        },
                    ),
                )
                .route(
                    "/down",
                    post(
                        |State(seen): State<Arc<Seen>>,
                         headers: HeaderMap,
                         Json(body): Json<Value>| async move {
                            note(&seen, headers, body).await;
                            StatusCode::SERVICE_UNAVAILABLE
                        },
                    ),
                )
                .route(
                    "/flaky",
                    post(
                        |State(seen): State<Arc<Seen>>,
                         headers: HeaderMap,
                         Json(body): Json<Value>| async move {
                            match note(&seen, headers, body).await {
                                1 => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
                                _ => Json(json!({ "score": 650 })).into_response(),
                            }
                        },
                    ),
                )
                .route(
                    "/refused",
                    post(
                        |State(seen): State<Arc<Seen>>,
                         headers: HeaderMap,
                         Json(body): Json<Value>| async move {
                            note(&seen, headers, body).await;
                            StatusCode::UNPROCESSABLE_ENTITY
                        },
                    ),
                )
                .route(
                    "/text",
                    post(
                        |State(seen): State<Arc<Seen>>,
                         headers: HeaderMap,
                         Json(body): Json<Value>| async move {
                            note(&seen, headers, body).await;
                            "not json"
                        },
                    ),
                )
                .route(
                    "/slow",
                    post(
                        |State(seen): State<Arc<Seen>>,
                         headers: HeaderMap,
                         Json(body): Json<Value>| async move {
                            note(&seen, headers, body).await;
                            tokio::time::sleep(Duration::from_secs(2)).await;
                            Json(json!({ "score": 1 }))
                        },
                    ),
                )
                .with_state(seen.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Fake { base, seen }
    }

    struct Fixed(HashMap<String, String>);

    impl Secrets for Fixed {
        fn get(&self, name: &str) -> Option<String> {
            self.0.get(name).cloned()
        }
    }

    fn adapter(limits: Limits) -> ConnectorAdapter {
        let secrets = Fixed(HashMap::from([(
            "BUREAU_KEY".to_owned(),
            SECRET.to_owned(),
        )]));
        ConnectorAdapter::live(limits, Arc::new(secrets))
    }

    fn config(url: String, extra: Value) -> Value {
        let mut config = json!({
            "url": url,
            "auth": { "type": "bearer", "secret": "BUREAU_KEY" },
            "body": { "id": "{{ applicant.id }}", "months": 12 },
            "outputKey": "bureau",
            "retries": 0,
            "mock": { "score": 1 }
        });
        for (key, value) in extra.as_object().unwrap() {
            config[key] = value.clone();
        }
        config
    }

    fn input() -> Value {
        json!({ "applicant": { "id": "A1" } })
    }

    fn assert_no_secret(value: &impl fmt::Debug) {
        assert!(
            !format!("{value:?}").contains(SECRET),
            "secret leaked: {value:?}"
        );
    }

    #[tokio::test]
    async fn calls_the_service_with_the_rendered_body_and_the_secret() {
        let fake = fake().await;
        let response = adapter(Limits::default())
            .handle(request(config(fake.url("/score"), json!({})), input()))
            .await
            .expect("response");
        let output = response.output.to_value();
        assert_eq!(
            output["bureau"],
            json!({ "score": 712 }),
            "the live answer, not the mock"
        );
        assert_eq!(output["applicant"], json!({ "id": "A1" }));
        assert_eq!(
            fake.seen.bodies.lock().unwrap()[0],
            json!({ "id": "A1", "months": 12 })
        );
        let headers = fake.seen.headers.lock().unwrap();
        assert_eq!(headers[0]["authorization"], format!("Bearer {SECRET}"));
        let trace = response.trace_data.expect("trace").to_value();
        assert_eq!(trace["mode"], json!("live"));
        assert_eq!(trace["outcome"], json!("ok"));
        assert_eq!(trace["attempts"], json!(1));
        assert_eq!(trace["status"], json!(200));
        assert_no_secret(&output);
        assert_no_secret(&trace);
    }

    #[tokio::test]
    async fn a_custom_header_carries_the_secret() {
        let fake = fake().await;
        let auth =
            json!({ "auth": { "type": "header", "header": "X-Api-Key", "secret": "BUREAU_KEY" } });
        adapter(Limits::default())
            .handle(request(config(fake.url("/score"), auth), input()))
            .await
            .expect("response");
        let headers = fake.seen.headers.lock().unwrap();
        assert_eq!(headers[0]["x-api-key"], SECRET);
        assert!(headers[0].get("authorization").is_none());
    }

    #[tokio::test]
    async fn a_server_error_is_retried() {
        let fake = fake().await;
        let response = adapter(Limits::default())
            .handle(request(
                config(fake.url("/flaky"), json!({ "retries": 2 })),
                input(),
            ))
            .await
            .expect("second attempt");
        assert_eq!(
            response.output.to_value()["bureau"],
            json!({ "score": 650 })
        );
        assert_eq!(
            response.trace_data.unwrap().to_value()["attempts"],
            json!(2)
        );
        assert_eq!(fake.hits(), 2);
    }

    #[tokio::test]
    async fn retries_are_capped_by_the_runtime() {
        let fake = fake().await;
        let limits = Limits {
            max_retries: 1,
            ..Limits::default()
        };
        let error = adapter(limits)
            .handle(request(
                config(fake.url("/down"), json!({ "retries": 3 })),
                input(),
            ))
            .await
            .expect_err("down");
        assert_eq!(fake.hits(), 2);
        let trace = error.trace.expect("trace").to_value();
        assert_eq!(
            trace["error"],
            json!({ "code": "status", "message": "the service answered 503" })
        );
        assert_eq!(trace["status"], json!(503));
    }

    #[tokio::test]
    async fn a_refusal_or_a_non_json_answer_is_not_retried() {
        let fake = fake().await;
        let adapter = adapter(Limits::default());
        let refused = adapter
            .handle(request(
                config(fake.url("/refused"), json!({ "retries": 2 })),
                input(),
            ))
            .await
            .expect_err("refused");
        assert!(refused.source.to_string().contains("422"));
        let text = adapter
            .handle(request(
                config(fake.url("/text"), json!({ "retries": 2 })),
                input(),
            ))
            .await
            .expect_err("text");
        assert!(text.source.to_string().contains("not JSON"));
        assert_eq!(fake.hits(), 2);
    }

    #[tokio::test]
    async fn a_slow_service_times_out() {
        let fake = fake().await;
        let error = adapter(Limits::default())
            .handle(request(
                config(fake.url("/slow"), json!({ "timeoutMs": 100 })),
                input(),
            ))
            .await
            .expect_err("timeout");
        assert_eq!(
            error.trace.unwrap().to_value()["error"]["code"],
            json!("timeout")
        );
        assert!(error.source.to_string().contains("100 ms"));
    }

    #[tokio::test]
    async fn the_fallback_answers_when_the_service_fails() {
        let fake = fake().await;
        let extra =
            json!({ "onError": "fallback", "fallback": { "score": null, "unavailable": true } });
        let response = adapter(Limits::default())
            .handle(request(config(fake.url("/down"), extra), input()))
            .await
            .expect("fallback");
        assert_eq!(
            response.output.to_value()["bureau"],
            json!({ "score": null, "unavailable": true })
        );
        let trace = response.trace_data.unwrap().to_value();
        assert_eq!(trace["outcome"], json!("fallback"));
        assert_eq!(trace["error"]["code"], json!("status"));
    }

    #[tokio::test]
    async fn an_unreachable_service_falls_back_too() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/score", listener.local_addr().unwrap());
        drop(listener);
        let extra = json!({ "onError": "fallback", "fallback": { "score": 0 } });
        let response = adapter(Limits::default())
            .handle(request(config(url, extra), input()))
            .await
            .expect("fallback");
        assert_eq!(
            response.trace_data.unwrap().to_value()["error"]["code"],
            json!("unreachable")
        );
    }

    #[tokio::test]
    async fn the_circuit_opens_after_repeated_failures() {
        let fake = fake().await;
        let limits = Limits {
            breaker_failures: 2,
            breaker_cooldown: Duration::from_millis(300),
            ..Limits::default()
        };
        let adapter = adapter(limits);
        let call = |url: String| adapter.handle(request(config(url, json!({})), input()));
        call(fake.url("/down")).await.expect_err("first");
        call(fake.url("/down")).await.expect_err("second");
        let open = call(fake.url("/down")).await.expect_err("open");
        assert_eq!(
            open.trace.unwrap().to_value()["error"]["code"],
            json!("circuit_open")
        );
        assert_eq!(fake.hits(), 2, "an open circuit does not call the service");
        call(fake.url("/score"))
            .await
            .expect("another URL has its own circuit");

        tokio::time::sleep(Duration::from_millis(350)).await;
        call(fake.url("/down"))
            .await
            .expect_err("half-open: one call goes through");
        assert_eq!(fake.hits(), 4);
    }

    #[tokio::test]
    async fn a_missing_secret_is_named_not_called() {
        let fake = fake().await;
        let auth =
            json!({ "auth": { "type": "bearer", "secret": "OTHER_KEY" }, "onError": "fallback" });
        let response = adapter(Limits::default())
            .handle(request(config(fake.url("/score"), auth), input()))
            .await
            .expect("falls back");
        let trace = response.trace_data.unwrap().to_value();
        assert_eq!(
            trace["error"],
            json!({ "code": "missing_secret", "message": "secret OTHER_KEY is not configured on the Runtime" })
        );
        assert_eq!(fake.hits(), 0);
    }

    #[tokio::test]
    async fn errors_never_carry_the_secret() {
        let fake = fake().await;
        let error = adapter(Limits::default())
            .handle(request(config(fake.url("/down"), json!({})), input()))
            .await
            .expect_err("down");
        assert_no_secret(&error.source.to_string());
        assert_no_secret(&error.trace);
    }

    #[test]
    fn env_secrets_read_the_prefixed_variable() {
        // SAFETY: the variable name is unique to this test and nothing else reads it.
        unsafe { std::env::set_var("DONKA_SECRET_CONNECTORS_TEST_KEY", "value") };
        assert_eq!(
            EnvSecrets.get("CONNECTORS_TEST_KEY").as_deref(),
            Some("value")
        );
        assert_eq!(EnvSecrets.get("CONNECTORS_TEST_ABSENT"), None);
    }
}
