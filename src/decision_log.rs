//! Donka decision log (DNK-18): a record of every evaluation goes to Donka
//! Studio, in the background, so the caller never waits for it.
//!
//! Records queue in memory and leave in batches. While Studio cannot be
//! reached a batch is retried with backoff; when the queue is full, new
//! records are dropped and counted in the logs rather than slowing
//! evaluations down. An answer Studio refuses outright (a bad token, a batch
//! too large) is logged and dropped: sending it again would not change it.
//! When the Runtime stops, what is still queued is sent within
//! `DECISION_LOG__SHUTDOWN_TIMEOUT`.
//!
//! A record holds the input, the output (or the error), the trace and the
//! release that answered. The trace is always recorded, so Studio can replay
//! the decision, and is returned to the caller only when they asked for it.
//! Records never hold an access token, and connector traces never hold a
//! secret (`donka-connectors`).

use crate::config::DecisionLogConfig;
use crate::data::release_data::ReleaseData;
use axum::http::{HeaderMap, HeaderName, HeaderValue};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use zen_engine::EvaluationError;

/// The caller's own reference for a decision (an application or customer
/// number), to find it in Studio's decision log.
pub const REFERENCE_HEADER: HeaderName = HeaderName::from_static("x-donka-reference");
/// The id of the decision's record, on every answer that was logged.
pub const DECISION_ID_HEADER: HeaderName = HeaderName::from_static("x-decision-id");
/// Longest reference accepted.
pub const MAX_REFERENCE: usize = 200;

const MAX_BACKOFF: Duration = Duration::from_secs(30);

static FEED: OnceLock<Option<Feed>> = OnceLock::new();

/// Starts the process-wide feed when it is configured. Runs at startup; a
/// later call (another agent in the same process, as in tests) keeps the first.
pub fn init(config: &DecisionLogConfig) -> anyhow::Result<()> {
    if FEED.get().is_some() {
        return Ok(());
    }
    let feed = Feed::from_config(config)?;
    match &feed {
        Some(_) => tracing::info!(url = ?config.url, "Decision log feed on"),
        None => tracing::info!("Decision log feed off (DECISION_LOG__URL is not set)"),
    }
    // Lost a race with another first call: the feed it started is kept.
    let _ = FEED.set(feed);
    Ok(())
}

/// Sends what is still queued, within the configured time. Runs when the
/// Runtime stops.
pub async fn shutdown() {
    if let Some(Some(feed)) = FEED.get() {
        feed.shutdown().await;
    }
}

/// The `X-Donka-Reference` header, checked: 1 to [`MAX_REFERENCE`] visible
/// ASCII characters.
pub fn reference(headers: &HeaderMap) -> Result<Option<String>, InvalidReference> {
    let Some(value) = headers.get(REFERENCE_HEADER) else {
        return Ok(None);
    };
    let text = value.to_str().map_err(|_| InvalidReference)?.trim();
    if text.is_empty() || text.len() > MAX_REFERENCE {
        return Err(InvalidReference);
    }
    Ok(Some(text.to_owned()))
}

#[derive(Debug, thiserror::Error)]
#[error("X-Donka-Reference must be 1 to {MAX_REFERENCE} visible ASCII characters")]
pub struct InvalidReference;

/// Starts a record for an evaluation, when the feed is on and the release
/// names its project, release and environment (older artifacts may not).
pub fn begin(
    release: Option<&ReleaseData>,
    key: &str,
    reference: Option<String>,
    input: &Value,
) -> Option<Pending> {
    let feed = FEED.get()?.as_ref()?;
    let release = release?;
    let (Some(project_id), Some(release_id), Some(environment)) = (
        release.project_id(),
        release.release_id(),
        release.environment.as_ref().and_then(|e| e.key.as_ref()),
    ) else {
        tracing::debug!(
            "Not logged: the release does not name its project, release and environment"
        );
        return None;
    };
    Some(Pending {
        feed,
        started: Instant::now(),
        record: Record {
            id: Uuid::new_v4(),
            project_id: project_id.to_string(),
            release_id: release_id.to_string(),
            environment: environment.to_string(),
            decision_key: key.to_owned(),
            reference,
            evaluated_at: Utc::now(),
            duration_us: 0,
            status: Status::Succeeded,
            input: input.clone(),
            output: None,
            trace: None,
            error: None,
        },
    })
}

/// An evaluation being recorded.
pub struct Pending {
    feed: &'static Feed,
    started: Instant,
    record: Record,
}

