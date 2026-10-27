//! CSA roster wire types — `GET {csa}/api/v1/teams/users/me/updates`.
//!
//! Field names documented verbatim from the live response (2026-10-27, sanitized fixture:
//! `tests/fixtures/roster.json`). Same tolerance rules as [`crate::conversations`]:
//! everything optional, unknown fields preserved, no I/O.
//!
//! Three findings this module exists to record:
//! - **Each team lists only its General channel** (`channels[0].isGeneral == true`) — the
//!   rest of a team's channels do not arrive here. Channels the user follows outside a
//!   team's own list arrive in the top-level `channels` array instead, so a UI must merge
//!   both (and fall back to the chat-service conversation list for anything else).
//! - **Last-message previews are camelCase** (`originalArrivalTime`, `imDisplayName`),
//!   unlike the chat service's all-lowercase [`crate::Message`] — hence
//!   [`PreviewMessage`], which is deliberately a separate type.
//! - The response doubles as the **meeting list**: `chats` carries `chatType: "meeting"`
//!   entries with their `meetingJoinUrl`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Root of the CSA `users/me/updates` response.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct RosterUpdate {
    /// Teams the user belongs to (see the General-only caveat in the module docs).
    #[serde(default)]
    pub teams: Vec<Team>,
    /// Channels reported outside any team entry (channel-only / favourited channels).
    #[serde(default)]
    pub channels: Vec<Channel>,
    /// Direct chats and scheduled meetings, with previews and meeting details.
    #[serde(default)]
    pub chats: Vec<ChatSummary>,
    /// Sync/paging state of this update.
    #[serde(default)]
    pub metadata: RosterMetadata,
    /// Notification/draft/mention/call-log streams — kept raw (no consumer yet).
    #[serde(rename = "privateFeeds", default)]
    pub private_feeds: Vec<Value>,
    /// Directory users; empty in every observed response.
    #[serde(default)]
    pub users: Vec<Value>,
    /// Unknown root-level fields.
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

/// Sync/paging state of a roster update.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct RosterMetadata {
    /// Opaque cursor for the next incremental update (base64 blob).
    #[serde(rename = "syncToken", default)]
    pub sync_token: Option<String>,
    /// Cursor for reading forwards, when there is more to fetch.
    #[serde(rename = "forwardSyncToken", default)]
    pub forward_sync_token: Option<String>,
    /// Whether the `chats` array was cut short.
    #[serde(rename = "hasMoreChats", default)]
    pub has_more_chats: Option<bool>,
    /// Whether this response is a partial (prefetch) view.
    #[serde(rename = "isPartialData", default)]
    pub is_partial_data: Option<bool>,
    /// Unknown fields.
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

/// One team.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Team {
    /// Team thread id (`19:…@thread.skype` observed — usable as a conversation id).
    #[serde(default)]
    pub id: Option<String>,
    /// Display name.
    #[serde(rename = "displayName", default)]
    pub display_name: Option<String>,
    /// Team description/motto.
    #[serde(default)]
    pub description: Option<String>,
    /// Caller's role in the team (numeric).
    #[serde(rename = "memberRole", default)]
    pub member_role: Option<u64>,
    /// Caller's role as text (observed `user`).
    #[serde(rename = "memberRoleString", default)]
    pub member_role_string: Option<String>,
    /// Member counts by role, present when `enableMembershipSummary=true`.
    #[serde(rename = "membershipSummary", default)]
    pub membership_summary: Option<MembershipSummary>,
    /// Roster version (ms-epoch stamp observed).
    #[serde(rename = "rosterVersion", default)]
    pub roster_version: Option<u64>,
    /// Team's mail-enabled address, when it has one.
    #[serde(rename = "smtpAddress", default)]
    pub smtp_address: Option<String>,
    /// Owning tenant id (36-char GUID).
    #[serde(rename = "tenantId", default)]
    pub tenant_id: Option<String>,
    /// Archived teams are read-only.
    #[serde(rename = "isArchived", default)]
    pub is_archived: Option<bool>,
    /// Whether the caller created the team.
    #[serde(rename = "isCreator", default)]
    pub is_creator: Option<bool>,
    /// Tombstone marker.
    #[serde(rename = "isDeleted", default)]
    pub is_deleted: Option<bool>,
    /// Favourited by the caller.
    #[serde(rename = "isFavorite", default)]
    pub is_favorite: Option<bool>,
    /// Followed by the caller (drives notification behaviour).
    #[serde(rename = "isFollowed", default)]
    pub is_followed: Option<bool>,
    /// Whether the team is locked (membership sync disabled).
    #[serde(rename = "isTeamLocked", default)]
    pub is_team_locked: Option<bool>,
    /// Tenant-wide (org-wide) team.
    #[serde(rename = "isTenantWide", default)]
    pub is_tenant_wide: Option<bool>,
    /// Caller has muted the team.
    #[serde(rename = "isUserMuted", default)]
    pub is_user_muted: Option<bool>,
    /// When the caller last joined (RFC-3339-like).
    #[serde(rename = "lastJoinAt", default)]
    pub last_join_at: Option<String>,
    /// Channels included in this update — **General only** in observed traffic.
    #[serde(default)]
    pub channels: Vec<Channel>,
    /// Channels the service offers but the caller has not added yet.
    #[serde(rename = "availableChannels", default)]
    pub available_channels: Vec<Channel>,
    /// Unknown fields (team settings/site blobs land here, untyped on purpose).
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

