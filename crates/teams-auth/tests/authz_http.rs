//! Mocked-HTTP tests (`wiremock`) for the authz exchange (skypetoken + region map).
//!
//! The two response shapes come from the live login (`docs/auth.md`); what is under test
//! is the contract `ChatClient` and the 401 recovery depend on:
//!
//! - the bearer token goes out as `Authorization: Bearer …` with an empty JSON body;
//! - enterprise and consumer shapes both yield a usable `AuthzSession`;
//! - a non-2xx becomes `AuthError::Authz` (status + truncated body), a body without a
//!   skypetoken becomes `AuthError::AuthzShape`.

use serde_json::json;
use teams_auth::{AuthError, AuthzClient};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Enterprise shape: `tokens.skypeToken` as a plain string (what the live call returned).
fn enterprise_body() -> serde_json::Value {
    json!({
        "tokens": {
            "skypeToken": "skype.jwt.value",
            "expiresIn": 86400
        },
        "region": "apac",
        "regionGtms": {
            "chatService": "https://apac.ng.msg.teams.microsoft.com",
            "presence": {"url": "https://presence.example.test"},
            "substrate": {"url": "https://substrate.office.com"}
        },
        "expiresIn": 86400
    })
}

/// Consumer shape: `skypeToken` as an object carrying `skypetoken`.
fn consumer_body() -> serde_json::Value {
    json!({
        "skypeToken": {"skypetoken": "consumer.jwt.value", "expiresIn": 86400},
        "region": "NA",
        "regionGtms": {}
    })
}

fn client(server: &MockServer) -> AuthzClient {
    AuthzClient::new().with_endpoint(format!("{}/v1.0/authz", server.uri()))
}

async fn mount(server: &MockServer, body: serde_json::Value) {
    Mock::given(method("POST"))
        .and(path("/v1.0/authz"))
        .and(header("authorization", "Bearer spaces.access.token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

#[tokio::test]
async fn enterprise_shape_yields_a_usable_session() {
    let server = MockServer::start().await;
    mount(&server, enterprise_body()).await;

    let session =
        client(&server).exchange("spaces.access.token").await.expect("enterprise shape parses");

    assert_eq!(session.skype_token, "skype.jwt.value");
    assert_eq!(session.region.as_deref(), Some("apac"));
    assert_eq!(session.expires_in, Some(86400));
    assert!(session.obtained_at.is_some(), "stamped locally after the exchange");
    assert!(!session.is_expired(teams_auth::EXPIRY_SKEW_SECS));
    // The map is what `ChatClient::from_session` builds its base URLs from.
    assert_eq!(session.host("chatService"), Some("https://apac.ng.msg.teams.microsoft.com"));
    assert_eq!(session.host("presence"), Some("https://presence.example.test"));
    assert_eq!(session.service_names(), vec!["chatService", "presence", "substrate"]);

    let wire = server.received_requests().await.expect("requests recorded");
    assert_eq!(wire.len(), 1, "exactly one authz call");
    assert_eq!(wire[0].body, b"{}", "the exchange posts an empty JSON object");
    let content_type = wire[0]
        .headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    assert!(content_type.contains("application/json"), "got: {content_type}");
}

#[tokio::test]
async fn consumer_shape_yields_a_usable_session() {
    let server = MockServer::start().await;
    mount(&server, consumer_body()).await;

    let session =
        client(&server).exchange("spaces.access.token").await.expect("consumer shape parses");

    assert_eq!(session.skype_token, "consumer.jwt.value");
    assert_eq!(session.region.as_deref(), Some("NA"));
    assert_eq!(session.expires_in, Some(86400));
    assert!(session.region_gtms.is_empty());
}

#[tokio::test]
async fn a_non_2xx_becomes_an_authz_error_with_status_and_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1.0/authz"))
        .respond_with(ResponseTemplate::new(401).set_body_raw(
            r#"{"errorCode":911,"message":"Authentication failed."}"#,
            "application/json",
        ))
        .mount(&server)
        .await;

    let err = client(&server).exchange("stale.token").await.expect_err("401 must not parse");

    let AuthError::Authz { status, body } = err else {
        panic!("expected AuthError::Authz, got {err:?}");
    };
    assert_eq!(status, 401);
    assert!(body.contains("Authentication failed"), "diagnostic body kept: {body}");
}

#[tokio::test]
async fn a_body_without_a_skypetoken_is_a_shape_error() {
    let server = MockServer::start().await;
    mount(&server, json!({"region": "apac", "regionGtms": {}})).await;

    let err = client(&server).exchange("spaces.access.token").await.expect_err("no token");

    assert!(
        matches!(&err, AuthError::AuthzShape(body) if body.contains("\"region\"")),
        "expected AuthError::AuthzShape echoing the body, got {err:?}"
    );
}
