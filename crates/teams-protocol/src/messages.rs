//! Message page wire types — `GET /v1/users/ME/conversations/{conv}/messages`.
//!
//! Field names documented verbatim from live responses (2026-10-27, sanitized fixture:
//! `tests/fixtures/messages.json`). Same tolerance rules as
//! [`crate::conversations`]: everything optional, unknown fields preserved, no I/O.
//!
//! Two wire quirks this module exists to absorb:
//! - **`properties` is heterogeneous**: `mentions`/`cards`/`files`/`links` arrive as
//!   *stringified* JSON, `emotions`/`deltaEmotions` as real arrays, flags as bools or
//!   strings. They are kept as [`serde_json::Value`] and exposed through lenient
//!   accessors, so a shape change degrades one accessor instead of failing the page.
//! - **`content` is polymorphic**: HTML for `RichText/Html`, a different XML dialect for
//!   `ThreadActivity/*` system messages, plain text for `Text`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::SyncMetadata;

/// Root of a messages page.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MessagePage {
    /// Sync cursor for paging backwards through history (wire key `_metadata`).
    #[serde(rename = "_metadata", default)]
    pub metadata: SyncMetadata,
    /// Messages of the page, newest-first in observed traffic.
    #[serde(default)]
    pub messages: Vec<Message>,
    /// Tenant the conversation belongs to (36-char GUID).
    #[serde(rename = "tenantId", default)]
    pub tenant_id: Option<String>,
    /// Unknown root-level fields.
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

/// One message. Also used for `Conversation::lastMessage` — same wire shape.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Message {
    /// Server message id (13-digit numeric string observed — keep as string).
    #[serde(default)]
    pub id: String,
    /// Client-generated id, used for send de-duplication.
    #[serde(rename = "clientmessageid", default)]
    pub client_message_id: Option<String>,
    /// Send time, RFC-3339-like.
    #[serde(rename = "composetime", default)]
    pub compose_time: Option<String>,
    /// Arrival time, RFC-3339-like; doubles as the message id for roots.
    #[serde(rename = "originalarrivaltime", default)]
    pub original_arrival_time: Option<String>,
    /// Message version (13-digit numeric string observed).
    #[serde(default)]
    pub version: Option<String>,
    /// Ordering sequence within the conversation.
    #[serde(rename = "sequenceId", default)]
    pub sequence_id: Option<u64>,
    /// Conversation id this message belongs to.
    #[serde(rename = "conversationid", default)]
    pub conversation_id: Option<String>,
    /// Absolute link to the conversation.
    #[serde(rename = "conversationLink", default)]
    pub conversation_link: Option<String>,
    /// Sender identity (MRI such as `8:orgid:<guid>`, or a URL identity).
    #[serde(default)]
    pub from: Option<String>,
    /// Sender display name as rendered in chat.
    #[serde(rename = "imdisplayname", default)]
    pub im_display_name: Option<String>,
    /// Sender display name taken from the auth token.
    #[serde(rename = "fromDisplayNameInToken", default)]
    pub from_display_name_in_token: Option<String>,
    /// Sender family name taken from the auth token.
    #[serde(rename = "fromFamilyNameInToken", default)]
    pub from_family_name_in_token: Option<String>,
    /// Sender given name taken from the auth token.
    #[serde(rename = "fromGivenNameInToken", default)]
    pub from_given_name_in_token: Option<String>,
    /// Sender identity of another tenant, present on some single-message reads.
    #[serde(rename = "fromTenantId", default)]
    pub from_tenant_id: Option<String>,
    /// Message body: HTML, XML or plain text depending on [`Message::message_type`].
    #[serde(default)]
    pub content: Option<String>,
    /// Body encoding (observed `Text` and lowercase `text` — kept as observed).
    #[serde(rename = "contenttype", default)]
    pub content_type: Option<String>,
    /// Message classifier: `RichText/Html`, `Text`, `ThreadActivity/AddMember`, …
    #[serde(rename = "messagetype", default)]
    pub message_type: Option<String>,
    /// Preview kind (observed `Message`).
    #[serde(rename = "type", default)]
    pub type_name: Option<String>,
    /// Thread root: equals `id` for roots, points at the root for replies.
    #[serde(rename = "rootMessageId", default)]
    pub root_message_id: Option<String>,
    /// Posting mode (observed `Standard`).
    #[serde(rename = "postType", default)]
    pub post_type: Option<String>,
    /// Server hint to keep the sender's display name authoritative.
    #[serde(rename = "prioritizeImDisplayName", default)]
    pub prioritize_im_display_name: Option<bool>,
    /// AMS (attachment/media) references; empty array when there are none.
    #[serde(rename = "amsreferences", default)]
    pub ams_references: Option<Value>,
    /// Aggregated reaction counts (`{"emotions":{"like":1}}` observed).
    #[serde(rename = "annotationsSummary", default)]
    pub annotations_summary: Option<Map<String, Value>>,
    /// Set when the message was edited — the edit marker, not the edited value.
    #[serde(rename = "skypeeditedid", default)]
    pub skype_edited_id: Option<String>,
    /// Whether annotations (reactions) exist for this message.
    #[serde(rename = "hasAnnotated", default)]
    pub has_annotated: Option<bool>,
    /// Heterogeneous property bag (mentions, files, reactions, edit/delete markers, …).
    #[serde(default)]
    pub properties: MessageProperties,
    /// Unknown fields.
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

