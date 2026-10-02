//! Connector nodes for Donka decisions.
//!
//! A connector is a custom node (`kind: "donka.connector"`) that calls an
//! outside service (a credit bureau, a KYC or AML provider) with JSON and
//! hands on its input with the response added under `outputKey`. The node's
//! `config` holds the URL, the request body (string values may be templates
//! such as `{{ applicant.nationalId }}`), the **name** of the secret that
//! authenticates the call, and what to do when the call fails: fail the
//! decision, or continue with the `fallback` the author defined.
//!
//! The same handler runs in three modes. [`ConnectorAdapter::mock`] answers
//! with the node's `mock` response and never leaves the process: it is what
//! Studio's simulator uses. [`ConnectorAdapter::replay`] answers with what
//! the service answered when a logged decision was made, so Studio can
//! re-evaluate it without calling anyone. [`ConnectorAdapter::live`]
//! (feature `live`, the Runtime) makes the call with a timeout, bounded
//! retries and a circuit breaker per URL, and reads secret values from its
//! [`Secrets`]. No secret value ever appears in a node's output, trace or
//! error.

use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use zen_engine::Variable;
use zen_engine::nodes::custom::{CustomNodeAdapter, CustomNodeRequest};
use zen_engine::nodes::{NodeError, NodeResponse, NodeResult};

#[cfg(feature = "live")]
mod live;
#[cfg(feature = "live")]
pub use live::{EnvSecrets, Limits, Secrets};

/// The custom node kind connectors use in a decision graph.
pub const KIND: &str = "donka.connector";
/// Prefix of the environment variable holding a secret: `BUREAU_KEY` is read
/// from `DONKA_SECRET_BUREAU_KEY`.
pub const SECRET_ENV_PREFIX: &str = "DONKA_SECRET_";
/// Longest secret name accepted.
pub const MAX_SECRET_NAME: usize = 64;

/// A connector node's settings, as the decision stores them.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectorConfig {
    /// Which template the node was made from (`http`, `bureau-score`); informational.
    #[serde(default)]
    pub preset: Option<String>,
    /// Where the JSON request is POSTed.
    pub url: String,
    #[serde(default)]
    pub auth: Auth,
    /// The request body; string values may be templates over the node's input.
    #[serde(default)]
    pub body: Value,
    /// Where the response goes in the node's output.
    pub output_key: String,
    /// Time allowed per attempt, in milliseconds (capped by the handler).
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Attempts after the first one (capped by the handler).
    #[serde(default)]
    pub retries: Option<u32>,
    #[serde(default)]
    pub on_error: OnError,
    /// What `outputKey` holds when the call fails and `onError` is `fallback`.
    #[serde(default)]
    pub fallback: Value,
    /// What `outputKey` holds in simulation.
    #[serde(default)]
    pub mock: Option<Value>,
}

/// How the call is authenticated. Only the secret's **name** is stored.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum Auth {
    #[default]
    None,
    /// `Authorization: Bearer <secret>`.
    Bearer { secret: String },
    /// `<header>: <secret>`, e.g. `X-Api-Key`.
    Header { header: String, secret: String },
}

impl Auth {
    fn secret(&self) -> Option<&str> {
        match self {
            Auth::None => None,
            Auth::Bearer { secret } | Auth::Header { secret, .. } => Some(secret),
        }
    }
}

/// What the decision does when the call still fails after its retries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OnError {
    /// The evaluation fails with the connector's error.
    #[default]
    Fail,
    /// The evaluation continues with `fallback` under `outputKey`.
    Fallback,
}

/// Why a connector node could not produce a response.
#[derive(Debug, thiserror::Error)]
pub enum ConnectorError {
    #[error("invalid connector settings: {0}")]
    InvalidConfig(String),
    #[error("could not build the request: {0}")]
    Template(String),
    #[error("no mock response: add one to the connector node to simulate it")]
    NoMock,
    #[error("the logged decision holds no answer from this connector")]
    NotRecorded,
    #[error("secret {0} is not configured on the Runtime")]
    MissingSecret(String),
    #[error("the service did not answer within {0} ms")]
    Timeout(u64),
    #[error("the service could not be reached")]
    Unreachable,
    #[error("the service answered {0}")]
    Status(u16),
    #[error("the service's answer is not JSON")]
    NotJson,
    #[error("the service failed repeatedly; calls are paused for a moment")]
    CircuitOpen,
}

