use crate::Agent;
use crate::config::BatchEvaluationConfig;
use crate::contract;
use crate::decision_log;
use crate::engine_ext::EngineExtension;
use anyhow::{Context, anyhow};
use axum::extract::Path;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tokio_util::task::LocalPoolHandle;
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use uuid::Uuid;
use zen_engine::EvaluationOptions;

#[derive(Deserialize, utoipa::ToSchema)]
pub struct EvaluateRequest {
    context: Value,
    trace: Option<bool>,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct BatchEvaluateRequest {
    inputs: Vec<BatchEvaluateInput>,
    trace: Option<bool>,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct BatchEvaluateInput {
    input: Value,
    #[serde(default)]
    reference: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BatchEvaluateResponse {
    batch_id: String,
    items: Vec<BatchEvaluateItem>,
}

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BatchEvaluateItem {
    index: usize,
    status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    decision_id: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EvaluateResponse {
    details: EvaluateDetailsResponse,

    #[serde(flatten)]
    graph_response: Value,
}

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EvaluateDetailsResponse {
    release_id: Option<Arc<str>>,
    version_id: Option<Arc<str>>,
}

#[utoipa::path(
    post,
    path = "/api/projects/{project}/evaluate/{*key}",
    params(
        ("project" = String, Path, description = "Project slug or id"),
        ("key" = String, Path, description = "Key (path) of decision model")
    ),
    request_body = EvaluateRequest,
    responses(
        (status = OK, body = EvaluateResponse)
    )
)]
pub async fn evaluate(
    headers: HeaderMap,
    Extension(local_pool): Extension<LocalPoolHandle>,
    Extension(agent): Extension<Agent>,
    Path((project, key)): Path<(Arc<str>, Arc<str>)>,
    Json(payload): Json<EvaluateRequest>,
) -> Result<Response, EvaluateError> {
    let span = Span::current();

    span.set_attribute("params.project", project.clone());
    span.set_attribute("params.key", key.clone());

    let Some(project_data) = agent.project(&project) else {
        let error = (StatusCode::NOT_FOUND, anyhow!("Project not found"));
        return Err(error.into());
    };

    if let Some(release_data) = project_data.engine.release_data() {
        if let Some(id) = release_data.release_id() {
            span.set_attribute("release.id", id.clone());
        }
        if let Some(version) = release_data.release_version() {
            span.set_attribute("release.version", version.clone());
        }
        if let Some(id) = release_data.project_id() {
            span.set_attribute("project.id", id.clone());
        }
        if let Some(key) = release_data.project_key() {
            span.set_attribute("project.key", key.clone());
        }
    };

    let access_token = headers
        .get("X-Access-Token")
        .map(|h| h.to_str().unwrap_or("").to_string())
        .unwrap_or_default();

    if !project_data.engine.can_access(access_token.as_str()) {
        let error = (
            StatusCode::UNAUTHORIZED,
            anyhow!("Invalid X-Access-Token Header"),
        );
        return Err(error.into());
    }

    let reference = decision_log::reference(&headers)
        .map_err(|error| EvaluateError::from(anyhow::Error::new(error)))?;
    let logged = decision_log::begin(
        project_data.engine.release_data().as_ref(),
        &key,
        reference,
        &payload.context,
    );
    let wants_trace = payload.trace.unwrap_or(false);
    // The decision log keeps the trace for replay.
    let trace = wants_trace || logged.is_some();

    let cloned_project_data = project_data.clone();
    let cloned_key = key.clone();
    let result = local_pool
        .spawn_pinned(move || async move {
            cloned_project_data
                .engine
                .evaluate_with_opts(
                    &cloned_key,
                    payload.context.into(),
                    EvaluationOptions {
                        trace,
                        max_depth: 10,
                    },
                )
                .await
                .map(|s| serde_json::to_value(s).context("Failed to serialize value"))
                .map_err(|e| {
                    (
                        anyhow::Error::msg(e.to_string()),
                        decision_log::error_trace(&e),
                        contract::violation(&cloned_project_data.engine, &cloned_key, &e),
                    )
                })
        })
        .await
        .expect("Thread failed to join");
    let result = match result {
        Ok(result) => result,
        Err((error, trace, violation)) => {
            tracing::error!(error = debug(&error), "Failed to evaluate decision model");
            let error = match violation {
                Some(violation) => EvaluateError::Contract(error, violation),
                None => error.into(),
            };
            return Ok(failed(logged, error, trace));
        }
    };

    let mut result = match result {
        Ok(result) => result,
        Err(error) => {
            tracing::error!(error = debug(&error), "Failed to serialize the response.");
            return Ok(failed(logged, error.into(), None));
        }
    };
    let decision_id = logged.map(|logged| logged.succeeded(&mut result, wants_trace));

    let release_data = project_data.engine.release_data();

    let release_id = release_data.and_then(|r| r.release_id().cloned());
    let version_id = project_data.engine.get_version(&key);

    let mut response = Json(EvaluateResponse {
        graph_response: result,
        details: EvaluateDetailsResponse {
            version_id,
            release_id,
        },
    })
    .into_response();
    if let Some((name, value)) = decision_id {
        response.headers_mut().insert(name, value);
    }
    Ok(response)
}

/// Evaluates ordered inputs against the same project release. Each item gets
/// an independent result and decision-log record.
#[utoipa::path(
    post,
    path = "/api/projects/{project}/evaluate-batch/{*key}",
    params(
        ("project" = String, Path, description = "Project slug or id"),
        ("key" = String, Path, description = "Key (path) of decision model")
    ),
    request_body = BatchEvaluateRequest,
    responses((status = OK, body = BatchEvaluateResponse))
)]
pub async fn evaluate_batch(
    headers: HeaderMap,
    Extension(local_pool): Extension<LocalPoolHandle>,
    Extension(agent): Extension<Agent>,
    Extension(limits): Extension<BatchEvaluationConfig>,
    limiter: Option<Extension<crate::rate_limit::RateLimiter>>,
    Path((project, key)): Path<(Arc<str>, Arc<str>)>,
    Json(payload): Json<BatchEvaluateRequest>,
) -> Result<Json<BatchEvaluateResponse>, EvaluateError> {
    let Some(project_data) = agent.project(&project) else {
        return Err((StatusCode::NOT_FOUND, anyhow!("Project not found")).into());
    };
    let access_token = headers
        .get("X-Access-Token")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if !project_data.engine.can_access(access_token) {
        return Err((
            StatusCode::UNAUTHORIZED,
            anyhow!("Invalid X-Access-Token Header"),
        )
            .into());
    }
    if payload.inputs.is_empty() || payload.inputs.len() > limits.max_inputs {
        return Err((
            StatusCode::BAD_REQUEST,
            anyhow!(
                "Batch must contain between 1 and {} inputs",
                limits.max_inputs
            ),
        )
            .into());
    }
    let batch_reference = decision_log::reference(&headers)
        .map_err(|error| EvaluateError::from(anyhow::Error::new(error)))?;
    let inputs = payload
        .inputs
        .into_iter()
        .map(|item| {
            let reference = item
                .reference
                .map(|value| decision_log::validate_reference(&value))
                .transpose()
                .map_err(|error| EvaluateError::from(anyhow::Error::new(error)))?
                .or_else(|| batch_reference.clone());
            Ok((item.input, reference))
        })
        .collect::<Result<Vec<_>, EvaluateError>>()?;
    let release = project_data.engine.release_data();
    let batch_id = Uuid::new_v4();
    let mut items = Vec::with_capacity(inputs.len());
    for (index, (input, reference)) in inputs.into_iter().enumerate() {
        if let Some(Extension(limiter)) = &limiter {
            if let Err(wait) = limiter.charge(access_token) {
                let seconds = wait.as_secs() + u64::from(wait.subsec_nanos() > 0);
                items.push(BatchEvaluateItem {
                    index,
                    status: StatusCode::TOO_MANY_REQUESTS.as_u16(),
                    result: None,
                    error: Some(serde_json::json!({"message": format!("Too many requests for this access token; retry in {seconds} s"), "retryAfter": seconds})),
                    decision_id: None,
                });
                continue;
            }
        }
        let logged =
            decision_log::begin_batch(release.as_ref(), &key, reference, &input, Some(batch_id));
        let wants_trace = payload.trace.unwrap_or(false);
        let trace = wants_trace || logged.is_some();
        let engine = project_data.clone();
        let evaluation_key = key.clone();
        let evaluation_input = input.clone();
        let evaluated = local_pool
            .spawn_pinned(move || async move {
                engine
                    .engine
                    .evaluate_with_opts(
                        &evaluation_key,
                        evaluation_input.into(),
                        EvaluationOptions {
                            trace,
                            max_depth: 10,
                        },
                    )
                    .await
                    .map(|value| serde_json::to_value(value).context("Failed to serialize value"))
                    .map_err(|error| {
                        (
                            error.to_string(),
                            decision_log::error_trace(&error),
                            contract::violation(&engine.engine, &evaluation_key, &error),
                        )
                    })
            })
            .await
            .expect("Thread failed to join");
        match evaluated {
            Ok(Ok(mut value)) => {
                let decision_id = logged.map(|entry| {
                    let (_, value) = entry.succeeded(&mut value, wants_trace);
                    value.to_str().unwrap_or_default().to_owned()
                });
                items.push(BatchEvaluateItem {
                    index,
                    status: 200,
                    result: Some(value),
                    error: None,
                    decision_id,
                });
            }
            Ok(Err(error)) => {
                let body = serde_json::json!({"message": "Failed to serialize the response"});
                tracing::error!(error = %error, "Failed to serialize the response");
                let decision_id = logged.map(|entry| {
                    let (_, value) = entry.failed(&body, None);
                    value.to_str().unwrap_or_default().to_owned()
                });
                items.push(BatchEvaluateItem {
                    index,
                    status: 500,
                    result: None,
                    error: Some(body),
                    decision_id,
                });
            }
            Err((message, trace, violation)) => {
                let failure: EvaluateError = match violation {
                    Some(violation) => EvaluateError::Contract(anyhow!(message), violation),
                    None => EvaluateError::Anyhow((StatusCode::BAD_REQUEST, anyhow!(message))),
                };
                let (status, body) = failure.parts();
                let decision_id = logged.map(|entry| {
                    let (_, value) = entry.failed(&body, trace);
                    value.to_str().unwrap_or_default().to_owned()
                });
                items.push(BatchEvaluateItem {
                    index,
                    status: status.as_u16(),
                    result: None,
                    error: Some(body),
                    decision_id,
                });
            }
        }
    }
    Ok(Json(BatchEvaluateResponse {
        batch_id: batch_id.to_string(),
        items,
    }))
}

