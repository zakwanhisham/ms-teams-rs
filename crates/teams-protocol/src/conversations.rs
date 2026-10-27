//! Conversation list wire types — `GET /v1/users/ME/conversations` (IC3 / MSNP24).
//!
//! Field names are documented verbatim from a live response (2026-10-27, sanitized
//! fixture: `tests/fixtures/conversations.json`). Design rules per `AGENTS.md`:
//! - every field is optional/defaulted so one unexpected shape never fails the page;
//! - unknown fields are preserved in each type's `extra` catch-all (flattened map);
//! - camelCase wire names map via explicit `#[serde(rename)]` because the protocol mixes
//!   camelCase (`threadProperties`) with concatenated lowercase (`lastimreceivedtime`).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Root of the conversations page.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ConversationList {
    /// Pagination/sync hints (wire key `_metadata`).
    #[serde(rename = "_metadata", default)]
    pub metadata: SyncMetadata,
    /// The conversations in this page (chats **and** channels).
    #[serde(default)]
    pub conversations: Vec<Conversation>,
    /// Any root-level fields we don't know yet.
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

/// `_metadata` block: sync cursor for incremental fetches. Shared by the conversations
/// page and the messages page (which adds the segment-boundary timestamps).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SyncMetadata {
    /// Opaque cursor for fetching older pages, when present.
    #[serde(rename = "backwardLink", default)]
    pub backward_link: Option<String>,
    /// Cursor for newer pages; empty string means "no more" in observed traffic.
    #[serde(rename = "forwardLink", default)]
    pub forward_link: Option<String>,
    /// Opaque sync state (479 chars observed on the conversations page) — pass back to
    /// resume pagination.
    #[serde(rename = "syncState", default)]
    pub sync_state: Option<String>,
    /// Number of items the service reports for this page.
    #[serde(rename = "totalCount", default)]
    pub total_count: Option<u64>,
    /// End of the newest complete message segment (ms epoch); messages page only.
    #[serde(rename = "lastCompleteSegmentEndTime", default)]
    pub last_complete_segment_end_time: Option<u64>,
    /// Start of the oldest complete message segment (ms epoch); messages page only.
    #[serde(rename = "lastCompleteSegmentStartTime", default)]
    pub last_complete_segment_start_time: Option<u64>,
    /// Unknown fields.
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

/// One conversation: a 1:1/group chat or a team channel.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Conversation {
    /// Conversation id (60 chars observed; contains `:`/`@`/`;` — percent-encode in URLs).
    #[serde(default)]
    pub id: String,
    /// Conversation kind (12 chars observed; exact values to be catalogued).
    #[serde(rename = "type", default)]
    pub conversation_type: String,
    /// Monotonic conversation version (ms-epoch-like `u64` observed).
    #[serde(default)]
    pub version: Option<u64>,
    /// Preview of the newest message (same wire shape as the messages endpoint).
    #[serde(rename = "lastMessage", default)]
    pub last_message: Option<crate::Message>,
    /// Read-receipt metadata version.
    #[serde(rename = "lastRcMetadataVersion", default)]
    pub last_rc_metadata_version: Option<u64>,
    /// Server id of the newest message, as a number.
    #[serde(rename = "lastUpdatedMessageId", default)]
    pub last_updated_message_id: Option<u64>,
    /// Version of the newest message.
    #[serde(rename = "lastUpdatedMessageVersion", default)]
    pub last_updated_message_version: Option<u64>,
    /// The caller's membership state in this conversation.
    #[serde(rename = "memberProperties", default)]
    pub member_properties: Option<MemberProperties>,
    /// URL to the messages collection of this conversation.
    #[serde(rename = "messages", default)]
    pub messages_link: Option<String>,
    /// Conversation-level properties (favourite, last activity, …).
    #[serde(default)]
    pub properties: ConversationProperties,
    /// Absolute link to this conversation resource.
    #[serde(rename = "targetLink", default)]
    pub target_link: Option<String>,
    /// Thread-level metadata (topic, thread type, group id, …).
    #[serde(rename = "threadProperties", default)]
    pub thread_properties: Option<ThreadProperties>,
    /// Unknown fields.
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

