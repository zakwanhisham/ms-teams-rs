//! Mocked-HTTP tests (`wiremock`) for 401 recovery: evict → re-mint → replay exactly once.
//!
//! The refresher is faked here (a real one would redeem a refresh token against the
//! identity platform); what is under test is the HTTP layer's contract:
//!
//! - a 401 with a refresher attached swaps in the fresh credential and replays;
//! - at most **one** replay per logical request, even if the service keeps answering 401;
//! - a failing re-mint surfaces immediately, without a second wire request;
//! - the auth header name differs per service (`Authentication` vs `Authorization`)
//!   but the value is replaced the same way.
//!
//! Substrate's client is not exercised here: its suggestions URL is the documented
//! constant `substrate.office.com` and cannot be pointed at the mock server; the replay
//! path itself lives in the shared `HttpService`, which the chat and roster tests cover.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::json;
use teams_api::{ApiError, ChatClient, RefreshFuture, RosterClient, TokenRefresher};
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// Refresher that hands out one fixed replacement value and counts its calls.
#[derive(Debug)]
struct FixedRefresher {
    calls: Arc<AtomicU32>,
    /// Full header value to install, e.g. `skypetoken=fresh`.
    value: String,
    /// Simulate a failed evict → re-mint (signed out, FRT revoked, …).
    fail: bool,
}

impl FixedRefresher {
    fn calls(counter: &Arc<AtomicU32>, value: &str) -> Self {
        Self { calls: Arc::clone(counter), value: value.to_owned(), fail: false }
    }

    fn failing(counter: &Arc<AtomicU32>) -> Self {
        Self { calls: Arc::clone(counter), value: String::new(), fail: true }
    }
}

impl TokenRefresher for FixedRefresher {
    fn refresh(&self) -> RefreshFuture<'_> {
        let calls = &self.calls;
        let value = self.value.clone();
        let fail = self.fail;
        Box::pin(async move {
            calls.fetch_add(1, Ordering::SeqCst);
            if fail { Err(ApiError::Config("re-mint failed".into())) } else { Ok(value) }
        })
    }
}

fn header_of(request: &Request, name: &str) -> String {
    request.headers.get(name).and_then(|value| value.to_str().ok()).unwrap_or_default().to_owned()
}

/// Answers 401 while the caller still sends `stale`, 200 once `fresh` arrives.
struct StaleUntilFresh {
    stale: &'static str,
    fresh: &'static str,
    /// Body of the eventual 200.
    ok_body: serde_json::Value,
}

impl Respond for StaleUntilFresh {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let auth = format!(
            "{} {}",
            header_of(request, "authentication"),
            header_of(request, "authorization")
        );
        if auth.contains(self.fresh) {
            ResponseTemplate::new(200).set_body_json(self.ok_body.clone())
        } else {
            assert!(
                auth.contains(self.stale),
                "first attempt must still carry the stale credential, got: {auth}"
            );
            ResponseTemplate::new(401).set_body_json(json!({"errorCode": 911}))
        }
    }
}

#[tokio::test]
async fn rejected_credential_is_re_minted_and_the_read_replayed() {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::method("GET"))
        .and(path("/v1/users/ME/conversations"))
        .respond_with(StaleUntilFresh {
            stale: "stale-token",
            fresh: "fresh-token",
            ok_body: json!({"conversations": []}),
        })
        .mount(&server)
        .await;

    let calls = Arc::new(AtomicU32::new(0));
    let client = ChatClient::new(&server.uri(), "stale-token")
        .expect("client")
        .with_refresher(FixedRefresher::calls(&calls, "skypetoken=fresh-token"));

    let page = client.list_conversations(5).await.expect("recovers after one re-mint");
    assert_eq!(page.conversations, Vec::new(), "the replayed 200 body is parsed");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "refresher ran exactly once");

    let requests = server.received_requests().await.expect("requests recorded");
    assert_eq!(requests.len(), 2, "original attempt + one replay");
    assert!(header_of(&requests[0], "authentication").contains("stale-token"));
    assert!(header_of(&requests[1], "authentication").contains("fresh-token"));
    assert_eq!(
        header_of(&requests[1], "ms-cv"),
        format!("{}.2", header_of(&requests[0], "ms-cv").rsplit_once('.').unwrap().0),
        "the replay is a new attempt on the same correlation vector"
    );
}

