//! Mocked-HTTP tests (`wiremock`) for the shared HTTP service: retry/backoff behaviour,
//! the `MS-CV` correlation vector on every attempt, and the credentials-never-in-`Debug`
//! guarantee.
//!
//! The 401 path is deliberately *not* retried by backoff — eviction + re-mint + a single
//! retry is a separate mechanism (see `tests/refresh.rs`); here we pin the boundary.

use std::time::Duration;

use serde_json::json;
use teams_api::{ApiError, ChatClient, RetryPolicy};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// Retry schedule with millisecond delays so the tests stay fast.
fn fast_policy(max_attempts: u32) -> RetryPolicy {
    RetryPolicy {
        max_attempts,
        base_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(4),
        factor: 2.0,
    }
}

/// Always answers with one fixed status and an empty body.
struct Always(u16);

impl Respond for Always {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        ResponseTemplate::new(self.0)
    }
}

/// A responder whose answer depends on how many requests it has seen.
struct Sequenced {
    seen: std::sync::atomic::AtomicU32,
    throttle_first: u32,
}

impl Sequenced {
    fn new(throttle_first: u32) -> Self {
        Self { seen: std::sync::atomic::AtomicU32::new(0), throttle_first }
    }
}

impl Respond for Sequenced {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        let nth = self.seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if nth < self.throttle_first {
            ResponseTemplate::new(429).insert_header("Retry-After", "0")
        } else {
            ResponseTemplate::new(200).set_body_json(json!({"conversations": []}))
        }
    }
}

async fn sent_ms_cv(server: &MockServer) -> Vec<String> {
    let requests = server.received_requests().await.expect("requests recorded");
    requests
        .iter()
        .map(|request| {
            request
                .headers
                .get("ms-cv")
                .expect("every request carries an MS-CV")
                .to_str()
                .expect("ms-cv is ascii")
                .to_owned()
        })
        .collect()
}

#[tokio::test]
async fn throttled_read_is_retried_and_recovers() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/users/ME/conversations"))
        .respond_with(Sequenced::new(1))
        .mount(&server)
        .await;

    let client = ChatClient::new(&server.uri(), "test-token")
        .expect("client")
        .with_retry_policy(fast_policy(3));

    let page = client.list_conversations(5).await.expect("read succeeds after one throttle");
    assert_eq!(page.conversations, Vec::new(), "the eventual 200 body is parsed");

    let requests = server.received_requests().await.expect("requests recorded");
    assert_eq!(requests.len(), 2, "throttle + exactly one retry");

    // Same vector across attempts (one client, one process), extension counting up.
    let vectors = sent_ms_cv(&server).await;
    let base = vectors[0].rsplit_once('.').expect("vector has an extension").0;
    assert_eq!(vectors[0], format!("{base}.1"), "first attempt is extension 1");
    assert_eq!(vectors[1], format!("{base}.2"), "retry is extension 2");
}

#[tokio::test]
async fn retry_budget_is_bounded_by_the_policy() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/users/ME/conversations"))
        .respond_with(Always(429))
        .mount(&server)
        .await;

    let client = ChatClient::new(&server.uri(), "test-token")
        .expect("client")
        .with_retry_policy(fast_policy(2));

    let err = client.list_conversations(5).await.expect_err("never succeeds");
    assert!(matches!(err, ApiError::Status { status: 429, .. }), "got: {err}");

    let requests = server.received_requests().await.expect("requests recorded");
    assert_eq!(requests.len(), 2, "two attempts total: the first plus one retry");
}

/// 401 means "this credential is dead", not "the network is flaky": backoff must not
/// burn the budget on it (the re-mint path handles it).
#[tokio::test]
async fn unauthorized_is_not_retried_by_backoff() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/users/ME/conversations"))
        .respond_with(Always(401))
        .mount(&server)
        .await;

    let client = ChatClient::new(&server.uri(), "test-token")
        .expect("client")
        .with_retry_policy(fast_policy(3));

    let err = client.list_conversations(5).await.expect_err("401 surfaces");
    assert!(matches!(err, ApiError::Status { status: 401, .. }), "got: {err}");

    let requests = server.received_requests().await.expect("requests recorded");
    assert_eq!(requests.len(), 1, "no blind retry on an invalid credential");
}

/// Same boundary for a plain 500: only throttling and front-door errors are retryable.
#[tokio::test]
async fn internal_errors_are_not_retried() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/users/ME/conversations"))
        .respond_with(Always(500))
        .mount(&server)
        .await;

    let client = ChatClient::new(&server.uri(), "test-token")
        .expect("client")
        .with_retry_policy(fast_policy(3));

    let err = client.list_conversations(5).await.expect_err("500 surfaces");
    assert!(matches!(err, ApiError::Status { status: 500, .. }), "got: {err}");

    let requests = server.received_requests().await.expect("requests recorded");
    assert_eq!(requests.len(), 1);
}

/// Credentials must never leak through `Debug` (the old derived impl printed them).
#[test]
fn debug_output_never_contains_the_credential() {
    let client = ChatClient::new("https://example.invalid", "TOP-SECRET-TOKEN").expect("client");

    let rendered = format!("{client:?}");
    assert!(!rendered.contains("TOP-SECRET"), "credential leaked into Debug: {rendered}");
    assert!(!rendered.contains("skypetoken="), "formatted token leaked: {rendered}");
    assert!(rendered.contains("Authentication"), "the header *name* stays diagnosable");
}