/// Thread-level properties of a conversation.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ThreadProperties {
    /// Thread creation time (RFC-3339-like; wire key is concatenated lowercase).
    #[serde(rename = "createdat", default)]
    pub created_at: Option<String>,
    /// Team/group id owning the thread (36-char GUID observed).
    #[serde(rename = "groupId", default)]
    pub group_id: Option<String>,
    /// Original thread id (60 chars, same shape as [`Conversation::id`]).
    #[serde(rename = "originalThreadId", default)]
    pub original_thread_id: Option<String>,
    /// Product/thread classifier (9 chars observed, e.g. chat vs channel).
    #[serde(rename = "productThreadType", default)]
    pub product_thread_type: Option<String>,
    /// Human-visible conversation topic/title (spaces).
    #[serde(rename = "spaceThreadTopic", default)]
    pub space_thread_topic: Option<String>,
    /// Channel topic/title (observed on `threadType: "topic"` threads).
    #[serde(default)]
    pub topic: Option<String>,
    /// Space classification (8 chars observed).
    #[serde(rename = "spaceType", default)]
    pub space_type: Option<String>,
    /// Thread classification (5 chars observed).
    #[serde(rename = "threadType", default)]
    pub thread_type: Option<String>,
    /// Last sequence id as a numeric string.
    #[serde(rename = "lastSequenceId", default)]
    pub last_sequence_id: Option<String>,
    /// Roster (membership) version.
    #[serde(rename = "rosterVersion", default)]
    pub roster_version: Option<u64>,
    /// Thread version (numeric string).
    #[serde(default)]
    pub version: Option<String>,
    /// Whether the caller created this thread.
    #[serde(rename = "isCreator", default)]
    pub is_creator: Option<bool>,
    /// Whether the caller's join is tracked (wire: stringified bool).
    #[serde(rename = "gapDetectionEnabled", default)]
    pub gap_detection_enabled: Option<String>,
    /// Last join time (epoch-milliseconds string observed).
    #[serde(rename = "lastjoinat", default)]
    pub last_join_at: Option<String>,
    /// Meeting-chat enforcement flag (stringified bool).
    #[serde(rename = "ongoingCallChatEnforcement", default)]
    pub ongoing_call_chat_enforcement: Option<String>,
    /// `SharePoint` site backing the channel, when present.
    #[serde(rename = "sharepointSiteUrl", default)]
    pub sharepoint_site_url: Option<String>,
    /// Unknown fields.
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

/// Conversation-level properties (note the concatenated-lowercase wire keys).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ConversationProperties {
    /// Who added the caller (44-char identity observed).
    #[serde(rename = "addedBy", default)]
    pub added_by: Option<String>,
    /// Tenant that added the caller (36-char GUID observed).
    #[serde(rename = "addedByTenantId", default)]
    pub added_by_tenant_id: Option<String>,
    /// Collapsed state (stringified bool).
    #[serde(default)]
    pub collapsed: Option<String>,
    /// Read-receipt consumption horizon (opaque cursor).
    #[serde(default)]
    pub consumptionhorizon: Option<String>,
    /// Favourite flag (stringified bool).
    #[serde(default)]
    pub favorite: Option<String>,
    /// Whether the conversation is empty (stringified bool).
    #[serde(default)]
    pub isemptyconversation: Option<String>,
    /// Last time the caller favourited it (epoch-milliseconds string).
    #[serde(rename = "lastTimeFavorited", default)]
    pub last_time_favorited: Option<String>,
    /// Last inbound message time (RFC-3339-like).
    #[serde(rename = "lastimreceivedtime", default)]
    pub last_im_received_time: Option<String>,
    /// Unknown fields.
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

/// The caller's membership state in a conversation.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MemberProperties {
    /// Whether the caller has read up to the horizon.
    #[serde(rename = "isReader", default)]
    pub is_reader: Option<bool>,
    /// Membership expiration (ms-epoch `u64`; max-value sentinel observed).
    #[serde(rename = "memberExpirationTime", default)]
    pub member_expiration_time: Option<u64>,
    /// Membership role (4 chars observed — e.g. `User`/`Admin`).
    #[serde(default)]
    pub role: Option<String>,
    /// Unknown fields.
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

impl Conversation {
    /// Human-visible topic: spaces carry `spaceThreadTopic`, channels carry `topic`.
    #[must_use]
    pub fn topic(&self) -> Option<&str> {
        let thread = self.thread_properties.as_ref()?;
        thread.space_thread_topic.as_deref().or(thread.topic.as_deref())
    }

    /// Product-level thread classifier, e.g. `TeamsTeam` vs `TeamsStandardChannel`.
    #[must_use]
    pub fn product_thread_type(&self) -> Option<&str> {
        self.thread_properties.as_ref().and_then(|thread| thread.product_thread_type.as_deref())
    }
}
