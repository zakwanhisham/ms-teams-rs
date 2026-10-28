//! Realtime event transport for Teams.
//!
//! Phase 5 live-verified parts (see `docs/endpoints.md` "Trouter websocket"):
//!
//! - **Allocator**: `POST {trouter}/v4/a` with the skypetoken header pair returns the
//!   connection shape (`Allocation`) including the V3 registrar URL and signed
//!   `connectparams`.
//! - **Registrar**: `POST {allocation.registrar_url}` registers this endpoint with the
//!   allocated `surl` (202 Accepted, empty body). The only template key seen accepted is
//!   `MicrosoftTeams`, and `product_context` must be serialised as a **JSON string**.
//! - **Codec**: socket.io 0.9 packet encode/decode as spoken by the official trouter
//!   client — server connect `1::`, disconnect `0:::{reason}`, event emits
//!   `5:::{"name":…,"args":[…]}` (object form, not the bare array).
//!
//! The websocket layer that consumes the codec (auth handshake, ping/pong, reconnect)
//! lands next; `RealtimeTransport` stays the shared seam between it and the GUI/CLI.

#![forbid(unsafe_code)]

pub mod alloc;
pub mod codec;
pub mod events;
pub mod registrar;

pub use events::TeamEvent;

/// Crate identity, used by tests until the transport code replaces it.
pub const REALTIME_CRATE: &str = "teams-realtime";

/// Product strings sent in the registration (`clientDescription`) — the live-verified
/// values from the 202 response probe (2026-10-29).
pub const TEAMS_APP_ID: &str = "Microsoft Teams Web";
pub const TEAMS_TEMPLATE_KEY: &str = "MicrosoftTeams";

#[cfg(test)]
mod tests {
    use super::{REALTIME_CRATE, TEAMS_APP_ID, TEAMS_TEMPLATE_KEY};

    #[test]
    fn crate_identity() {
        assert_eq!(REALTIME_CRATE, "teams-realtime");
        assert_eq!(TEAMS_APP_ID, "Microsoft Teams Web");
        assert_eq!(TEAMS_TEMPLATE_KEY, "MicrosoftTeams");
    }
}