impl ConnectorError {
    /// A short, stable code for traces and dashboards.
    pub fn code(&self) -> &'static str {
        match self {
            ConnectorError::InvalidConfig(_) => "invalid_config",
            ConnectorError::Template(_) => "template",
            ConnectorError::NoMock => "no_mock",
            ConnectorError::NotRecorded => "not_recorded",
            ConnectorError::MissingSecret(_) => "missing_secret",
            ConnectorError::Timeout(_) => "timeout",
            ConnectorError::Unreachable => "unreachable",
            ConnectorError::Status(_) => "status",
            ConnectorError::NotJson => "not_json",
            ConnectorError::CircuitOpen => "circuit_open",
        }
    }
}

/// How a call went, for the node's trace. Holds no secret and no response body.
#[derive(Debug, Default)]
pub struct CallReport {
    pub attempts: u32,
    pub status: Option<u16>,
    pub duration_ms: u128,
}

enum Mode {
    Mock,
    /// Each connector node's output when the decision was made, by node id.
    Replay(HashMap<String, Value>),
    #[cfg(feature = "live")]
    Live(live::Live),
}

/// The custom node handler for connector nodes.
pub struct ConnectorAdapter {
    mode: Mode,
}

impl fmt::Debug for ConnectorAdapter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectorAdapter")
            .field("mode", &self.mode_name())
            .finish()
    }
}

impl ConnectorAdapter {
    /// Answers with each node's `mock` response; nothing leaves the process.
    pub fn mock() -> Self {
        Self { mode: Mode::Mock }
    }

    /// Answers with what each connector answered when a logged decision was
    /// made: `outputs` maps node ids to the node's output in that decision's
    /// trace. Nothing leaves the process.
    pub fn replay(outputs: HashMap<String, Value>) -> Self {
        Self {
            mode: Mode::Replay(outputs),
        }
    }

    fn mode_name(&self) -> &'static str {
        match self.mode {
            Mode::Mock => "mock",
            Mode::Replay(_) => "replay",
            #[cfg(feature = "live")]
            Mode::Live(_) => "live",
        }
    }

    /// Calls the services for real, within `limits`, with secret values from `secrets`.
    #[cfg(feature = "live")]
    pub fn live(limits: Limits, secrets: std::sync::Arc<dyn Secrets>) -> Self {
        Self {
            mode: Mode::Live(live::Live::new(limits, secrets)),
        }
    }

    async fn respond(&self, request: CustomNodeRequest) -> NodeResult {
        let node_id = request.node.id.clone();
        let fail = |error: ConnectorError, trace: Option<Value>| NodeError {
            node_id: node_id.clone(),
            trace: trace.map(Variable::from),
            source: error.to_string().into(),
        };
        if request.node.kind.as_ref() != KIND {
            return Err(fail(
                ConnectorError::InvalidConfig(format!(
                    "unknown custom node kind {}",
                    request.node.kind
                )),
                None,
            ));
        }
        let config = parse(&request.node.config).map_err(|error| fail(error, None))?;
        let input = without_nodes(request.input.to_value());

        let (result, report) = match &self.mode {
            Mode::Mock => (
                config.mock.clone().ok_or(ConnectorError::NoMock),
                CallReport::default(),
            ),
            Mode::Replay(outputs) => (
                outputs
                    .get(node_id.as_ref())
                    .and_then(|output| output.get(&config.output_key))
                    .cloned()
                    .ok_or(ConnectorError::NotRecorded),
                CallReport::default(),
            ),
            #[cfg(feature = "live")]
            Mode::Live(live) => match render(&config.body, &request.input) {
                Ok(body) => live.call(&config, body).await,
                Err(error) => (Err(error), CallReport::default()),
            },
        };
        let mode = self.mode_name();
        match result {
            Ok(response) => Ok(NodeResponse {
                output: Variable::from(with_output(input, &config.output_key, response)),
                trace_data: Some(Variable::from(trace(mode, "ok", &report, None))),
            }),
            Err(error) if config.on_error == OnError::Fallback && fallback_allowed(&error) => {
                Ok(NodeResponse {
                    output: Variable::from(with_output(
                        input,
                        &config.output_key,
                        config.fallback.clone(),
                    )),
                    trace_data: Some(Variable::from(trace(
                        mode,
                        "fallback",
                        &report,
                        Some(&error),
                    ))),
                })
            }
            Err(error) => {
                let trace = trace(mode, "error", &report, Some(&error));
                Err(fail(error, Some(trace)))
            }
        }
    }
}

