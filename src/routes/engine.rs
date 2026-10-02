use crate::Agent;
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
use zen_engine::EvaluationOptions;

#[derive(Deserialize, utoipa::ToSchema)]
pub struct EvaluateRequest {
    context: Value,
    trace: Option<bool>,
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
                    )
                })
        })
        .await
        .expect("Thread failed to join");
    let result = match result {
        Ok(result) => result,
        Err((error, trace)) => {
            tracing::error!(error = debug(&error), "Failed to evaluate decision model");
            return Ok(failed(logged, error.into(), trace));
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
