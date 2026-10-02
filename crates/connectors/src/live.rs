//! Real calls: timeout per attempt, bounded retries with backoff, and a
//! circuit breaker per URL so a failing service is not hammered.

use crate::{Auth, CallReport, ConnectorConfig, ConnectorError, SECRET_ENV_PREFIX};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Where secret values come from. Values never leave the handler.
pub trait Secrets: Send + Sync {
    fn get(&self, name: &str) -> Option<String>;
}

/// Secrets from the process environment: `BUREAU_KEY` is `DONKA_SECRET_BUREAU_KEY`.
#[derive(Debug, Default, Clone, Copy)]
pub struct EnvSecrets;

impl Secrets for EnvSecrets {
    fn get(&self, name: &str) -> Option<String> {
        std::env::var(format!("{SECRET_ENV_PREFIX}{name}"))
            .ok()
            .filter(|value| !value.is_empty())
    }
}

/// What the Runtime allows, whatever the node asks for.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Time per attempt when the node does not say, and the most it may ask for.
    pub default_timeout: Duration,
    pub max_timeout: Duration,
    /// Attempts after the first one when the node does not say, and the most it may ask for.
    pub default_retries: u32,
    pub max_retries: u32,
    /// Consecutive failed calls that open a URL's circuit…
    pub breaker_failures: u32,
    /// …and how long it stays open before one call is tried again.
    pub breaker_cooldown: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            default_timeout: Duration::from_secs(3),
            max_timeout: Duration::from_secs(10),
            default_retries: 1,
            max_retries: 3,
            breaker_failures: 5,
            breaker_cooldown: Duration::from_secs(30),
        }
    }
}

/// Wait before retry `n` (from 1): 100 ms, 200 ms, 400 ms… up to one second.
fn backoff(retry: u32) -> Duration {
    Duration::from_millis((100u64 << retry.saturating_sub(1).min(4)).min(1000))
}

#[derive(Debug, Default)]
struct Breaker {
    failures: u32,
    open_until: Option<Instant>,
}

pub(crate) struct Live {
    client: reqwest::Client,
    secrets: Arc<dyn Secrets>,
    limits: Limits,
    breakers: Mutex<HashMap<String, Breaker>>,
}

impl Live {
    pub(crate) fn new(limits: Limits, secrets: Arc<dyn Secrets>) -> Self {
        Self {
            client: reqwest::Client::new(),
            secrets,
            limits,
            breakers: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) async fn call(
        &self,
        config: &ConnectorConfig,
        body: Value,
    ) -> (Result<Value, ConnectorError>, CallReport) {
        let mut report = CallReport::default();
        let started = Instant::now();
        let header = match self.auth_header(&config.auth) {
            Ok(header) => header,
            Err(error) => return (Err(error), report),
        };
        if !self.allowed(&config.url) {
            return (Err(ConnectorError::CircuitOpen), report);
        }
        let timeout = config
            .timeout_ms
            .map(Duration::from_millis)
            .unwrap_or(self.limits.default_timeout)
            .min(self.limits.max_timeout);
        let retries = config
            .retries
            .unwrap_or(self.limits.default_retries)
            .min(self.limits.max_retries);

        let mut outcome = Err(ConnectorError::Unreachable);
        for attempt in 0..=retries {
            if attempt > 0 {
                tokio::time::sleep(backoff(attempt)).await;
            }
            report.attempts = attempt + 1;
            let mut request = self.client.post(&config.url).timeout(timeout).json(&body);
            if let Some((name, value)) = &header {
                request = request.header(name.as_str(), value.as_str());
            }
            let (result, retry) = match request.send().await {
                Ok(response) => {
                    let status = response.status();
                    report.status = Some(status.as_u16());
                    if status.is_success() {
                        match response.json::<Value>().await {
                            Ok(value) => (Ok(value), false),
                            Err(_) => (Err(ConnectorError::NotJson), false),
                        }
                    } else {
                        // The service refused the request itself: trying again will not help.
                        let transient = status.is_server_error()
                            || status == reqwest::StatusCode::TOO_MANY_REQUESTS
                            || status == reqwest::StatusCode::REQUEST_TIMEOUT;
                        (Err(ConnectorError::Status(status.as_u16())), transient)
                    }
                }
                Err(error) if error.is_timeout() => (
                    Err(ConnectorError::Timeout(timeout.as_millis() as u64)),
                    true,
                ),
                Err(_) => (Err(ConnectorError::Unreachable), true),
            };
            outcome = result;
            if outcome.is_ok() || !retry {
                break;
            }
        }
        report.duration_ms = started.elapsed().as_millis();
        self.record(&config.url, outcome.is_ok());
        (outcome, report)
    }

    /// The header carrying the secret, or why it cannot be built. Errors name
    /// the secret, never its value.
    fn auth_header(&self, auth: &Auth) -> Result<Option<(String, String)>, ConnectorError> {
        let (header, secret, bearer) = match auth {
            Auth::None => return Ok(None),
            Auth::Bearer { secret } => ("Authorization".to_owned(), secret, true),
            Auth::Header { header, secret } => (header.clone(), secret, false),
        };
        let value = self
            .secrets
            .get(secret)
            .ok_or_else(|| ConnectorError::MissingSecret(secret.clone()))?;
        Ok(Some((
            header,
            if bearer {
                format!("Bearer {value}")
            } else {
                value
            },
        )))
    }

    fn allowed(&self, url: &str) -> bool {
        let mut breakers = self
            .breakers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let breaker = breakers.entry(url.to_owned()).or_default();
        match breaker.open_until {
            // Half-open: once the cooldown is over, one call goes through.
            Some(until) if Instant::now() < until => false,
            Some(_) => {
                breaker.open_until = None;
                true
            }
            None => true,
        }
    }

    fn record(&self, url: &str, ok: bool) {
        let mut breakers = self
            .breakers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let breaker = breakers.entry(url.to_owned()).or_default();
        if ok {
            *breaker = Breaker::default();
            return;
        }
        breaker.failures += 1;
        if breaker.failures >= self.limits.breaker_failures {
            breaker.open_until = Some(Instant::now() + self.limits.breaker_cooldown);
            breaker.failures = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_up_to_a_second() {
        assert_eq!(backoff(1), Duration::from_millis(100));
        assert_eq!(backoff(2), Duration::from_millis(200));
        assert_eq!(backoff(3), Duration::from_millis(400));
        assert_eq!(backoff(9), Duration::from_millis(1000));
    }
}