impl CustomNodeAdapter for ConnectorAdapter {
    fn handle(&self, request: CustomNodeRequest) -> Pin<Box<dyn Future<Output = NodeResult> + '_>> {
        Box::pin(self.respond(request))
    }
}

/// Each node's output in a decision's serialized trace (`{ "<node id>":
/// { "output": … } }`), the answers [`ConnectorAdapter::replay`] takes.
pub fn recorded_outputs(trace: &Value) -> HashMap<String, Value> {
    trace
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(id, node)| Some((id.clone(), node.get("output")?.clone())))
        .collect()
}

/// A node's settings, checked: the URL is http(s), names are valid, and the
/// output key is a plain field name.
pub fn parse(config: &Value) -> Result<ConnectorConfig, ConnectorError> {
    let config: ConnectorConfig = serde_json::from_value(config.clone())
        .map_err(|e| ConnectorError::InvalidConfig(e.to_string()))?;
    if !(config.url.starts_with("https://") || config.url.starts_with("http://")) {
        return Err(ConnectorError::InvalidConfig(
            "url must start with http:// or https://".into(),
        ));
    }
    if config.output_key.is_empty()
        || !config
            .output_key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(ConnectorError::InvalidConfig(
            "outputKey must be letters, digits and underscores".into(),
        ));
    }
    if let Some(secret) = config.auth.secret()
        && !valid_secret_name(secret)
    {
        return Err(ConnectorError::InvalidConfig(format!(
            "secret names are 1 to {MAX_SECRET_NAME} capital letters, digits and underscores, starting with a letter"
        )));
    }
    if let Auth::Header { header, .. } = &config.auth
        && (header.is_empty()
            || !header
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-'))
    {
        return Err(ConnectorError::InvalidConfig(
            "header names are letters, digits and dashes".into(),
        ));
    }
    Ok(config)
}

/// `BUREAU_API_KEY`: capital letters, digits and underscores, starting with a letter.
pub fn valid_secret_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_SECRET_NAME
        && name.starts_with(|c: char| c.is_ascii_uppercase())
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// A failure that is the service's, not the author's: only these fall back.
fn fallback_allowed(error: &ConnectorError) -> bool {
    !matches!(
        error,
        ConnectorError::InvalidConfig(_)
            | ConnectorError::Template(_)
            | ConnectorError::NoMock
            | ConnectorError::NotRecorded
    )
}

/// Renders every string of `body` that holds a template, against the node's input.
pub fn render(body: &Value, input: &Variable) -> Result<Value, ConnectorError> {
    Ok(match body {
        Value::String(text) if text.contains("{{") => {
            let rendered = zen_tmpl::render(text, input.clone())
                .map_err(|e| ConnectorError::Template(e.to_string()))?;
            rendered.to_value()
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| render(item, input))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| Ok((key.clone(), render(value, input)?)))
                .collect::<Result<Map<_, _>, ConnectorError>>()?,
        ),
        other => other.clone(),
    })
}

/// The node's input as it reached the node, without the engine's `$nodes`.
fn without_nodes(mut input: Value) -> Value {
    if let Value::Object(map) = &mut input {
        map.remove("$nodes");
    }
    input
}

/// The input with the response added under `key`.
fn with_output(input: Value, key: &str, response: Value) -> Value {
    let mut map = match input {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    map.insert(key.to_owned(), response);
    Value::Object(map)
}

fn trace(mode: &str, outcome: &str, report: &CallReport, error: Option<&ConnectorError>) -> Value {
    let mut trace = json!({ "mode": mode, "outcome": outcome });
    if report.attempts > 0 {
        trace["attempts"] = json!(report.attempts);
        trace["durationMs"] = json!(report.duration_ms);
    }
    if let Some(status) = report.status {
        trace["status"] = json!(status);
    }
    if let Some(error) = error {
        trace["error"] = json!({ "code": error.code(), "message": error.to_string() });
    }
    trace
}

#[cfg(test)]
mod tests;
