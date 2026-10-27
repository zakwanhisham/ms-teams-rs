//! Deserialization tests for the CSA roster fixture (`tests/fixtures/roster.json`).
//!
//! The fixture is a sanitized slice of a live `users/me/updates` response (2026-10-27):
//! two teams (each with its General channel), two chats (one meeting, one 1:1), the one
//! channel reported at the top level, and two private feeds. Every identity, name, URL and
//! timestamp is `<redacted>` or zeroed — see `tools/sanitize_fixture.py`.

use teams_protocol::{RosterUpdate, Team};

const FIXTURE: &str = include_str!("fixtures/roster.json");

fn roster() -> RosterUpdate {
    serde_json::from_str(FIXTURE).expect("roster fixture deserializes")
}

#[test]
fn fixture_has_the_expected_shape() {
    let roster = roster();
    assert_eq!(roster.teams.len(), 2, "two teams kept");
    assert_eq!(roster.chats.len(), 2, "one meeting + one chat kept");
    assert_eq!(roster.channels.len(), 1, "one channel outside the teams");
    assert_eq!(roster.private_feeds.len(), 2, "notification streams kept raw");
    assert!(roster.users.is_empty(), "the users array was empty live");
}

#[test]
fn metadata_records_sync_state() {
    let meta = &roster().metadata;
    assert_eq!(meta.sync_token.as_deref(), Some("<redacted>"), "opaque cursor survives");
    assert_eq!(meta.forward_sync_token, None, "no forward cursor on a complete update");
    assert_eq!(meta.has_more_chats, Some(false));
    assert_eq!(meta.is_partial_data, Some(false), "full, not a prefetch");
}

#[test]
fn team_typed_fields_and_general_channel() {
    let roster = roster();
    let team: &Team = &roster.teams[0];

    assert_eq!(team.id.as_deref(), Some("<redacted>"));
    assert_eq!(team.display_name.as_deref(), Some("<redacted>"));
    assert_eq!(team.member_role, Some(1));
    assert_eq!(team.member_role_string.as_deref(), Some("user"));
    assert_eq!(team.is_archived, Some(false));
    assert_eq!(team.is_tenant_wide, Some(false));
    assert_eq!(team.is_favorite, Some(true));
    // ms-epoch stamps are zeroed by the sanitizer, so only presence is assertable.
    assert!(team.roster_version.is_some());

    let summary = team.membership_summary.as_ref().expect("membershipSummary present");
    assert_eq!(summary.total_member_count, Some(66));
    assert_eq!(summary.user_role_count, Some(64));
    assert_eq!(summary.admin_role_count, Some(2));
    assert_eq!(summary.bot_count, Some(2));
    assert_eq!(summary.muted_members_count, Some(0));

    // The General-only caveat: the team carries exactly its General channel.
    assert_eq!(team.channels.len(), 1);
    let general = team.general_channel().expect("general channel present");
    assert_eq!(general.is_general, Some(true));
    assert_eq!(general.thread_type.as_deref(), Some("space"));
    assert_eq!(general.thread_sub_type.as_deref(), Some(""));
    assert_eq!(general.channel_type, Some(0));
    assert_eq!(general.is_member, Some(true));
    assert_eq!(general.is_message_read, Some(true));
    // Every id is redacted in the fixture, so the *link* can only be asserted as present:
    // live, `parentTeamId` equals the owning team's `id`.
    assert_eq!(general.parent_team_id.as_deref(), Some("<redacted>"));

    // Settings blobs are untyped and must survive in the catch-all.
    assert!(team.extra.contains_key("teamGuestSettings"));
    assert!(team.extra.contains_key("teamSettings"));
    assert!(team.extra.contains_key("teamStatus"));
}

#[test]
fn top_level_channel_carries_its_parent_team() {
    let roster = roster();
    let channel = &roster.channels[0];

    assert_eq!(channel.is_general, Some(false), "a non-General channel");
    assert_eq!(channel.parent_team_id.as_deref(), Some("<redacted>"));
    assert_eq!(channel.is_favorite, Some(true));
    assert_eq!(channel.group_id.as_deref(), Some("<redacted>"));
    assert!(channel.tenant_id.is_some(), "only top-level channels carry tenantId");
}

