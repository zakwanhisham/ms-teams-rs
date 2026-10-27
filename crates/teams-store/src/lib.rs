//! Local persistence for the Teams client.
//!
//! SQLite (bundled, no system dependency) storing:
//! - `conversations`, `messages`, `users`, `reactions` — cached history for instant loads
//! - `sync_cursor` — the realtime resume watermark
//! - `pending_outbox` — optimistic sends awaiting server acknowledgement / retry
//!
//! Schema changes go through explicit migrations (Phase 5). The database is a **cache**:
//! wiping it must never lose data that only exists on the server.

#![forbid(unsafe_code)]

/// Placeholder so the crate has a symbol to document/test against.
///
/// Replaced by `Store` + migrations in Phase 5.
pub const STORE_CRATE: &str = "teams-store";

#[cfg(test)]
mod tests {
    use super::STORE_CRATE;

    #[test]
    fn crate_identity() {
        assert_eq!(STORE_CRATE, "teams-store");
    }
}
