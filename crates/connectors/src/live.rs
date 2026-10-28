//! Real calls: timeout per attempt, bounded retries with backoff, and a
//! circuit breaker per URL so a failing service is not hammered.

use crate::{Auth, CallReport, ClientTls, ConnectorConfig, ConnectorError, SECRET_ENV_PREFIX};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Where secret values come from. Values never leave the handler.
pub trait Secrets: Send + Sync {
    fn get(&self, name: &str) -> Option<String>;
}

/// Secrets from the process environment: `BUREAU_KEY` is `DONKA_SECRET_BUREAU_KEY`, or the
/// content of the file `DONKA_SECRET_BUREAU_KEY_FILE` names (a certificate, a mounted secret),
/// without its trailing newlines.
#[derive(Debug, Default, Clone, Copy)]
pub struct EnvSecrets;

impl Secrets for EnvSecrets {
    fn get(&self, name: &str) -> Option<String> {
        let variable = format!("{SECRET_ENV_PREFIX}{name}");
        let value = match std::env::var(&variable) {
            Ok(value) => Some(value),
            Err(_) => std::env::var(format!("{variable}_FILE"))
                .ok()
                .and_then(|path| std::fs::read_to_string(path).ok())
                .map(|text| text.trim_end_matches(['\n', '\r']).to_owned()),
        };
        value.filter(|value| !value.is_empty())
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

/// A token from an OAuth2 token endpoint, and when to stop using it.
#[derive(Clone)]
struct Token {
    value: String,
    refresh_at: Option<Instant>,
}

/// Which token: one per token endpoint, client and scopes.
type TokenKey = (String, String, Vec<String>);

pub(crate) struct Live {
    /// The client for calls without a client certificate.
    client: reqwest::Client,
    /// One client per client certificate (by the names of its secrets), built at first use.
    tls_clients: Mutex<HashMap<ClientTls, reqwest::Client>>,
    tokens: Mutex<HashMap<TokenKey, Token>>,
    secrets: Arc<dyn Secrets>,
    limits: Limits,
    breakers: Mutex<HashMap<String, Breaker>>,
}

/// How a request for the service will be authenticated.
enum Credentials {
    None,
    Header(String, String),
    OAuth2 {
        key: TokenKey,
        client_secret: String,
    },
}

/// One attempt's result, and whether trying again could help.
type Attempt = (Result<Value, ConnectorError>, bool);

impl Live {
    pub(crate) fn new(limits: Limits, secrets: Arc<dyn Secrets>) -> Self {
        Self {
            client: reqwest::Client::new(),
            tls_clients: Mutex::new(HashMap::new()),
            tokens: Mutex::new(HashMap::new()),
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
        let prepared = self.credentials(&config.auth).and_then(|credentials| {
            let client = match &config.tls {
                Some(tls) => self.tls_client(tls)?,
                None => self.client.clone(),
            };
            Ok((credentials, client))
        });
        let (credentials, client) = match prepared {
            Ok(prepared) => prepared,
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
            let (result, retry) = self
                .attempt(&client, config, &credentials, &body, timeout, &mut report)
                .await;
            outcome = result;
            if outcome.is_ok() || !retry {
                break;
            }
        }
        report.duration_ms = started.elapsed().as_millis();
        self.record(&config.url, outcome.is_ok());
        (outcome, report)
    }

    /// One attempt: the token when the connector uses OAuth2, then the call. A `401` drops the
    /// token and the call is made once more with a new one.
    async fn attempt(
        &self,
        client: &reqwest::Client,
        config: &ConnectorConfig,
        credentials: &Credentials,
        body: &Value,
        timeout: Duration,
        report: &mut CallReport,
    ) -> Attempt {
        let mut renewed = false;
        loop {
            let header = match credentials {
                Credentials::None => None,
                Credentials::Header(name, value) => Some((name.clone(), value.clone())),
                Credentials::OAuth2 { key, client_secret } => {
                    match self.token(client, key, client_secret, timeout).await {
                        Ok(token) => Some(("Authorization".to_owned(), format!("Bearer {token}"))),
                        Err(failure) => return failure,
                    }
                }
            };
            let mut request = client.post(&config.url).timeout(timeout).json(body);
            if let Some((name, value)) = &header {
                request = request.header(name.as_str(), value.as_str());
            }
            let response = match request.send().await {
                Ok(response) => response,
                Err(error) if error.is_timeout() => {
                    return (
                        Err(ConnectorError::Timeout(timeout.as_millis() as u64)),
                        true,
                    );
                }
                Err(error) => return (Err(connect_error(&error)), true),
            };
            let status = response.status();
            report.status = Some(status.as_u16());
            if status == reqwest::StatusCode::UNAUTHORIZED
                && !renewed
                && let Credentials::OAuth2 { key, .. } = credentials
            {
                self.forget_token(key);
                renewed = true;
                continue;
            }
            if status.is_success() {
                return match response.json::<Value>().await {
                    Ok(value) => (Ok(value), false),
                    Err(_) => (Err(ConnectorError::NotJson), false),
                };
            }
            // The service refused the request itself: trying again will not help.
            return (
                Err(ConnectorError::Status(status.as_u16())),
                transient(status),
            );
        }
    }

    /// The token for `key`: the one kept while it is fresh, else a new one from the token
    /// endpoint (client id and secret in HTTP Basic, RFC 6749 §2.3.1). The token is never
    /// logged, traced or put in an error.
    async fn token(
        &self,
        client: &reqwest::Client,
        key: &TokenKey,
        client_secret: &str,
        timeout: Duration,
    ) -> Result<String, Attempt> {
        if let Some(token) = self.lock_tokens().get(key)
            && token.refresh_at.is_none_or(|at| Instant::now() < at)
        {
            return Ok(token.value.clone());
        }
        let (token_url, client_id, scopes) = key;
        let mut form = vec![("grant_type", "client_credentials".to_owned())];
        if !scopes.is_empty() {
            form.push(("scope", scopes.join(" ")));
        }
        let response = client
            .post(token_url)
            .timeout(timeout)
            .basic_auth(client_id, Some(client_secret))
            .header("Accept", "application/json")
            .form(&form)
            .send()
            .await;
        let response = match response {
            Ok(response) => response,
            Err(error) if error.is_timeout() => {
                return Err((
                    Err(ConnectorError::Timeout(timeout.as_millis() as u64)),
                    true,
                ));
            }
            Err(error) => {
                let error = match connect_error(&error) {
                    ConnectorError::Unreachable => ConnectorError::TokenUnreachable,
                    other => other,
                };
                return Err((Err(error), true));
            }
        };
        let status = response.status();
        if !status.is_success() {
            return Err((
                Err(ConnectorError::TokenStatus(status.as_u16())),
                transient(status),
            ));
        }
        #[derive(serde::Deserialize)]
        struct Answer {
            access_token: String,
            #[serde(default)]
            expires_in: Option<u64>,
        }
        let answer = match response.json::<Answer>().await {
            Ok(answer) if !answer.access_token.is_empty() => answer,
            _ => return Err((Err(ConnectorError::TokenInvalid), false)),
        };
        // Renewed a little before it expires, so a call never carries a token that lapses on
        // the way: a tenth of its lifetime early, thirty seconds at most.
        let refresh_at = answer.expires_in.map(|seconds| {
            let lifetime = Duration::from_secs(seconds);
            Instant::now() + lifetime - (lifetime / 10).min(Duration::from_secs(30))
        });
        self.lock_tokens().insert(
            key.clone(),
            Token {
                value: answer.access_token.clone(),
                refresh_at,
            },
        );
        Ok(answer.access_token)
    }

    fn forget_token(&self, key: &TokenKey) {
        self.lock_tokens().remove(key);
    }

    fn lock_tokens(&self) -> std::sync::MutexGuard<'_, HashMap<TokenKey, Token>> {
        self.tokens
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// How the request will be authenticated, or why it cannot be. Errors name the secret,
    /// never its value.
    fn credentials(&self, auth: &Auth) -> Result<Credentials, ConnectorError> {
        Ok(match auth {
            Auth::None => Credentials::None,
            Auth::Bearer { secret } => Credentials::Header(
                "Authorization".to_owned(),
                format!("Bearer {}", self.secret(secret)?),
            ),
            Auth::Header { header, secret } => {
                Credentials::Header(header.clone(), self.secret(secret)?)
            }
            Auth::OAuth2 {
                token_url,
                client_id,
                secret,
                scopes,
            } => Credentials::OAuth2 {
                key: (token_url.clone(), client_id.clone(), scopes.clone()),
                client_secret: self.secret(secret)?,
            },
        })
    }

    fn secret(&self, name: &str) -> Result<String, ConnectorError> {
        self.secrets
            .get(name)
            .ok_or_else(|| ConnectorError::MissingSecret(name.to_owned()))
    }

    /// The client presenting `tls`'s certificate and trusting its CA as well as the public
    /// ones. Built once per certificate: the secrets are read at the first call.
    fn tls_client(&self, tls: &ClientTls) -> Result<reqwest::Client, ConnectorError> {
        let mut clients = self
            .tls_clients
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(client) = clients.get(tls) {
            return Ok(client.clone());
        }
        let certificate = self.secret(&tls.certificate)?;
        let key = self.secret(&tls.key)?;
        let invalid =
            || ConnectorError::InvalidCertificate(format!("{} and {}", tls.certificate, tls.key));
        // A PEM bundle: the certificate (and its chain), then the private key.
        let identity = reqwest::Identity::from_pem(format!("{certificate}\n{key}").as_bytes())
            .map_err(|_| invalid())?;
        let mut builder = reqwest::Client::builder().identity(identity);
        if let Some(ca) = &tls.ca {
            let bundle = self.secret(ca)?;
            let roots = reqwest::Certificate::from_pem_bundle(bundle.as_bytes())
                .ok()
                .filter(|roots| !roots.is_empty())
                .ok_or_else(|| ConnectorError::InvalidCa(ca.clone()))?;
            for root in roots {
                builder = builder.add_root_certificate(root);
            }
        }
        let client = builder.build().map_err(|_| invalid())?;
        clients.insert(tls.clone(), client.clone());
        Ok(client)
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

/// Whether a refusal may pass on a second try: server errors, rate limits, timeouts.
fn transient(status: reqwest::StatusCode) -> bool {
    status.is_server_error()
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status == reqwest::StatusCode::REQUEST_TIMEOUT
}

/// A request that got no answer: a failed TLS handshake says why (an unknown CA, a refused
/// client certificate), anything else is unreachable. The reason holds no secret.
fn connect_error(error: &reqwest::Error) -> ConnectorError {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(current) = source {
        if let Some(tls) = current.downcast_ref::<rustls::Error>() {
            return ConnectorError::Tls(tls.to_string());
        }
        // An I/O error's `source` skips the error it wraps: step into it instead.
        source = match current.downcast_ref::<std::io::Error>() {
            Some(io) => io
                .get_ref()
                .map(|inner| inner as &(dyn std::error::Error + 'static)),
            None => current.source(),
        };
    }
    ConnectorError::Unreachable
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
