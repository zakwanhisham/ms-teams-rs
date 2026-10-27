//! Deserialization tests for the messages wire types.
//!
//! The fixture is a **sanitized** slice of live traffic (2026-10-27) covering three wire
//! shapes: a rich message with stringified mentions + reactions, a `ThreadActivity/*`
//! system message whose XML keeps its tag structure but has every identity and
//! timestamp zeroed, and an edited/deleted-marked message. Names, ids, URLs and topics
//! are `<redacted>` or zeroed. Never replace it with unsanitized data.

use teams_protocol::{Message, MessagePage};

const FIXTURE: &str = include_str!("fixtures/messages.json");
const THREAD_FIXTURE: &str = include_str!("fixtures/thread.json");
const SINGLE_FIXTURE: &str = include_str!("fixtures/message.json");

fn page() -> MessagePage {
    serde_json::from_str(FIXTURE).expect("messages fixture must deserialize")
}

#[test]
fn parses_live_messages_fixture() {
    let page = page();
    assert_eq!(page.messages.len(), 3);
    assert_eq!(page.tenant_id.as_deref(), Some("<redacted>"));

    let meta = &page.metadata;
    assert!(meta.sync_state.is_some(), "paging cursor present");
    assert!(meta.backward_link.is_some());
    assert!(meta.forward_link.is_none(), "messages pages omit forwardLink");
    assert_eq!(meta.last_complete_segment_end_time, Some(0));
    assert_eq!(meta.last_complete_segment_start_time, Some(0));
    assert_eq!(meta.total_count, None, "messages pages omit totalCount");
}

#[test]
fn rich_message_is_fully_typed() {
    let page = page();
    let msg: &Message = &page.messages[0];

    assert_eq!(msg.message_type.as_deref(), Some("RichText/Html"));
    assert_eq!(msg.content_type.as_deref(), Some("Text"));
    assert_eq!(msg.type_name.as_deref(), Some("Message"));
    assert_eq!(msg.post_type.as_deref(), Some("Standard"));
    assert_eq!(msg.sequence_id, Some(730));
    assert!(msg.id.parse::<u64>().is_ok(), "message ids are numeric strings");
    assert_eq!(msg.root_message_id.as_deref(), Some(msg.id.as_str()), "root, not a reply");
    assert!(!msg.is_system());
    assert!(!msg.is_edited());
    assert!(!msg.is_reply());
    assert!(msg.content.is_some(), "body present (redacted)");

    let props = &msg.properties;
    assert_eq!(props.format_variant.as_deref(), Some("TEAMS"));
    assert_eq!(props.subject.as_deref(), Some("<redacted>"));
    assert_eq!(props.cards.as_ref(), Some(&serde_json::json!("[]")), "stringified array");
    assert_eq!(props.language_stamp.as_deref(), Some("<redacted>"), "language stamp is a string");
}

/// The wire ships `mentions` as a *stringified* JSON array — the accessor must parse it.
#[test]
fn stringified_mentions_are_parsed() {
    let page = page();
    let mentions = page.messages[0].mentions();

    // Fixture keeps 2 of the 3 raw mentions (sanitizer truncates arrays).
    assert_eq!(mentions.len(), 2);
    let first = &mentions[0];
    assert_eq!(first.schema.as_deref(), Some("http://schema.skype.com/Mention"));
    assert_eq!(first.item_id, Some(0));
    assert_eq!(first.mention_type.as_deref(), Some("person"));
    assert_eq!(first.mri.as_deref(), Some("<redacted>"));
    assert_eq!(first.display_name.as_deref(), Some("<redacted>"));
    assert_eq!(mentions[1].item_id, Some(1));

    let other = &page.messages[1];
    assert!(other.mentions().is_empty(), "system message has no mentions");
}

#[test]
fn reactions_merge_emotions_and_delta_emotions() {
    let page = page();
    let emotions = page.messages[0].emotions();

    let keys: Vec<&str> = emotions.iter().map(|e| e.key.as_str()).collect();
    assert!(keys.contains(&"follow"));
    assert!(keys.contains(&"1f389_partypopper"));

    let with_users = emotions.iter().find(|e| !e.users.is_empty()).expect("users");
    let user = &with_users.users[0];
    assert_eq!(user.mri.as_deref(), Some("<redacted>"));
    assert_eq!(
        user.time,
        Some(serde_json::json!(0)),
        "ms-epoch numbers ≥1e12 are zeroed by the sanitizer"
    );
    assert!(user.value.is_some());
    assert!(user.extra.is_empty());
}

#[test]
fn annotations_summary_counts_reactions() {
    let page = page();
    let summary =
        page.messages[0].annotations_summary.as_ref().expect("annotationsSummary present");
    let emotions =
        summary.get("emotions").and_then(serde_json::Value::as_object).expect("emotions map");
    assert_eq!(emotions.get("like").and_then(serde_json::Value::as_u64), Some(1));
    assert_eq!(emotions.get("1f389_partypopper").and_then(serde_json::Value::as_u64), Some(6));
}