/// Member counts returned with `enableMembershipSummary=true`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MembershipSummary {
    /// Owners/admins.
    #[serde(rename = "adminRoleCount", default)]
    pub admin_role_count: Option<u64>,
    /// Bot accounts.
    #[serde(rename = "botCount", default)]
    pub bot_count: Option<u64>,
    /// Members who muted the team.
    #[serde(rename = "mutedMembersCount", default)]
    pub muted_members_count: Option<u64>,
    /// Total membership.
    #[serde(rename = "totalMemberCount", default)]
    pub total_member_count: Option<u64>,
    /// Standard members.
    #[serde(rename = "userRoleCount", default)]
    pub user_role_count: Option<u64>,
    /// Unknown fields.
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

/// One channel, both inside [`Team::channels`] and at the roster's top level.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Channel {
    /// Channel thread id (`19:…@thread.skype` observed).
    #[serde(default)]
    pub id: Option<String>,
    /// Channel display name.
    #[serde(rename = "displayName", default)]
    pub display_name: Option<String>,
    /// Channel topic/description.
    #[serde(default)]
    pub description: Option<String>,
    /// SharePoint/Entra group id backing the team (36-char GUID).
    #[serde(rename = "groupId", default)]
    pub group_id: Option<String>,
    /// Owning team thread id — set on channels reported at the roster's top level.
    #[serde(rename = "parentTeamId", default)]
    pub parent_team_id: Option<String>,
    /// Owning tenant id.
    #[serde(rename = "tenantId", default)]
    pub tenant_id: Option<String>,
    /// Numeric channel class (0 observed for standard channels).
    #[serde(rename = "channelType", default)]
    pub channel_type: Option<u64>,
    /// Whether this is the team's General channel.
    #[serde(rename = "isGeneral", default)]
    pub is_general: Option<bool>,
    /// Archived (read-only).
    #[serde(rename = "isArchived", default)]
    pub is_archived: Option<bool>,
    /// Tombstone marker.
    #[serde(rename = "isDeleted", default)]
    pub is_deleted: Option<bool>,
    /// Favourited by the caller.
    #[serde(rename = "isFavorite", default)]
    pub is_favorite: Option<bool>,
    /// Followed by the caller.
    #[serde(rename = "isFollowed", default)]
    pub is_followed: Option<bool>,
    /// Whether the caller is a member of the channel.
    #[serde(rename = "isMember", default)]
    pub is_member: Option<bool>,
    /// All messages read (drives the unread badge).
    #[serde(rename = "isMessageRead", default)]
    pub is_message_read: Option<bool>,
    /// Caller moderates the channel.
    #[serde(rename = "isModerator", default)]
    pub is_moderator: Option<bool>,
    /// Caller muted the channel.
    #[serde(rename = "isMuted", default)]
    pub is_muted: Option<bool>,
    /// Pinned in the client's sidebar.
    #[serde(rename = "isPinned", default)]
    pub is_pinned: Option<bool>,
    /// Shared with another tenant.
    #[serde(rename = "isShared", default)]
    pub is_shared: Option<bool>,
    /// No messages yet.
    #[serde(rename = "isEmptyConversation", default)]
    pub is_empty_conversation: Option<bool>,
    /// Caller's numeric role in the channel.
    #[serde(rename = "memberRole", default)]
    pub member_role: Option<u64>,
    /// Caller's role as text (observed `user`).
    #[serde(rename = "memberRoleString", default)]
    pub member_role_string: Option<String>,
    /// Thread class (observed `space`).
    #[serde(rename = "threadType", default)]
    pub thread_type: Option<String>,
    /// Thread sub-class (observed `''`).
    #[serde(rename = "threadSubType", default)]
    pub thread_sub_type: Option<String>,
    /// Channel creation time (RFC-3339-like).
    #[serde(rename = "creationTime", default)]
    pub creation_time: Option<String>,
    /// When the caller last joined the channel.
    #[serde(rename = "lastJoinAt", default)]
    pub last_join_at: Option<String>,
    /// Newest message, as a compact camelCase preview.
    #[serde(rename = "lastMessage", default)]
    pub last_message: Option<PreviewMessage>,
    /// Unknown fields (tabs, connector profiles, file settings land here).
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

