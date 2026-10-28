//! Mocked-HTTP tests (`wiremock`) for the Phase 5 chat write path: send (with the
//! `Location` id capture), edit, delete, react, unreact, isread, conversation
//! `consumptionhorizon`, and the typing indicator.
//!
//! The bodies and status codes below are the ones verified live on 2026-10-29 (see
//! `docs/endpoints.md` § Chat service). The 201 fixture is sanitized.

use std::time::Duration;

use serde_json::{Value, json};
use teams_api::{ApiError, ChatClient, RetryPolicy};
use wiremock::matchers::{body_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn fast_policy() -> RetryPolicy {
    RetryPolicy {
        max_attempts: 1,
        base_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(2),
        factor: 2.0,
    }
}

/// A client pointed at the mock server (skypetoken content irrelevant here).
fn client(server: &MockServer) -> ChatClient {
    ChatClient::new(&server.uri(), "test-token")
        .ok()
        .map(|c| c.with_retry_policy(fast_policy()))
        .expect("client built")
}

/// Send: 201 + `Location` (id is the path tail) + `{"OriginalArrivalTime": <ms>}` body —
/// the exact 201 shape verified live; fixture sanitized from the raw response.
#[tokio::test]
async fn send_message_captures_location_id_and_arrival_time() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/chat-send-201.json")).expect("fixture json");
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/users/ME/conversations/48:notifications/messages"))
        .respond_with(
            ResponseTemplate::new(201)
                .insert_header("Location", "https://apac.ng.msg.teams.microsoft.com/v1/users/ME/conversations/48:notifications/messages/1793206366212abc")
                .set_body_json(fixture),
        )
        .mount(&server)
        .await;
    let chat = client(&server);

    let sent = chat
        .send_message("48:notifications", "<div>hello</div>", 1_793_206_366_212)
        .await
        .expect("send ok");

    assert_eq!(sent.message_id.as_deref(), Some("1793206366212abc"));
    // The committed fixture is sanitized (ms epoch zeroed by the redaction policy).
    assert_eq!(sent.original_arrival_time.as_deref(), Some("0"));

    let request = &server.received_requests().await.expect("requests")[0];
    let body: Value = serde_json::from_slice(&request.body).expect("json body");
    assert_eq!(
        body,
        json!({
            "clientmessageid": "1793206366212",
            "content": "<div>hello</div>",
            "contenttype": "text",
            "messagetype": "RichText/Html",
        })
    );
    // URL-encoding keeps the conversation id as one path segment.
    assert_eq!(body.get("messagetype").and_then(Value::as_str), Some("RichText/Html"));
}

/// Edit: PUT content with `properties.skypeeditedid`, 200 empty body.
#[tokio::test]
async fn edit_message_puts_content_with_skypeeditedid() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/v1/users/ME/conversations/48:notifications/messages/1793206366212"))
        .and(body_json(json!({
            "content": "<div>hi <i>(edited)</i></div>",
            "contenttype": "text",
            "messagetype": "RichText/Html",
            "properties": { "skypeeditedid": "99100" },
        })))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let chat = client(&server);
    chat.edit_message("48:notifications", "1793206366212", 99_100, "<div>hi <i>(edited)</i></div>")
        .await
        .expect("edit ok");
}

/// Delete: 200 with a JSON `null` body.
#[tokio::test]
async fn delete_message_accepts_null_body() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/v1/users/ME/conversations/48:notifications/messages/1793206366212"))
        .respond_with(ResponseTemplate::new(200).set_body_string("null"))
        .mount(&server)
        .await;
    let chat = client(&server);
    chat.delete_message("48:notifications", "1793206366212").await.expect("delete ok");
}

/// React/unreact: the `properties?name=emotions` PUT with `{key,value}` / `{key}`.
#[tokio::test]
async fn react_and_unreact_put_emotions_property() {
    let server = MockServer::start().await;
    let prop_path = "/v1/users/ME/conversations/48:notifications/messages/1793206366212/properties";
    Mock::given(method("PUT"))
        .and(path(prop_path))
        .and(query_param("name", "emotions"))
        .and(body_json(json!({ "emotions": { "key": "like", "value": 1_800_000_000_000i64 } })))
        .respond_with(ResponseTemplate::new(200).set_body_string("200"))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path(prop_path))
        .and(query_param("name", "emotions"))
        .and(body_json(json!({ "emotions": { "key": "like" } })))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let chat = client(&server);
    chat.react("48:notifications", "1793206366212", "like", 1_800_000_000_000)
        .await
        .expect("react ok");
    chat.unreact("48:notifications", "1793206366212", "like").await.expect("unreact ok");
}

/// isread message property and the conversation `consumptionhorizon` (lowercase name).
#[tokio::test]
async fn read_receipts_put_isread_and_consumptionhorizon() {
    let server = MockServer::start().await;
    let isread =
        path("/v1/users/ME/conversations/48:notifications/messages/1793206366212/properties");
    Mock::given(method("PUT"))
        .and(isread)
        .and(query_param("name", "isread"))
        .and(body_json(json!({ "isread": "1800000000000" })))
        .respond_with(ResponseTemplate::new(200).set_body_string("200"))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/v1/users/ME/conversations/48:notifications/properties"))
        .and(query_param("name", "consumptionhorizon"))
        .and(body_json(json!({ "consumptionhorizon": "1793085256125;1793091035864;0" })))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let chat = client(&server);
    chat.mark_message_read("48:notifications", "1793206366212", 1_800_000_000_000)
        .await
        .expect("isread ok");
    chat.set_consumption_horizon("48:notifications", "1793085256125;1793091035864;0")
        .await
        .expect("consumptionhorizon ok");
}

/// Typing indicator POSTs `Control/Typing` with empty content, gets the 201 send response.
#[tokio::test]
async fn typing_indicator_posts_control_typing() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/users/ME/conversations/48:notifications/messages"))
        .and(body_json(
            json!({ "content": "", "contenttype": "text", "messagetype": "Control/Typing" }),
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "OriginalArrivalTime": 0 })))
        .mount(&server)
        .await;
    let chat = client(&server);
    chat.send_typing("48:notifications").await.expect("typing ok");
}

/// The `Location`-id extraction helper: absolute URL, bare id, and degenerate forms.
#[test]
fn message_id_from_location_takes_the_path_tail() {
    use teams_api::__test_only_message_id_from_location as id_of;
    assert_eq!(
        id_of(
            "https://apac.ng.msg.teams.microsoft.com/v1/users/ME/conversations/48:notifications/messages/1793206366213"
        ),
        Some("1793206366213")
    );
    assert_eq!(id_of("1793206366213"), Some("1793206366213"));
    assert_eq!(id_of(""), None);
    assert_eq!(id_of("https://example.com/"), None);
}

/// 400 error surfaces as [`ApiError::Status`] (no retry for 400s).
#[tokio::test]
async fn rejected_property_write_is_an_error_status() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/v1/users/ME/conversations/48:notifications/messages/1793206366212/properties"))
        .and(query_param("name", "emotions"))
        .respond_with(ResponseTemplate::new(400).set_body_json(
            json!({ "errorCode": 201, "message": "Invalid format of emotions property" }),
        ))
        .mount(&server)
        .await;
    let chat = client(&server);
    match chat.react("48:notifications", "1793206366212", "like", 0).await {
        Err(ApiError::Status { status: 400, .. }) => {}
        other => panic!("expected 400 status error, got {other:?}"),
    }
}
