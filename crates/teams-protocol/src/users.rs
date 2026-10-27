//! Sender/participant identity assembled from the wire's loose user fields.
//!
//! The chat surface never ships a structured user object alongside messages — identity
//! arrives scattered: an MRI in `from`, a display name in `imdisplayname` (or the
//! token-inlined `from*NameInToken` trio), and a tenant id on single-message reads.
//! [`User`] binds those pieces (as observed live; see the message-read notes in
//! `docs/endpoints.md`) so the GUI and CLI have one representation for rendering.
//! Structured user objects from other surfaces (CSA roster, substrate search) have
//! their own wire types and stay where they belong.

/// One person's identity as the message wire describes it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct User {
    /// Sender MRI, e.g. `8:orgid:<guid>` (the chat surface's `from` field).
    pub mri: Option<String>,
    /// Display name as rendered in chat (`imdisplayname`, falling back to the
    /// token-inlined `fromDisplayNameInToken`).
    pub display_name: Option<String>,
    /// Given name from the auth token, when the service inlined it.
    pub given_name: Option<String>,
    /// Family name from the auth token, when the service inlined it.
    pub family_name: Option<String>,
    /// Tenant id, when the read exposes one (single-message reads do).
    pub tenant_id: Option<String>,
}

impl User {
    /// Assemble the sender of `message`. Every field is optional — partial identity
    /// is normal on pooled surfaces (`lastMessage` previews carry less than pages).
    #[must_use]
    pub fn sender_of(message: &crate::Message) -> Self {
        Self {
            mri: message.from.clone(),
            display_name: message
                .im_display_name
                .clone()
                .or_else(|| message.from_display_name_in_token.clone()),
            given_name: message.from_given_name_in_token.clone(),
            family_name: message.from_family_name_in_token.clone(),
            tenant_id: message.from_tenant_id.clone(),
        }
    }

    /// The best display name available, empty when none shipped.
    #[must_use]
    pub fn display_name_or_placeholder(&self) -> &str {
        self.display_name.as_deref().unwrap_or("<unknown>")
    }
}