impl Pending {
    /// Records a successful answer. `body` is the engine's response
    /// (`{ performance, result, trace }`); its trace is removed unless the
    /// caller asked for it. Returns the header naming the record.
    pub fn succeeded(mut self, body: &mut Value, keep_trace: bool) -> (HeaderName, HeaderValue) {
        self.record.output = body.get("result").cloned();
        self.record.trace = match body.as_object_mut() {
            Some(map) if !keep_trace => map.remove("trace"),
            _ => body.get("trace").cloned(),
        };
        self.send(Status::Succeeded)
    }

    /// Records a failed evaluation: the error body the caller receives, and
    /// the trace up to the failing node when the engine gave one.
    pub fn failed(mut self, body: &Value, trace: Option<Value>) -> (HeaderName, HeaderValue) {
        self.record.error = Some(body.clone());
        self.record.trace = trace;
        self.send(Status::Failed)
    }

    fn send(mut self, status: Status) -> (HeaderName, HeaderValue) {
        self.record.status = status;
        self.record.duration_us =
            u64::try_from(self.started.elapsed().as_micros()).unwrap_or(u64::MAX);
        let id = self.record.id;
        self.feed.push(self.record);
        let value =
            HeaderValue::from_str(&id.to_string()).unwrap_or_else(|_| HeaderValue::from_static(""));
        (DECISION_ID_HEADER, value)
    }
}

/// The trace a node error carries (the nodes run until the failing one).
/// Error answers never show it, as upstream; the record keeps it.
pub fn error_trace(error: &EvaluationError) -> Option<Value> {
    match error {
        EvaluationError::NodeError {
            trace: Some(trace), ..
        } => Some(trace.to_value()),
        _ => None,
    }
}

/// One evaluation, as Studio's feed endpoint receives it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub id: Uuid,
    pub project_id: String,
    pub release_id: String,
    /// `staging` or `production`.
    pub environment: String,
    pub decision_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    pub evaluated_at: DateTime<Utc>,
    /// How long the evaluation took, in microseconds.
    pub duration_us: u64,
    pub status: Status,
    pub input: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Succeeded,
    Failed,
}

/// Studio's answer to a batch: records it refused (an unknown release, an
/// environment the token does not cover) are listed; the rest are stored.
#[derive(Debug, Deserialize)]
struct Answer {
    #[serde(default)]
    rejected: Vec<Rejected>,
}

#[derive(Debug, Deserialize)]
struct Rejected {
    id: Uuid,
    code: String,
}

/// The queue and the task that empties it.
pub struct Feed {
    queue: mpsc::Sender<Record>,
    stop: CancellationToken,
    worker: Mutex<Option<JoinHandle<()>>>,
    dropped: Arc<AtomicU64>,
    shutdown_timeout: Duration,
}

impl Feed {
    /// The feed `config` describes, or `None` when it is off.
    pub fn from_config(config: &DecisionLogConfig) -> anyhow::Result<Option<Self>> {
        let (url, token) = match (&config.url, &config.token) {
            (None, None) => return Ok(None),
            (Some(url), Some(token)) if !url.is_empty() && !token.is_empty() => (url, token),
            _ => anyhow::bail!("set both DECISION_LOG__URL and DECISION_LOG__TOKEN, or neither"),
        };
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            anyhow::bail!("DECISION_LOG__URL must start with http:// or https://");
        }
        if config.batch_size == 0 || config.queue_capacity == 0 {
            anyhow::bail!(
                "DECISION_LOG__BATCH_SIZE and DECISION_LOG__QUEUE_CAPACITY must be at least 1"
            );
        }
        let client = reqwest::Client::builder().timeout(config.timeout).build()?;
        Ok(Some(Self::start(
            Sender {
                client,
                url: url.clone(),
                authorization: format!("Bearer {token}"),
            },
            config,
        )))
    }

    fn start(sender: Sender, config: &DecisionLogConfig) -> Self {
        let (queue, records) = mpsc::channel(config.queue_capacity);
        let stop = CancellationToken::new();
        let worker = tokio::spawn(
            Worker {
                records,
                stop: stop.clone(),
                sender,
                batch_size: config.batch_size,
                batch_bytes: config.batch_bytes,
                flush_interval: config.flush_interval,
            }
            .run(),
        );
        Self {
            queue,
            stop,
            worker: Mutex::new(Some(worker)),
            dropped: Arc::new(AtomicU64::new(0)),
            shutdown_timeout: config.shutdown_timeout,
        }
    }

    fn push(&self, record: Record) {
        match self.queue.try_send(record) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(record)) => {
                let dropped = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
                // 1, 2, 4, 8…: visible without flooding the logs.
                if dropped.is_power_of_two() {
                    tracing::error!(
                        record = %record.id,
                        dropped,
                        "Decision log queue is full; records are being dropped"
                    );
                }
            }
            Err(mpsc::error::TrySendError::Closed(record)) => {
                tracing::error!(record = %record.id, "Decision log feed stopped; record dropped");
            }
        }
    }

    /// Stops taking records and sends what is queued, within the shutdown timeout.
    pub async fn shutdown(&self) {
        self.stop.cancel();
        let worker = self.worker.lock().ok().and_then(|mut w| w.take());
        let Some(mut worker) = worker else { return };
        if tokio::time::timeout(self.shutdown_timeout, &mut worker)
            .await
            .is_err()
        {
            worker.abort();
            tracing::error!("Decision log records still queued at shutdown were not sent");
        }
    }
}

