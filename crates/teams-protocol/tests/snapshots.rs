//! Snapshot tests over the sanitized wire fixtures.
//!
//! Deserializing each fixture and serializing the model back through serde pins the
//! field names we read on the wire: if a rename or default drifts, the snapshot diff
//! shows it before a live capture does. Snapshots are derived from the sanitized
//! fixtures only — never replace them with unsanitized data (see CONTRIBUTING.md).
//!
//! Review/regenerate with `cargo insta` (`INSTA_UPDATE=always cargo insta test`), or
//! accept freshly written `.snap.new` files manually.

use teams_protocol::{
    ConversationList, EventMessage, Message, MessagePage, RosterUpdate, SearchSuggestions,
};

/// Snapshot the parsed `ConversationList` fixture.
#[test]
fn conversation_list_snapshot() {
    let model: ConversationList = serde_json::from_str(include_str!("fixtures/conversations.json"))
        .expect("conversations fixture");
    insta::assert_json_snapshot!(model);
}

/// Snapshot the parsed message-page fixture (chat bubble + system + edited shapes).
#[test]
fn messages_page_snapshot() {
    let model: MessagePage =
        serde_json::from_str(include_str!("fixtures/messages.json")).expect("messages fixture");
    insta::assert_json_snapshot!(model);
}

/// Snapshot the parsed thread fixture.
#[test]
fn thread_page_snapshot() {
    let model: MessagePage =
        serde_json::from_str(include_str!("fixtures/thread.json")).expect("thread fixture");
    insta::assert_json_snapshot!(model);
}

/// Snapshot the parsed single-message fixture.
#[test]
fn single_message_snapshot() {
    let model: Message =
        serde_json::from_str(include_str!("fixtures/message.json")).expect("message fixture");
    insta::assert_json_snapshot!(model);
}

/// Snapshot the parsed CSA roster fixture.
#[test]
fn roster_snapshot() {
    let model: RosterUpdate =
        serde_json::from_str(include_str!("fixtures/roster.json")).expect("roster fixture");
    insta::assert_json_snapshot!(model);
}

/// Snapshot the parsed substrate search fixture.
#[test]
fn search_snapshot() {
    let model: SearchSuggestions =
        serde_json::from_str(include_str!("fixtures/search.json")).expect("search fixture");
    insta::assert_json_snapshot!(model);
}

/// Snapshot the parsed call-event fixture (a `callEnded` and a `callStarted` body).
#[test]
fn event_call_page_snapshot() {
    let model: MessagePage =
        serde_json::from_str(include_str!("fixtures/event-call.json")).expect("event-call fixture");
    insta::assert_json_snapshot!(model);
}

/// Snapshot the typed projection of the call events (participants/occurrence prose is
/// sanitizer-redacted; the structure is what this locks in).
#[test]
fn event_call_projection_snapshot() {
    let model: MessagePage =
        serde_json::from_str(include_str!("fixtures/event-call.json")).expect("event-call fixture");
    let events: Vec<EventMessage> =
        model.messages.iter().filter_map(Message::event_message).collect();
    insta::assert_json_snapshot!(events);
}

/// Snapshot a parsed `RichText` AST from a synthesized dialect sample (the live HTML is
/// gitignored, so the snapshot keeps a synthetic-but-faithful sample).
#[test]
fn richtext_projection_snapshot() {
    let html = "<p style=\"margin-left:0px\">Hello <strong>world</strong> and \
<span class=\"animated-emoticon-20-smile\" type=\"(smile)\" title=\"Smile\" itemscope>\
<img src=\"https://cdn.invalid/smile.gif\" alt=\"Smile\" itemid=\"smile\" \
itemscope itemtype=\"http://schema.skype.com/\"/></span>\
<span itemscope itemtype=\"http://schema.skype.com/\" itemid=\"0\">@Rex</span></p>";
    let model = teams_protocol::parse_with_mentions(
        html,
        &[teams_protocol::Mention {
            display_name: Some("Rex".into()),
            item_id: Some(0),
            ..Default::default()
        }],
    );
    insta::assert_json_snapshot!(model);
}
