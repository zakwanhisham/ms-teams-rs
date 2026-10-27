//! Deserialization tests for the conversations wire types.
//!
//! The fixture is a **sanitized** slice of a live response (2026-10-27): all names,
//! ids, URLs, topics and timestamps are `<redacted>` or zeroed, enum-like values and
//! numeric strings are kept verbatim. Never replace it with unsanitized data.

use teams_protocol::{Conversation, ConversationList};

/// Real (sanitized) traffic: must deserialize into the typed model.
#[test]
fn parses_live_conversations_fixture() {
    let raw = include_str!("fixtures/conversations.json");
    let list: ConversationList = serde_json::from_str(raw).expect("fixture must deserialize");

    // _metadata
    assert_eq!(list.metadata.total_count, Some(5));
    assert!(list.metadata.sync_state.is_some(), "sync cursor present");
    assert!(list.metadata.backward_link.is_some());
}

#[test]
fn fixture_conversations_are_fully_typed() {
    let list: ConversationList =
        serde_json::from_str(include_str!("fixtures/conversations.json")).expect("fixture");

    assert_eq!(list.conversations.len(), 2, "fixture keeps 2 of 5 items");

    let space = &list.conversations[0];
    assert_eq!(space.conversation_type, "Conversation");
    assert_eq!(space.product_thread_type(), Some("TeamsTeam"));
    assert_eq!(
        space.thread_properties.as_ref().and_then(|t| t.thread_type.as_deref()),
        Some("space")
    );
    assert!(space.version.is_some());
    assert_ne!(space.id, "");
    assert!(space.topic().is_some(), "space topic is redacted but present");

    let preview = space.last_message.as_ref().expect("lastMessage present");
    assert_eq!(preview.message_type.as_deref(), Some("RichText/Html"));
    assert_eq!(preview.type_name.as_deref(), Some("Message"));
    assert_eq!(preview.sequence_id, Some(741));
    assert!(preview.root_message_id.is_some());
    assert!(preview.id.parse::<u64>().is_ok(), "message id is numeric");

    let member = space.member_properties.as_ref().expect("memberProperties");
    assert_eq!(member.role.as_deref(), Some("User"));
    assert_eq!(member.is_reader, Some(false));

    let channel = &list.conversations[1];
    assert_eq!(channel.product_thread_type(), Some("TeamsStandardChannel"));
    assert_eq!(
        channel.thread_properties.as_ref().and_then(|t| t.thread_type.as_deref()),
        Some("topic")
    );
}

/// Unknown fields must be preserved (not dropped) so future protocol work can inspect them.
#[test]
fn unknown_fields_land_in_the_catch_all() {
    let list: ConversationList =
        serde_json::from_str(include_str!("fixtures/conversations.json")).expect("fixture");

    let channel = &list.conversations[1];
    let thread = channel.thread_properties.as_ref().expect("threadProperties");
    for key in ["spaceId", "privacy", "topicThreadTopic"] {
        assert!(thread.extra.contains_key(key), "expected {key} in catch-all");
    }
    let props = &channel.properties;
    assert!(props.extra.contains_key("isfollowed"));
    let member = channel.member_properties.as_ref().expect("memberProperties");
    assert!(member.extra.contains_key("isFollowing"));
}

/// One weird shape must never fail the whole page (AGENTS.md: degrade, don't error).
#[test]
fn tolerates_missing_blocks_and_unknown_root_fields() {
    let minimal = r#"{
        "conversations": [
            {"id": "19:abc@thread.v2", "type": "Conversation", "futureField": 42}
        ],
        "brandNewRoot": {"nested": true}
    }"#;
    let list: ConversationList = serde_json::from_str(minimal).expect("minimal page");

    assert_eq!(list.conversations.len(), 1);
    let conv = &list.conversations[0];
    assert!(conv.last_message.is_none());
    assert!(conv.thread_properties.is_none());
    assert_eq!(conv.topic(), None);
    assert_eq!(conv.extra.get("futureField").and_then(serde_json::Value::as_i64), Some(42));
    assert!(list.extra.contains_key("brandNewRoot"));
    assert!(list.metadata.sync_state.is_none());
}

/// Round-trip: what we parse must serialize back without loss (used when replaying).
#[test]
fn roundtrip_is_lossless() {
    let list: ConversationList =
        serde_json::from_str(include_str!("fixtures/conversations.json")).expect("fixture");
    let again = serde_json::to_string(&list).expect("serialize");
    let back: ConversationList = serde_json::from_str(&again).expect("reparse");
    assert_eq!(list, back);
}

/// The helpers used by the CLI summary.
#[test]
fn topic_falls_back_to_channel_topic() {
    let list: ConversationList =
        serde_json::from_str(include_str!("fixtures/conversations.json")).expect("fixture");
    let conv: &Conversation = &list.conversations[1];
    assert!(conv.topic().is_some(), "channel topic redacted but present");
}
