//! Mocked-HTTP tests (`wiremock`) for the device-code flow and refresh redemption.
//!
//! The endpoint shapes come from `docs/auth.md` ("Endpoint notes"); what is under test
//! here is the *behaviour* the live login relied on:
//!
//! - `devicecode` sends `client_id` + `scope` and parses the issued code;
//! - a rejected scope surfaces as `AuthError::Identity` with the AADSTS description;
//! - polling walks `authorization_pending` → `slow_down` (+5 s backoff) → success and
//!   stamps the tokens locally;
//! - decline/expiry end the poll instead of looping;
//! - the refresh grant posts the right form and parses the new token set.
//!
//! The poll loop sleeps a **minimum 5 s** before each attempt (defensive floor on the
//! service's `interval`), so those tests take real time — they run in parallel with each
//! other, and the backoff assertion measures the elapsed time directly.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use serde_json::json;
use teams_auth::{AuthError, DeviceCodeClient, DeviceCodeRequest, FOCI_CLIENT_ID};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// The scope verified accepted for the FOCI client (see `docs/auth.md`).
const SCOPE: &str = "openid offline_access https://api.spaces.skype.com/.default";

/// `urn:ietf:params:oauth:grant-type:device_code` — spelled out for form assertions.
const DEVICE_CODE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// Issued-code response as the identity platform returns it.
fn device_code_body() -> serde_json::Value {
    json!({
        "device_code": "device-blob",
        "user_code": "ABC123DEF",
        "verification_uri": "https://microsoft.com/devicelogin",
        "verification_uri_complete": "https://microsoft.com/devicelogin?code=ABC123DEF",
        "expires_in": 900,
        "interval": 5,
        "message": "To sign in, use a web browser to open the page and enter the code."
    })
}

/// Successful token response (access + family refresh token).
fn token_body() -> serde_json::Value {
    json!({
        "access_token": "access.jwt.value",
        "refresh_token": "frt.value",
        "token_type": "Bearer",
        "expires_in": 3600,
        "scope": SCOPE,
        "id_token": "id.jwt.value"
    })
}

/// Decoded `application/x-www-form-urlencoded` body of a recorded request.
fn form_of(request: &Request) -> BTreeMap<String, String> {
    url::form_urlencoded::parse(&request.body)
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect()
}

/// Client pointed at `server`, production defaults otherwise.
fn client(server: &MockServer) -> DeviceCodeClient {
    DeviceCodeClient::new("common", FOCI_CLIENT_ID).with_login_base(server.uri())
}

/// Issue a code against the mounted mock (the devicecode test's own round trip).
async fn issued_code(server: &MockServer) -> DeviceCodeRequest {
    client(server).request_code(SCOPE).await.expect("device code issued")
}

/// The same issued code, as a poll test carries it — the poll tests mock only `/token`,
/// so they must not need a `devicecode` round trip first.
fn code_request() -> DeviceCodeRequest {
    serde_json::from_value(device_code_body()).expect("fixture parses")
}

#[tokio::test]
async fn request_code_posts_client_and_scope_and_parses_the_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/common/oauth2/v2.0/devicecode"))
        .respond_with(ResponseTemplate::new(200).set_body_json(device_code_body()))
        .mount(&server)
        .await;

    let request = issued_code(&server).await;

    assert_eq!(request.device_code, "device-blob");
    assert_eq!(request.user_code, "ABC123DEF");
    assert_eq!(request.interval, 5);
    assert_eq!(request.expires_in, 900);
    // The terminal hint prefers the pre-filled URL when the service provides one.
    assert_eq!(request.login_hint(), "https://microsoft.com/devicelogin?code=ABC123DEF");

    let wire = server.received_requests().await.expect("requests recorded");
    assert_eq!(wire.len(), 1, "exactly one devicecode call");
    let form = form_of(&wire[0]);
    assert_eq!(form.get("client_id").map(String::as_str), Some(FOCI_CLIENT_ID));
    assert_eq!(form.get("scope").map(String::as_str), Some(SCOPE));
}

#[tokio::test]
async fn a_rejected_scope_surfaces_the_identity_error_with_its_aadsts_text() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/common/oauth2/v2.0/devicecode"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": "invalid_grant",
            "error_description": "AADSTS7000218: The request body must contain the \
                following parameter: 'client_id'.",
            "trace_id": "00000000-0000-0000-0000-000000000000",
            "correlation_id": "00000000-0000-0000-0000-000000000000"
        })))
        .mount(&server)
        .await;

    let err = client(&server)
        .request_code("https://graph.microsoft.com/.default")
        .await
        .expect_err("graph scope is rejected for this client");

    let AuthError::Identity { status, error, description } = err else {
        panic!("expected AuthError::Identity, got {err:?}");
    };
    assert_eq!(status, 400);
    assert_eq!(error, "invalid_grant");
    assert!(description.contains("AADSTS7000218"), "AADSTS text is kept: {description}");
}

/// Answers `authorization_pending`, then `slow_down`, then success — one response per
/// call, so the poll has to walk the real state machine.
struct PendingThenSlowDownThenToken {
    calls: Arc<AtomicU32>,
}