/// One direct chat or scheduled meeting in [`RosterUpdate::chats`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ChatSummary {
    /// Conversation id — same form as the chat service's, so it can be passed to
    /// `ChatClient::list_messages` directly.
    #[serde(default)]
    pub id: Option<String>,
    /// `chat` or `meeting`.
    #[serde(rename = "chatType", default)]
    pub chat_type: Option<String>,
    /// Chat sub-class — **an integer on the wire** (0/1 observed across 64 chats) despite
    /// the name; semantics unknown, so it is kept as [`Value`].
    #[serde(rename = "chatSubType", default)]
    pub chat_sub_type: Option<Value>,
    /// Thread class (observed `chat`, `meeting`).
    #[serde(rename = "threadType", default)]
    pub thread_type: Option<String>,
    /// Chat title (empty for 1:1 — the peer's name lives in `members`).
    #[serde(default)]
    pub title: Option<String>,
    /// Participants (MRIs, roles, mute flags).
    #[serde(default)]
    pub members: Vec<ChatMember>,
    /// Newest message, as a compact camelCase preview.
    #[serde(rename = "lastMessage", default)]
    pub last_message: Option<PreviewMessage>,
    /// Whether the caller has read the conversation.
    #[serde(rename = "isRead", default)]
    pub is_read: Option<bool>,
    /// Hidden from the caller's chat list.
    #[serde(default)]
    pub hidden: Option<bool>,
    /// Whether the caller started the conversation.
    #[serde(rename = "isOneOnOne", default)]
    pub is_one_on_one: Option<bool>,
    /// Whether the newest message came from the caller.
    #[serde(rename = "isLastMessageFromMe", default)]
    pub is_last_message_from_me: Option<bool>,
    /// Creation time (RFC-3339-like).
    #[serde(rename = "createdAt", default)]
    pub created_at: Option<String>,
    /// When the caller last joined.
    #[serde(rename = "lastJoinAt", default)]
    pub last_join_at: Option<String>,
    /// Scheduled-meeting details; present for `chatType: "meeting"` entries.
    #[serde(rename = "meetingInformation", default)]
    pub meeting_information: Option<MeetingInformation>,
    /// Unknown fields (membership/retention/interop state lands here).
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

/// One participant of a [`ChatSummary`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ChatMember {
    /// Member MRI (`8:orgid:<guid>`, `8:orgid:<tenant>:<user>`, …).
    #[serde(default)]
    pub mri: Option<String>,
    /// Role in the conversation (observed `Organizer`, `User`).
    #[serde(default)]
    pub role: Option<String>,
    /// Whether the caller muted this member.
    #[serde(rename = "isMuted", default)]
    pub is_muted: Option<bool>,
    /// Whether the service hides this member's identity from the caller.
    #[serde(rename = "isIdentityMasked", default)]
    pub is_identity_masked: Option<bool>,
    /// Entra object id of the member (36-char GUID).
    #[serde(rename = "objectId", default)]
    pub object_id: Option<String>,
    /// Unknown fields.
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

/// Scheduled-meeting details carried by meeting chats.
///
/// Kept close to the wire: this is what a join flow needs (start/end, join URL,
/// cancellation), everything else stays in `extra` until Phase 9 needs it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MeetingInformation {
    /// Meeting subject.
    #[serde(default)]
    pub subject: Option<String>,
    /// Scheduled start (RFC-3339, `Z` form).
    #[serde(rename = "startTime", default)]
    pub start_time: Option<String>,
    /// Scheduled end.
    #[serde(rename = "endTime", default)]
    pub end_time: Option<String>,
    /// Location string (observed `Microsoft Teams Meeting`).
    #[serde(default)]
    pub location: Option<String>,
    /// Deep link that joins the meeting — contains the tenant id, never log it.
    #[serde(rename = "meetingJoinUrl", default)]
    pub meeting_join_url: Option<String>,
    /// Organizer MRI.
    #[serde(rename = "organizerId", default)]
    pub organizer_id: Option<String>,
    /// Whether the meeting was cancelled.
    #[serde(rename = "isCancelled", default)]
    pub is_cancelled: Option<bool>,
    /// Numeric meeting class.
    #[serde(rename = "meetingType", default)]
    pub meeting_type: Option<u64>,
    /// Scheduling scenario (observed `meetup-join`).
    #[serde(default)]
    pub scenario: Option<String>,
    /// Owning tenant id.
    #[serde(rename = "tenantId", default)]
    pub tenant_id: Option<String>,
    /// Unknown fields (iCal uid, exchange id, copilot flags land here).
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

