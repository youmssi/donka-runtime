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