impl Respond for PendingThenSlowDownThenToken {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        match self.calls.fetch_add(1, Ordering::SeqCst) {
            0 => ResponseTemplate::new(400).set_body_json(
                json!({"error": "authorization_pending", "error_description": "AADSTS50014"}),
            ),
            1 => ResponseTemplate::new(400)
                .set_body_json(json!({"error": "slow_down", "error_description": "AADSTS50014"})),
            _ => ResponseTemplate::new(200).set_body_json(token_body()),
        }
    }
}

#[tokio::test]
async fn polling_walks_pending_slow_down_and_success_with_a_growing_interval() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/common/oauth2/v2.0/token"))
        .respond_with(PendingThenSlowDownThenToken { calls: Arc::new(AtomicU32::new(0)) })
        .mount(&server)
        .await;

    let started = std::time::Instant::now();
    let request = code_request();
    let tokens = client(&server).poll_for_token(&request).await.expect("approval arrives");

    assert_eq!(tokens.access_token, "access.jwt.value");
    assert_eq!(tokens.refresh_token.as_deref(), Some("frt.value"));
    assert_eq!(tokens.token_type, "Bearer");
    assert!(tokens.obtained_at.is_some(), "stamped locally after the wire response");
    assert!(!tokens.is_expired(300));

    // sleep(5) → pending, sleep(5) → slow_down, sleep(5+5) → success = 20 s minimum.
    // The slow_down step is what makes this ≥ 20 s; without the backoff it would be 15 s.
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_secs(20),
        "slow_down must grow the interval by 5 s, poll took {elapsed:?}"
    );

    let wire = server.received_requests().await.expect("requests recorded");
    assert_eq!(wire.len(), 3, "pending, slow_down, success");
    for request in &wire {
        let form = form_of(request);
        assert_eq!(form.get("grant_type").map(String::as_str), Some(DEVICE_CODE_GRANT));
        assert_eq!(form.get("device_code").map(String::as_str), Some("device-blob"));
        assert_eq!(form.get("client_id").map(String::as_str), Some(FOCI_CLIENT_ID));
    }
}

#[tokio::test]
async fn declining_the_request_ends_the_poll() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/common/oauth2/v2.0/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": "authorization_declined",
            "error_description": "AADSTS70000: The user denied consent."
        })))
        .mount(&server)
        .await;

    let request = code_request();
    let err = client(&server).poll_for_token(&request).await.expect_err("user declined");

    assert!(
        matches!(&err, AuthError::DeviceCode(msg) if msg.contains("declined")),
        "expected a terminal device-code error, got {err:?}"
    );
    assert_eq!(
        server.received_requests().await.expect("requests recorded").len(),
        1,
        "one poll attempt, then the flow stops"
    );
}

#[tokio::test]
async fn an_expired_code_ends_the_poll() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/common/oauth2/v2.0/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": "expired_token",
            "error_description": "AADSTS700003: The device code has expired."
        })))
        .mount(&server)
        .await;

    let request = code_request();
    let err = client(&server).poll_for_token(&request).await.expect_err("code expired");

    assert!(
        matches!(&err, AuthError::DeviceCode(msg) if msg.contains("expired")),
        "expected expiry, got {err:?}"
    );
}

#[tokio::test]
async fn a_deadline_passed_before_approval_never_hits_the_network() {
    let server = MockServer::start().await;
    // No mock mounted: any request would be a 404 and fail the assertion below.
    let request = DeviceCodeRequest {
        device_code: "device-blob".into(),
        user_code: "ABC123DEF".into(),
        verification_uri: "https://microsoft.com/devicelogin".into(),
        verification_uri_complete: None,
        expires_in: 0,
        interval: 5,
        message: String::new(),
    };

    let err = client(&server).poll_for_token(&request).await.expect_err("already expired");

    assert!(
        matches!(&err, AuthError::DeviceCode(msg) if msg.contains("expired")),
        "expected expiry, got {err:?}"
    );
    assert!(
        server.received_requests().await.expect("requests recorded").is_empty(),
        "an expired code must not be polled"
    );
}

#[tokio::test]
async fn the_refresh_grant_posts_the_token_set_and_stamps_it() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/common/oauth2/v2.0/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(token_body()))
        .mount(&server)
        .await;

    let tokens =
        client(&server).refresh("frt.value", SCOPE).await.expect("family refresh token redeems");

    assert_eq!(tokens.access_token, "access.jwt.value");
    assert!(tokens.obtained_at.is_some(), "stamped locally");
    assert!(!tokens.is_expired(300));

    let wire = server.received_requests().await.expect("requests recorded");
    assert_eq!(wire.len(), 1);
    let form = form_of(&wire[0]);
    assert_eq!(form.get("grant_type").map(String::as_str), Some("refresh_token"));
    assert_eq!(form.get("refresh_token").map(String::as_str), Some("frt.value"));
    assert_eq!(form.get("scope").map(String::as_str), Some(SCOPE));
}

#[tokio::test]
async fn a_rejected_refresh_token_surfaces_as_an_identity_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/common/oauth2/v2.0/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": "invalid_grant",
            "error_description": "AADSTS70000: The refresh token has been revoked."
        })))
        .mount(&server)
        .await;

    let err = client(&server)
        .refresh("revoked", SCOPE)
        .await
        .expect_err("revoked refresh token must not mint");

    let AuthError::Identity { status, error, description } = err else {
        panic!("expected AuthError::Identity, got {err:?}");
    };
    assert_eq!(status, 400);
    assert_eq!(error, "invalid_grant");
    assert!(description.contains("revoked"), "{description}");
}