/// A service that rejects the *fresh* credential too must get exactly one replay, then
/// the error — never a refresh loop.
#[tokio::test]
async fn replay_happens_at_most_once_even_when_the_service_keeps_refusing() {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::method("GET"))
        .and(path("/v1/users/ME/conversations"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let calls = Arc::new(AtomicU32::new(0));
    let client = ChatClient::new(&server.uri(), "stale-token")
        .expect("client")
        .with_refresher(FixedRefresher::calls(&calls, "skypetoken=fresh-token"));

    let err = client.list_conversations(5).await.expect_err("still unauthorized");
    assert!(matches!(err, ApiError::Status { status: 401, .. }), "got: {err}");

    let requests = server.received_requests().await.expect("requests recorded");
    assert_eq!(requests.len(), 2, "stale attempt + exactly one replay");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "the second 401 does not re-mint again");
}

/// When re-minting itself fails the caller sees why — and no further request is sent.
#[tokio::test]
async fn a_failing_re_mint_surfaces_without_a_second_request() {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::method("GET"))
        .and(path("/v1/users/ME/conversations"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let calls = Arc::new(AtomicU32::new(0));
    let client = ChatClient::new(&server.uri(), "stale-token")
        .expect("client")
        .with_refresher(FixedRefresher::failing(&calls));

    let err = client.list_conversations(5).await.expect_err("re-mint fails");
    assert!(
        matches!(&err, ApiError::Config(message) if message.contains("re-mint failed")),
        "got: {err}"
    );

    let requests = server.received_requests().await.expect("requests recorded");
    assert_eq!(requests.len(), 1, "the request is not replayed with a dead credential");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

/// Bearer services go through the same loop with a different header name and value shape.
#[tokio::test]
async fn roster_replays_with_a_fresh_bearer() {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::method("GET"))
        .and(path("/api/v1/teams/users/me/updates"))
        .respond_with(StaleUntilFresh {
            stale: "stale-bearer",
            fresh: "fresh-bearer",
            ok_body: json!({}),
        })
        .mount(&server)
        .await;

    let calls = Arc::new(AtomicU32::new(0));
    let client = RosterClient::new(&server.uri(), "stale-bearer")
        .expect("client")
        .with_refresher(FixedRefresher::calls(&calls, "Bearer fresh-bearer"));

    let roster = client.roster().await.expect("recovers after one re-mint");
    assert_eq!(roster.teams, [] as [teams_protocol::Team; 0], "empty body parses");
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let requests = server.received_requests().await.expect("requests recorded");
    assert_eq!(requests.len(), 2);
    assert!(header_of(&requests[0], "authorization").contains("Bearer stale-bearer"));
    assert!(header_of(&requests[1], "authorization").contains("Bearer fresh-bearer"));
}

/// 401s must not be confused with throttling: a client *without* a refresher never
/// replays (its caller is expected to surface the 401 — regression pin against the
/// backoff loop swallowing the status).
#[tokio::test]
async fn without_a_refresher_a_401_stays_a_single_attempt() {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::method("GET"))
        .and(path("/v1/users/ME/conversations"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let client = ChatClient::new(&server.uri(), "stale-token").expect("client");
    let err = client.list_conversations(5).await.expect_err("401 surfaces");
    assert!(matches!(err, ApiError::Status { status: 401, .. }), "got: {err}");

    let requests = server.received_requests().await.expect("requests recorded");
    assert_eq!(requests.len(), 1);
}