/// The message property bag. Values are kept as [`Value`] where the wire is
/// stringified JSON or where we have only one observation of the type; use
/// [`Message::mentions`] / [`Message::emotions`] for typed access.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MessageProperties {
    /// Mentions: **stringified** JSON array (or, in some responses, a real array).
    #[serde(default)]
    pub mentions: Option<Value>,
    /// Adaptive cards: stringified JSON array.
    #[serde(default)]
    pub cards: Option<Value>,
    /// Attached files: stringified JSON array.
    #[serde(default)]
    pub files: Option<Value>,
    /// Link previews: stringified JSON array.
    #[serde(default)]
    pub links: Option<Value>,
    /// Reactions, as `[{key, users:[{mri, time, value}]}]` — real JSON array.
    #[serde(rename = "deltaEmotions", default)]
    pub delta_emotions: Option<Value>,
    /// Current reaction set, same shape as [`MessageProperties::delta_emotions`].
    #[serde(default)]
    pub emotions: Option<Value>,
    /// Language detection stamp (`languages=en:100;…&detector=Bling`).
    #[serde(rename = "languageStamp", default)]
    pub language_stamp: Option<String>,
    /// Client format hint (observed `TEAMS`, `Text`).
    #[serde(rename = "formatVariant", default)]
    pub format_variant: Option<String>,
    /// Subject line carried by system messages.
    #[serde(default)]
    pub subject: Option<String>,
    /// Blur-hash placeholder for inline images.
    #[serde(rename = "blurHash", default)]
    pub blur_hash: Option<String>,
    /// Unknown property keys (`draftId`, `deletetime`, `eventReason`, `atp`, …).
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

/// One mention entry from [`MessageProperties::mentions`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Mention {
    /// Schema discriminator, e.g. `http://schema.skype.com/Mention`.
    #[serde(rename = "@type", default)]
    pub schema: Option<String>,
    /// Character offset of the mention in the message body.
    #[serde(rename = "itemid", default)]
    pub item_id: Option<u64>,
    /// Mentioned user's MRI (`8:orgid:<guid>`).
    #[serde(default)]
    pub mri: Option<String>,
    /// Mention kind (observed `person`).
    #[serde(rename = "mentionType", default)]
    pub mention_type: Option<String>,
    /// Display name rendered at the mention position.
    #[serde(rename = "displayName", default)]
    pub display_name: Option<String>,
    /// Unknown fields.
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