/// Compact last-message preview attached to channels, chats and meetings.
///
/// Deliberately **not** [`crate::Message`]: this shape is camelCase
/// (`originalArrivalTime`, `imDisplayName`) where the chat service's message objects are
/// all-lowercase, and it carries a `containerId` instead of a conversation link.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PreviewMessage {
    /// Server message id.
    #[serde(default)]
    pub id: Option<String>,
    /// Conversation this preview belongs to.
    #[serde(rename = "containerId", default)]
    pub container_id: Option<String>,
    /// Sender MRI.
    #[serde(default)]
    pub from: Option<String>,
    /// Sender display name as rendered.
    #[serde(rename = "imDisplayName", default)]
    pub im_display_name: Option<String>,
    /// Sender display name taken from the auth token.
    #[serde(rename = "fromDisplayNameInToken", default)]
    pub from_display_name_in_token: Option<String>,
    /// Message body (HTML or plain text).
    #[serde(default)]
    pub content: Option<String>,
    /// Message classifier (`RichText/Html`, `Text`, `ThreadActivity/*`, …).
    #[serde(rename = "messageType", default)]
    pub message_type: Option<String>,
    /// Arrival time (RFC-3339-like).
    #[serde(rename = "originalArrivalTime", default)]
    pub original_arrival_time: Option<String>,
    /// Compose time.
    #[serde(rename = "composeTime", default)]
    pub compose_time: Option<String>,
    /// Ordering sequence within the conversation.
    #[serde(rename = "sequenceId", default)]
    pub sequence_id: Option<u64>,
    /// Thread class of the conversation (observed `chat`, `space`, `meeting`).
    #[serde(rename = "threadType", default)]
    pub thread_type: Option<String>,
    /// Parent message id for thread replies.
    #[serde(rename = "parentMessageId", default)]
    pub parent_message_id: Option<String>,
    /// Client-generated id (send de-duplication).
    #[serde(rename = "clientMessageId", default)]
    pub client_message_id: Option<String>,
    /// Preview kind (observed `Message`).
    #[serde(rename = "type", default)]
    pub type_name: Option<String>,
    /// Unknown fields.
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

impl Team {
    /// The team's General channel, when this update included it.
    #[must_use]
    pub fn general_channel(&self) -> Option<&Channel> {
        self.channels.iter().find(|channel| channel.is_general.unwrap_or(false))
    }
}

impl RosterUpdate {
    /// Every channel the update knows about, deduplicated by thread id.
    ///
    /// Merges the per-team lists with the top-level [`RosterUpdate::channels`], because the
    /// service splits them: teams only carry General, the rest arrive at the top level.
    /// Entries without an id are kept (nothing to deduplicate on).
    #[must_use]
    pub fn all_channels(&self) -> Vec<&Channel> {
        let mut seen = std::collections::HashSet::new();
        self.teams
            .iter()
            .flat_map(|team| team.channels.iter().chain(team.available_channels.iter()))
            .chain(self.channels.iter())
            .filter(|channel| match channel.id.as_deref() {
                Some(id) => seen.insert(id),
                None => true,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(id: &str, general: bool) -> Channel {
        Channel { id: Some(id.to_owned()), is_general: Some(general), ..Channel::default() }
    }

    /// The service splits channels across two places; the merge must not double-count.
    #[test]
    fn all_channels_merges_and_deduplicates() {
        let roster = RosterUpdate {
            teams: vec![Team {
                id: Some("team1".into()),
                channels: vec![channel("gen1", true)],
                available_channels: vec![channel("extra1", false)],
                ..Team::default()
            }],
            // The same channel reported again at the top level, plus an unidentifiable one.
            channels: vec![channel("gen1", true), channel("top1", false), Channel::default()],
            ..RosterUpdate::default()
        };

        let ids: Vec<Option<&str>> =
            roster.all_channels().into_iter().map(|c| c.id.as_deref()).collect();
        assert_eq!(ids, vec![Some("gen1"), Some("extra1"), Some("top1"), None]);
    }

    #[test]
    fn general_channel_is_found_by_flag_not_position() {
        let team = Team {
            channels: vec![channel("other", false), channel("gen", true)],
            ..Team::default()
        };
        assert_eq!(team.general_channel().and_then(|c| c.id.as_deref()), Some("gen"));

        let empty = Team::default();
        assert!(empty.general_channel().is_none());
    }
}