struct Worker {
    records: mpsc::Receiver<Record>,
    stop: CancellationToken,
    sender: Sender,
    batch_size: usize,
    batch_bytes: usize,
    flush_interval: Duration,
}

impl Worker {
    async fn run(mut self) {
        loop {
            let first = tokio::select! {
                record = self.records.recv() => record,
                () = self.stop.cancelled(), if !self.records.is_closed() => {
                    // No new records; what is queued still leaves.
                    self.records.close();
                    continue;
                }
            };
            let Some(first) = first else { return };
            let mut batch = Batch::default();
            batch.push(&first);
            let deadline = tokio::time::Instant::now() + self.flush_interval;
            while batch.len < self.batch_size && batch.bytes < self.batch_bytes {
                tokio::select! {
                    record = self.records.recv() => match record {
                        Some(record) => batch.push(&record),
                        None => break,
                    },
                    () = tokio::time::sleep_until(deadline), if !self.stop.is_cancelled() => break,
                    // Stopping: take what is queued without waiting for more.
                    () = self.stop.cancelled(), if !self.records.is_closed() => self.records.close(),
                }
            }
            self.sender.deliver(batch).await;
        }
    }
}

/// Serialized records waiting to leave together.
#[derive(Default)]
struct Batch {
    records: Vec<Vec<u8>>,
    len: usize,
    bytes: usize,
}

impl Batch {
    fn push(&mut self, record: &Record) {
        match serde_json::to_vec(record) {
            Ok(bytes) => {
                self.bytes += bytes.len();
                self.len += 1;
                self.records.push(bytes);
            }
            Err(error) => {
                tracing::error!(record = %record.id, %error, "Decision record could not be serialized");
            }
        }
    }

    /// `{"records":[…]}`.
    fn body(&self) -> Vec<u8> {
        let mut body = Vec::with_capacity(self.bytes + self.records.len() + 16);
        body.extend_from_slice(br#"{"records":["#);
        for (i, record) in self.records.iter().enumerate() {
            if i > 0 {
                body.push(b',');
            }
            body.extend_from_slice(record);
        }
        body.extend_from_slice(b"]}");
        body
    }
}

struct Sender {
    client: reqwest::Client,
    url: String,
    authorization: String,
}

enum Sent {
    Delivered(Answer),
    Refused(u16),
    Retry(String),
}

impl Sender {
    /// Sends the batch until Studio takes it or refuses it.
    async fn deliver(&self, batch: Batch) {
        if batch.len == 0 {
            return;
        }
        let body = batch.body();
        let mut backoff = Duration::from_secs(1);
        loop {
            match self.send(body.clone()).await {
                Sent::Delivered(answer) => {
                    for rejected in answer.rejected {
                        tracing::warn!(
                            record = %rejected.id,
                            code = rejected.code,
                            "Studio rejected a decision record"
                        );
                    }
                    return;
                }
                Sent::Refused(status) => {
                    tracing::error!(
                        status,
                        records = batch.len,
                        "Studio refused decision records; they are dropped"
                    );
                    return;
                }
                Sent::Retry(reason) => {
                    tracing::warn!(
                        reason,
                        records = batch.len,
                        retry_in_ms = backoff.as_millis() as u64,
                        "Decision records not delivered yet"
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                }
            }
        }
    }

    async fn send(&self, body: Vec<u8>) -> Sent {
        let response = match self
            .client
            .post(&self.url)
            .header(reqwest::header::AUTHORIZATION, &self.authorization)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) if error.is_timeout() => return Sent::Retry("timeout".into()),
            Err(_) => return Sent::Retry("Studio could not be reached".into()),
        };
        let status = response.status();
        if status.is_success() {
            // A 2xx whose body cannot be read still means the records were taken.
            return Sent::Delivered(response.json().await.unwrap_or(Answer {
                rejected: Vec::new(),
            }));
        }
        // Only transport errors, 408, 429 and 5xx are worth another try.
        if status.is_server_error() || status.as_u16() == 408 || status.as_u16() == 429 {
            Sent::Retry(format!("Studio answered {}", status.as_u16()))
        } else {
            Sent::Refused(status.as_u16())
        }
    }
}

#[cfg(test)]
mod tests;
