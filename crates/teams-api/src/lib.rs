//! Typed HTTP clients for the unofficial Microsoft Teams APIs.
//!
//! Services covered (see `TODO.md` Phase 4):
//!
//! | Service | Base URL source | Auth header |
//! |---------|-----------------|-------------|
//! | Chat (IC3 / MSNP24) | `regionGtms.chatService` from authz | `Authentication: skypetoken=<jwt>` |
//! | Roster (CSA) | `teams.microsoft.com/api/csa/{region}/…` | `Authorization: Bearer <jwt>` |
//! | People search (substrate) | `substrate.office.com/search/…` | `Authorization: Bearer <jwt>` |
//! | Calendar / mail (Graph) | `graph.microsoft.com/v1.0` | `Authorization: Bearer <jwt>` |
//! | Files (ASM) | `api.asm.skype.com` | `Authorization: skypetoken <jwt>` |
//!
//! Rules: base URLs always come from the authz `regionGtms` map (never hardcoded regions),
//! retries use jittered exponential backoff with `Retry-After` support, and every failure
//! maps into `ApiError`.

#![forbid(unsafe_code)]

/// Placeholder so the crate has a symbol to document/test against.
///
/// Replaced by `ApiClient` / `ChatClient` in Phase 4.
pub const API_CRATE: &str = "teams-api";

/// Chat service header name. Note: `Authentication`, **not** `Authorization`.
pub const SKYPETOKEN_HEADER: &str = "Authentication";

#[cfg(test)]
mod tests {
    use super::{API_CRATE, SKYPETOKEN_HEADER};

    #[test]
    fn crate_identity() {
        assert_eq!(API_CRATE, "teams-api");
    }

    #[test]
    fn skypetoken_header_is_not_authorization() {
        assert_eq!(SKYPETOKEN_HEADER, "Authentication");
    }
}
