//! `TeamEvent` decoding tests: each branch exercised with a sanitized exemplar of the
//! wire message shape (source: committed traces + live read-backs, 2026-10-27/29).

use serde_json::{Value, json};
use teams_realtime::TeamEvent;

fn decode(value: Value) -> TeamEvent {
    #[allow(clippy::needless_pass_by_value)] // tests take Json values by value
    fn inner(value: Value) -> TeamEvent {
        TeamEvent::from_message_json(&value)
    }
    inner(value)
}

/// The normal bubble: `Text` body.
#[test]
fn plain_text_message_is_message_created() {
    let event = decode(json!({
        "type": "Message", "id": "1793085256125", "clientmessageid": "1793085256100",
        "composetime": "2026-10-27T04:34:16.125Z", "originalarrivaltime": "2026-10-27T04:34:16.125Z",
        "conversationid": "48:notifications", "messagetype": "Text", "contenttype": "text",
        "content": "hello", "sequenceId": 10, "version": "1793085256125",
        "from": "8:orgid:REDACTED",
    }));
    match event {
        TeamEvent::MessageCreated { conversation_id, message } => {
            assert_eq!(conversation_id, "48:notifications");
            assert_eq!(message.content.as_deref(), Some("hello"));
        }
        other => panic!("expected MessageCreated, got {other:?}"),
    }
}

/// `RichText` with the reply-pointing `rootMessageId` still decodes as created.
#[test]
fn richtext_message_is_message_created() {
    let event = decode(json!({
        "type": "Message", "id": "1793085256126", "messagetype": "RichText/Html",
        "contenttype": "text", "content": "<div>hi</div>",
        "conversationid": "19:orgid-REDACTED@thread.tacv2;messageid=1793085256125",
        "rootMessageId": "1793085256125",
    }));
    match event {
        TeamEvent::MessageCreated { conversation_id, message } => {
            assert_eq!(conversation_id, "19:orgid-REDACTED@thread.tacv2;messageid=1793085256125");
            assert_eq!(message.root_message_id.as_deref(), Some("1793085256125"));
        }
        other => panic!("expected MessageCreated, got {other:?}"),
    }
}

/// The edit marker classifies as edited and carries the marker id.
#[test]
fn skypeeditedid_is_message_edited() {
    let event = decode(json!({
        "type": "Message", "id": "1793085256125", "messagetype": "RichText/Html",
        "contenttype": "text", "content": "<div>hi <i>(edited)</i></div>",
        "conversationid": "48:notifications", "skypeeditedid": "1793085900000",
    }));
    match event {
        TeamEvent::MessageEdited { message, edit_client_message_id, .. } => {
            assert_eq!(edit_client_message_id, "1793085900000");
            assert_eq!(message.content.as_deref(), Some("<div>hi <i>(edited)</i></div>"));
        }
        other => panic!("expected MessageEdited, got {other:?}"),
    }
}

/// Delete bookkeeping rides the property bag.
#[test]
fn deletetime_is_message_deleted() {
    let event = decode(json!({
        "type": "Message", "id": "1793085256125", "messagetype": "RichText/Html",
        "content": "<div>…</div>", "conversationid": "48:notifications",
        "properties": { "deletetime": "1793089000000" },
    }));
    match event {
        TeamEvent::MessageDeleted { message, .. } => {
            let del = message.properties.extra.get("deletetime").expect("deletetime kept");
            assert_eq!(del, &json!("1793089000000"));
        }
        other => panic!("expected MessageDeleted, got {other:?}"),
    }
}

/// Reactions: the `{key, users:[{mri,time,value}]}` shape (read-path verified).
#[test]
fn emotions_property_is_reaction_changed() {
    let event = decode(json!({
        "type": "Message", "id": "1793206976854", "messagetype": "RichText/Html",
        "contenttype": "text", "content": "<div>probe</div>",
        "conversationid": "48:notifications",
        "properties": {
            "emotions": [
                { "key": "like", "users": [ { "mri": "8:orgid:REDACTED", "time": 1_793_207_000_000_i64, "value": "1" } ] }
            ],
        },
    }));
    match event {
        TeamEvent::ReactionChanged { emotions, .. } => {
            assert_eq!(emotions[0]["key"], "like");
            assert_eq!(emotions[0]["users"][0]["time"], 1_793_207_000_000_i64);
        }
        other => panic!("expected ReactionChanged, got {other:?}"),
    }
}

/// Typing: `Control/Typing`.
#[test]
fn control_typing_is_typing() {
    let event = decode(json!({
        "type": "Message", "id": "", "messagetype": "Control/Typing",
        "contenttype": "text", "content": "", "conversationid": "48:notifications",
    }));
    match event {
        TeamEvent::Typing { conversation_id, .. } => {
            assert_eq!(conversation_id, "48:notifications");
        }
        other => panic!("expected Typing, got {other:?}"),
    }
}

/// Call logs: `Event/Call` bodies (shape verified live 2026-10-28).
#[test]
fn event_call_is_call_activity() {
    let event = decode(json!({
        "type": "Message", "id": "1793100000000", "messagetype": "Event/Call",
        "contenttype": "application/user+xml",
        "content": "<ended/><partlist alt='' count='1'><part identity='8:orgid:REDACTED'><name>8:orgid:REDACTED</name><duration>31</duration></part></partlist>",
        "conversationid": "48:notifications", "skypeguid": "REDACTED-GUID",
    }));
    match event {
        TeamEvent::CallActivity { message, .. } => {
            assert_eq!(message.message_type.as_deref(), Some("Event/Call"));
        }
        other => panic!("expected CallActivity, got {other:?}"),
    }
}

/// Thread activity → conversation updated.
#[test]
fn thread_activity_is_conversation_updated() {
    let event = decode(json!({
        "type": "Message", "id": "1793300000000", "messagetype": "ThreadActivity/AddMember",
        "content": "<addmember><ellipse xlink:href='8:orgid:REDACTED'/></addmember>",
        "conversationid": "19:orgid-REDACTED@thread.tacv2",
    }));
    match event {
        TeamEvent::ConversationUpdated { conversation_id, .. } => {
            assert_eq!(conversation_id, "19:orgid-REDACTED@thread.tacv2");
        }
        other => panic!("expected ConversationUpdated, got {other:?}"),
    }
}

/// An unrecognized message shape stays prominent in `Other`.
#[test]
fn unknown_shape_is_other() {
    let event = decode(json!({ "type": "Message", "messagetype": "Event/Calendar/…",
                                "content": "Something new", "conversationid": "48:notifications" }));
    match event {
        TeamEvent::Other(v) => assert_eq!(v["messagetype"], "Event/Calendar/…"),
        other => panic!("expected Other, got {other:?}"),
    }
}

/// Absent/broken fields degrade to `Other`, never panic (`properties` missing entirely).
#[test]
fn empty_object_is_other() {
    match decode(json!({})) {
        TeamEvent::Other(_) => {}
        other => panic!("expected Other for empty object, got {other:?}"),
    }
}

/// The full page envelope is not an event — decoding a page stays `Other`.
#[test]
fn page_envelope_is_other() {
    match decode(json!({ "_metadata": {}, "messages": [], "tenantId": null })) {
        TeamEvent::Other(_) => {}
        other => panic!("expected Other for page envelope, got {other:?}"),
    }
}