/// One reaction kind on a message: `key` is `like`, `think`, an emoji shortcode, …
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Emotion {
    /// Reaction kind.
    #[serde(default)]
    pub key: String,
    /// Who reacted.
    #[serde(default)]
    pub users: Vec<EmotionUser>,
    /// Unknown fields.
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

/// One user's reaction.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct EmotionUser {
    /// Reacting user's MRI.
    #[serde(default)]
    pub mri: Option<String>,
    /// Reaction time (ms epoch; observed as a **number**).
    #[serde(default)]
    pub time: Option<Value>,
    /// Reaction value (ms epoch; observed as a **string** — hence untyped).
    #[serde(default)]
    pub value: Option<Value>,
    /// Unknown fields.
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

impl Message {
    /// Parsed mentions. The wire ships them stringified, sometimes as a real array,
    /// and occasionally as something else entirely — anything unparseable yields `[]`
    /// rather than failing the whole page.
    #[must_use]
    pub fn mentions(&self) -> Vec<Mention> {
        parse_lenient(self.properties.mentions.as_ref())
    }

    /// Parsed reactions. Same leniency contract as [`Message::mentions`].
    #[must_use]
    pub fn emotions(&self) -> Vec<Emotion> {
        parse_lenient(self.properties.emotions.as_ref())
            .into_iter()
            .chain(parse_lenient(self.properties.delta_emotions.as_ref()))
            .collect()
    }

    /// `true` for server-generated activity messages (`ThreadActivity/*`,
    /// `SystemEventMessage`) — these render as control events, not chat bubbles.
    #[must_use]
    pub fn is_system(&self) -> bool {
        self.message_type.as_deref().is_some_and(|kind| {
            kind.starts_with("ThreadActivity/") || kind.starts_with("SystemEvent")
        })
    }

    /// `true` when the service reports an edit marker.
    #[must_use]
    pub fn is_edited(&self) -> bool {
        self.skype_edited_id.is_some()
    }

    /// `true` when a reply points at a different root message.
    #[must_use]
    pub fn is_reply(&self) -> bool {
        match (self.root_message_id.as_deref(), self.id.as_str()) {
            (Some(root), id) => !root.is_empty() && root != id,
            (None, _) => false,
        }
    }
}

/// Parse a value that may be a JSON string or a JSON array into `T`.
///
/// Never fails: a wrong or corrupt shape returns an empty list.
fn parse_lenient<T: serde::de::DeserializeOwned>(value: Option<&Value>) -> Vec<T> {
    match value {
        Some(Value::String(raw)) => serde_json::from_str(raw).unwrap_or_default(),
        Some(Value::Array(_) | Value::Object(_)) => {
            serde_json::from_value(value.cloned().unwrap_or(Value::Null)).unwrap_or_default()
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Message {
        serde_json::from_value(serde_json::json!({
            "id": "1793085256125",
            "messagetype": "RichText/Html",
            "content": "<p>hi</p>",
            "rootMessageId": "1793085256125",
            "properties": {
                "mentions": "[{\"itemid\":0,\"mri\":\"8:orgid:1\",\"mentionType\":\"person\"}]",
                "emotions": [{"key": "like", "users": [{"mri": "8:orgid:1", "time": 1, "value": "2"}]}]
            },
            "futureField": {"nested": true}
        }))
        .expect("sample")
    }

    #[test]
    fn stringified_mentions_are_parsed() {
        let msg = sample();
        let mentions = msg.mentions();
        assert_eq!(mentions.len(), 1);
        assert_eq!(mentions[0].mention_type.as_deref(), Some("person"));
        assert_eq!(mentions[0].item_id, Some(0));
    }

    #[test]
    fn real_array_mentions_are_parsed_too() {
        let mut msg = sample();
        msg.properties.mentions = Some(serde_json::json!([
            {"mri": "8:orgid:2", "displayName": "Someone"}
        ]));
        let mentions = msg.mentions();
        assert_eq!(mentions.len(), 1);
        assert_eq!(mentions[0].display_name.as_deref(), Some("Someone"));
    }

    #[test]
    fn unparseable_properties_degrade_to_empty() {
        let mut msg = sample();
        msg.properties.mentions = Some(serde_json::json!(42));
        assert!(msg.mentions().is_empty(), "wrong JSON type degrades to no mentions");

        msg.properties.mentions = Some(serde_json::json!("not json {"));
        assert!(msg.mentions().is_empty(), "corrupt JSON degrades to no mentions");
    }

    #[test]
    fn emotions_merge_current_and_delta() {
        let mut msg = sample();
        msg.properties.delta_emotions = Some(serde_json::json!([{"key": "think", "users": []}]));
        let keys: Vec<String> = msg.emotions().into_iter().map(|e| e.key).collect();
        assert_eq!(keys, ["like", "think"]);
    }

    #[test]
    fn classification_helpers() {
        let msg = sample();
        assert!(!msg.is_system());
        assert!(!msg.is_edited());
        assert!(!msg.is_reply());

        let mut system = msg.clone();
        system.message_type = Some("ThreadActivity/AddMember".into());
        assert!(system.is_system());

        let mut edited = msg.clone();
        edited.skype_edited_id = Some("123".into());
        assert!(edited.is_edited());

        let mut reply = msg.clone();
        reply.root_message_id = Some("1793058838818".into());
        assert!(reply.is_reply());
    }

    #[test]
    fn unknown_fields_are_preserved() {
        let msg = sample();
        assert!(msg.extra.contains_key("futureField"));
    }
}
