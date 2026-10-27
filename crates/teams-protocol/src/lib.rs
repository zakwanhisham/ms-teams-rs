//! Wire types for the unofficial Microsoft Teams APIs.
//!
//! This crate is intentionally **transport-free**: it contains only the data
//! models shared by `teams-auth`, `teams-api`, `teams-realtime` and `teams-call`, plus the
//! parsing of Teams' wire HTML into a renderable `richtext` AST.
//!
//! Design rules (see `TODO.md` Phase 4):
//! - Every deserializer must tolerate unknown fields — the API changes under us.
//! - Never fail to represent a message; degrade to plain text instead.
//! - No secrets, no HTTP, no I/O in this crate.

#![forbid(unsafe_code)]

mod conversations;
mod messages;
mod roster;
mod search;

pub use conversations::{
    Conversation, ConversationList, ConversationProperties, MemberProperties, SyncMetadata,
    ThreadProperties,
};
pub use messages::{Emotion, EmotionUser, Mention, Message, MessagePage, MessageProperties};
pub use roster::{
    Channel, ChatMember, ChatSummary, MeetingInformation, MembershipSummary, PreviewMessage,
    RosterMetadata, RosterUpdate, Team,
};
pub use search::{
    BestMatchHint, LayoutHints, PersonSuggestion, SearchInstrumentation, SearchSuggestions,
    SuggestionsGroup,
};

/// Placeholder so the crate has a symbol to document/test against.
///
/// Removed as soon as the first real wire types land in Phase 4.
pub const PROTOCOL_CRATE: &str = "teams-protocol";

#[cfg(test)]
mod tests {
    use super::PROTOCOL_CRATE;

    #[test]
    fn crate_identity() {
        assert_eq!(PROTOCOL_CRATE, "teams-protocol");
    }
}