/// The error answer, with the decision log's record of it.
fn failed(
    logged: Option<decision_log::Pending>,
    error: EvaluateError,
    trace: Option<Value>,
) -> Response {
    let (status, body) = error.parts();
    let decision_id = logged.map(|logged| logged.failed(&body, trace));
    let mut response = (status, Json(body)).into_response();
    if let Some((name, value)) = decision_id {
        response.headers_mut().insert(name, value);
    }
    response
}

pub enum EvaluateError {
    EngineError(Box<zen_engine::EvaluationError>),
    Anyhow((StatusCode, anyhow::Error)),
    /// The request breaks the decision's input contract (DNK-37).
    Contract(anyhow::Error, contract::Violation),
}

impl EvaluateError {
    fn parts(self) -> (StatusCode, Value) {
        match self {
            EvaluateError::EngineError(error) => (
                StatusCode::BAD_REQUEST,
                serde_json::to_value(&error).unwrap_or_default(),
            ),
            EvaluateError::Anyhow((status, error)) => {
                (status, serde_json::json!({ "message": error.to_string() }))
            }
            EvaluateError::Contract(error, violation) => (
                StatusCode::BAD_REQUEST,
                serde_json::json!({ "message": error.to_string(), "contract": violation }),
            ),
        }
    }
}

impl IntoResponse for EvaluateError {
    fn into_response(self) -> Response {
        let (status, body) = self.parts();
        (status, Json(body)).into_response()
    }
}

impl From<Box<zen_engine::EvaluationError>> for EvaluateError {
    fn from(value: Box<zen_engine::EvaluationError>) -> Self {
        Self::EngineError(value)
    }
}

impl From<anyhow::Error> for EvaluateError {
    fn from(value: anyhow::Error) -> Self {
        Self::Anyhow((StatusCode::BAD_REQUEST, value))
    }
}

impl From<(StatusCode, anyhow::Error)> for EvaluateError {
    fn from(value: (StatusCode, anyhow::Error)) -> Self {
        Self::Anyhow(value)
    }
}