#[test]
fn chats_cover_a_meeting_and_a_direct_chat() {
    let roster = roster();
    let (meeting, chat) = (&roster.chats[0], &roster.chats[1]);

    assert_eq!(meeting.chat_type.as_deref(), Some("meeting"));
    assert_eq!(meeting.thread_type.as_deref(), Some("meeting"));
    assert_eq!(
        meeting.chat_sub_type,
        Some(serde_json::json!(1)),
        "chatSubType is an integer on the wire"
    );
    assert!(meeting.meeting_information.is_some(), "meetings carry meeting info");

    assert_eq!(chat.chat_type.as_deref(), Some("chat"));
    assert_eq!(chat.thread_type.as_deref(), Some("chat"));
    assert_eq!(chat.chat_sub_type, Some(serde_json::json!(0)));
    assert!(chat.meeting_information.is_none(), "plain chats carry none");
    assert_eq!(chat.title, None, "1:1 chats have no title");
    assert_eq!(chat.hidden, Some(true));
    assert_eq!(chat.is_read, Some(true));
    // Conversation ids take the chat service's form, so they feed `list_messages` directly.
    assert_eq!(meeting.id.as_deref(), Some("<redacted>"));
    assert_eq!(chat.id.as_deref(), Some("<redacted>"));
}

#[test]
fn meeting_information_exposes_join_details() {
    let meeting = &roster().chats[0];
    let info = meeting.meeting_information.as_ref().expect("meeting info");

    // Sanitized (they embed the tenant id), but present and typed.
    assert_eq!(info.meeting_join_url.as_deref(), Some("<redacted>"));
    assert_eq!(info.start_time.as_deref(), Some("<redacted>"));
    assert_eq!(info.end_time.as_deref(), Some("<redacted>"));
    assert_eq!(info.organizer_id.as_deref(), Some("<redacted>"));
    assert_eq!(info.is_cancelled, Some(false));
    assert_eq!(info.meeting_type, Some(2));
    assert_eq!(info.scenario.as_deref(), Some(""));
    assert!(info.extra.contains_key("iCalUid"), "untyped scheduling fields kept");
}

#[test]
fn chat_members_are_typed() {
    let meeting = &roster().chats[0];
    assert_eq!(meeting.members.len(), 2, "array limit keeps two members");

    let member = &meeting.members[0];
    assert_eq!(member.mri.as_deref(), Some("<redacted>"));
    assert_eq!(member.object_id.as_deref(), Some("<redacted>"));
    assert_eq!(member.role.as_deref(), Some("Admin"));
    assert_eq!(member.is_muted, Some(false));
    assert_eq!(member.is_identity_masked, Some(false));
    assert!(meeting.members[1].extra.contains_key("shareHistoryTime"), "per-member extras survive");
}

#[test]
fn previews_use_the_camel_case_shape() {
    let roster = roster();
    let preview = roster.channels[0].last_message.as_ref().expect("channel preview");

    assert_eq!(preview.id.as_deref(), Some("1793068074661"));
    assert_eq!(preview.sequence_id, Some(115));
    assert_eq!(preview.message_type.as_deref(), Some("RichText/Html"));
    assert_eq!(preview.type_name.as_deref(), Some("Message"));
    assert_eq!(preview.im_display_name.as_deref(), Some("<redacted>"));
    assert_eq!(preview.parent_message_id, preview.id.clone(), "a root preview points at itself");
    assert!(preview.extra.contains_key("isEscalationToNewPerson"), "untyped preview fields kept");

    // A thread reply's preview points at a different root.
    let reply = roster.teams[0].channels[0].last_message.as_ref().expect("general channel preview");
    assert_eq!(reply.sequence_id, Some(741));
    assert_eq!(reply.parent_message_id.as_deref(), Some("1793058838818"));

    // The meeting chat has no messages yet: an all-null preview, not an absent one.
    let empty = roster.chats[0].last_message.as_ref().expect("empty preview present");
    assert_eq!(empty.id, None);
    assert_eq!(empty.sequence_id, Some(0));
}

#[test]
fn unknown_fields_are_preserved_at_every_level() {
    let roster = roster();
    assert!(roster.extra.is_empty(), "all six root keys are typed — nothing spills over");
    assert!(roster.metadata.extra.is_empty(), "metadata has no unknown keys");
    assert!(roster.chats[1].extra.contains_key("quickReplyAugmentation"));
    assert!(roster.channels[0].extra.contains_key("tabs"));
}

#[test]
fn roundtrip_is_lossless() {
    let roster = roster();
    let again = serde_json::to_string(&roster).expect("serialize");
    let back: RosterUpdate = serde_json::from_str(&again).expect("reparse");
    assert_eq!(roster, back);
}