/// System messages are a separate XML dialect, not HTML — and must classify as system.
#[test]
fn system_message_keeps_xml_structure_without_identities() {
    let page = page();
    let msg = &page.messages[1];

    assert_eq!(msg.message_type.as_deref(), Some("ThreadActivity/AddMember"));
    assert!(msg.is_system());
    assert!(msg.content_type.is_none(), "system messages carry no contenttype");
    assert!(msg.im_display_name.is_none());
    assert!(msg.properties.extra.is_empty());

    let content = msg.content.as_deref().expect("system body");
    assert!(content.starts_with("<addmember>"), "XML dialect, not HTML");
    assert!(content.contains("<target>8:orgid:00000000-0000-0000-0000-000000000000</target>"));
    assert!(content.contains("<eventtime>0</eventtime>"));
    assert!(!content.contains("74da8b91"), "tenant must never reach the fixture");
}

#[test]
fn edited_and_delete_markers_are_reachable() {
    let page = page();
    let msg = &page.messages[2];

    assert!(msg.is_edited());
    assert_eq!(msg.skype_edited_id.as_deref(), Some(msg.client_message_id.as_deref().unwrap()));
    assert!(!msg.is_system());

    // Delete/bookkeeping keys are not yet first-class fields — they must survive in the
    // property catch-all so later work can promote them deliberately.
    let props = &msg.properties;
    assert_eq!(props.extra.get("adminDelete").and_then(serde_json::Value::as_bool), Some(true));
    assert!(props.extra.contains_key("deletetime"));
    assert!(props.extra.contains_key("hardDeleteTime"));
    assert!(props.extra.contains_key("hardDeleteReason"));
}

/// One weird shape must never fail the whole page (AGENTS.md: degrade, don't error).
#[test]
fn tolerates_minimal_and_unknown_shapes() {
    let raw = r#"{
        "messages": [
            {"id": "1700000000001", "futureField": 42},
            {"id": "1700000000002", "properties": {"brandNewProp": {"nested": true}}}
        ],
        "brandNewRoot": "x"
    }"#;
    let page: MessagePage = serde_json::from_str(raw).expect("minimal page");

    assert_eq!(page.messages.len(), 2);
    assert_eq!(
        page.messages[0].extra.get("futureField").and_then(serde_json::Value::as_i64),
        Some(42)
    );
    assert!(page.messages[1].properties.extra.contains_key("brandNewProp"));
    assert!(page.extra.contains_key("brandNewRoot"));
    assert!(page.metadata.sync_state.is_none());
    assert!(page.messages[0].mentions().is_empty(), "no properties at all → no mentions");
    assert!(page.messages[0].emotions().is_empty(), "no properties at all → no reactions");
}

#[test]
fn roundtrip_is_lossless() {
    let page = page();
    let again = serde_json::to_string(&page).expect("serialize");
    let back: MessagePage = serde_json::from_str(&again).expect("reparse");
    assert_eq!(page, back);
}

/// `GET …/conversations/{conv};messageid={root}/messages` — same envelope as the plain
/// messages page, holding the root plus its replies.
#[test]
fn thread_fixture_holds_root_and_reply() {
    let page: MessagePage = serde_json::from_str(THREAD_FIXTURE).expect("thread fixture");

    assert_eq!(page.messages.len(), 2);
    assert_eq!(page.tenant_id.as_deref(), Some("<redacted>"));
    let meta = &page.metadata;
    assert!(meta.sync_state.is_some(), "thread pages still expose a cursor");
    assert!(
        meta.backward_link.is_none(),
        "thread response carries only syncState + segment bounds"
    );
    assert_eq!(meta.last_complete_segment_start_time, Some(0));

    let (root, reply) = (&page.messages[0], &page.messages[1]);
    assert!(!root.is_reply(), "root points at itself");
    assert!(reply.is_reply());
    assert_eq!(
        reply.root_message_id.as_deref(),
        Some(root.id.as_str()),
        "reply resolves to the root fetched"
    );
    assert_eq!(reply.conversation_id, root.conversation_id);
    assert_eq!(reply.message_type, root.message_type);
    // The reply carries the stringified attachment arrays; the root does not.
    assert_eq!(reply.properties.cards.as_ref(), Some(&serde_json::json!("[]")));
    assert_eq!(reply.properties.files.as_ref(), Some(&serde_json::json!("[]")));
    assert_eq!(reply.properties.links.as_ref(), Some(&serde_json::json!("[]")));
}

/// `GET …/conversations/{conv}/messages/{id}` — a bare message, no page envelope.
#[test]
fn single_message_fixture_is_bare_and_typed() {
    let msg: Message = serde_json::from_str(SINGLE_FIXTURE).expect("single fixture");

    assert_eq!(msg.id, "1793058838818");
    assert_eq!(msg.sequence_id, Some(733));
    assert_eq!(msg.content_type.as_deref(), Some("Text"));
    assert_eq!(
        msg.from_tenant_id.as_deref(),
        Some("<redacted>"),
        "single-message reads add fromTenantId"
    );
    assert!(msg.annotations_summary.is_some());
    assert!(!msg.is_reply());
    assert!(!msg.is_edited());

    // The lenient accessors work on this shape too.
    let mentions = msg.mentions();
    assert_eq!(mentions.len(), 1);
    assert_eq!(mentions[0].mention_type.as_deref(), Some("person"));
    assert_eq!(mentions[0].item_id, Some(0));

    let emotions = msg.emotions();
    assert_eq!(emotions.len(), 1);
    assert_eq!(emotions[0].key, "follow");

    assert!(msg.properties.subject.is_some());
    assert_eq!(
        msg.properties.extra.get("skipfanouttobots"),
        Some(&serde_json::json!(false)),
        "untyped bool property survives in the catch-all"
    );
}
