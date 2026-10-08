//! Per-token request limits (DNK-22). Each access token gets `requests` per
//! `window`; past that, the Runtime answers `429` with `Retry-After` before
//! anything is evaluated. Health, version and the API docs are never limited.
//!
//! Only tokens the project accepts are counted, so the table holds one entry
//! per issued token; a wrong token goes on to its usual `401`. Tokens are kept
//! as their SHA-256, never in clear. Limits are per Runtime instance.

use crate::config::RateLimitConfig;
use crate::data::access::hash_token;
use crate::engine_ext::EngineExtension;
use crate::provider::Agent;
use axum::Json;
use axum::extract::Request;
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use dashmap::DashMap;
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Generic cell rate algorithm: each request moves the token's theoretical
/// arrival time on by `window / requests`; a request that would push it more
/// than one window ahead of now is refused. A full window's worth may arrive at
/// once, then they come back steadily.
#[derive(Clone)]
pub struct RateLimiter {
    interval: Duration,
    window: Duration,
    arrivals: Arc<DashMap<String, Instant>>,
}

impl RateLimiter {
    /// `None` when no limit is configured.
    pub fn new(config: &RateLimitConfig) -> Option<Self> {
        let requests = config.requests.filter(|requests| *requests > 0)?;
        Some(Self {
            interval: config.window / requests,
            window: config.window,
            arrivals: Arc::default(),
        })
    }

    /// `Ok` when the request may go on, or how long until it would.
    fn check(&self, key: String, now: Instant) -> Result<(), Duration> {
        let mut arrival = self.arrivals.entry(key).or_insert(now);
        let next = (*arrival).max(now) + self.interval;
        let ahead = next - now;
        if ahead > self.window {
            return Err(ahead - self.window);
        }
        *arrival = next;
        Ok(())
    }

    /// Drops the tokens that are back to a full allowance, so they cost nothing.
    fn forget_idle(&self, now: Instant) {
        self.arrivals.retain(|_, arrival| *arrival > now);
    }

    /// Forgets idle tokens once per window, for as long as the Runtime runs.
    pub fn spawn_cleanup(&self) {
        let limiter = self.clone();
        tokio::spawn(async move {
            let mut ticks = tokio::time::interval(limiter.window);
            loop {
                ticks.tick().await;
                limiter.forget_idle(Instant::now());
            }
        });
    }
}

/// The surfaces a token opens: `/api/projects/{project}/…` and `/api/rules/{project}…`.
fn limited_project(path: &str) -> Option<(&str, bool)> {
    let (rest, rules) = match path.strip_prefix("/api/projects/") {
        Some(rest) => (rest, false),
        None => (path.strip_prefix("/api/rules/")?, true),
    };
    let project = rest
        .split('/')
        .next()
        .filter(|project| !project.is_empty())?;
    Some((project, rules))
}

pub async fn limit(
    axum::Extension(limiter): axum::Extension<RateLimiter>,
    axum::Extension(agent): axum::Extension<Agent>,
    request: Request,
    next: Next,
) -> Response {
    let Some((project, rules)) = limited_project(request.uri().path()) else {
        return next.run(request).await;
    };
    let token = request
        .headers()
        .get("X-Access-Token")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let accepted = agent
        .project(project)
        .is_some_and(|project| project.engine.can_access(token));
    if !accepted {
        return next.run(request).await;
    }

    match limiter.check(hash_token(token), Instant::now()) {
        Ok(()) => next.run(request).await,
        Err(wait) => too_many_requests(wait, rules),
    }
}

/// In each surface's own error shape, with `Retry-After` in whole seconds.
fn too_many_requests(wait: Duration, rules: bool) -> Response {
    let seconds = wait.as_secs() + u64::from(wait.subsec_nanos() > 0);
    let body = if rules {
        json!({ "code": "rateLimit.exceeded", "retryAfter": seconds })
    } else {
        json!({ "message": format!("Too many requests for this access token; retry in {seconds} s") })
    };
    let mut response = (StatusCode::TOO_MANY_REQUESTS, Json(body)).into_response();
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from(seconds.max(1)));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limiter(requests: u32, window_ms: u64) -> RateLimiter {
        RateLimiter::new(&RateLimitConfig {
            requests: Some(requests),
            window: Duration::from_millis(window_ms),
        })
        .unwrap()
    }

    #[test]
    fn off_unless_configured() {
        assert!(RateLimiter::new(&RateLimitConfig::default()).is_none());
        let zero = RateLimitConfig {
            requests: Some(0),
            ..Default::default()
        };
        assert!(RateLimiter::new(&zero).is_none());
    }

    #[test]
    fn a_full_window_then_refused_until_one_comes_back() {
        let limiter = limiter(3, 3_000);
        let start = Instant::now();
        for _ in 0..3 {
            assert!(limiter.check("a".into(), start).is_ok());
        }
        let wait = limiter.check("a".into(), start).unwrap_err();
        assert_eq!(wait, Duration::from_secs(1));
        // A refused request does not count.
        assert_eq!(limiter.check("a".into(), start).unwrap_err(), wait);
        assert!(limiter.check("a".into(), start + wait).is_ok());
        assert!(limiter.check("a".into(), start + wait).is_err());
    }

    #[test]
    fn each_token_has_its_own_allowance() {
        let limiter = limiter(1, 1_000);
        let now = Instant::now();
        assert!(limiter.check("a".into(), now).is_ok());
        assert!(limiter.check("a".into(), now).is_err());
        assert!(limiter.check("b".into(), now).is_ok());
    }

    #[test]
    fn idle_tokens_are_forgotten() {
        let limiter = limiter(2, 1_000);
        let now = Instant::now();
        limiter.check("a".into(), now).unwrap();
        limiter.forget_idle(now);
        assert_eq!(limiter.arrivals.len(), 1);
        limiter.forget_idle(now + Duration::from_secs(1));
        assert!(limiter.arrivals.is_empty());
    }

    #[test]
    fn only_project_surfaces_are_limited() {
        assert_eq!(
            limited_project("/api/projects/credit/evaluate/a.json"),
            Some(("credit", false))
        );
        assert_eq!(limited_project("/api/rules/credit"), Some(("credit", true)));
        assert_eq!(
            limited_project("/api/rules/credit/evaluate/x"),
            Some(("credit", true))
        );
        assert_eq!(limited_project("/api/health"), None);
        assert_eq!(limited_project("/api/version"), None);
        assert_eq!(limited_project("/api/docs/"), None);
        assert_eq!(limited_project("/api/projects/"), None);
    }

    #[test]
    fn retry_after_is_whole_seconds_rounded_up() {
        let response = too_many_requests(Duration::from_millis(1_200), false);
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()[header::RETRY_AFTER], "2");
        let response = too_many_requests(Duration::from_millis(1), true);
        assert_eq!(response.headers()[header::RETRY_AFTER], "1");
    }
}
