//! Realtime event transport for Teams.
//!
//! Two transports behind one `RealtimeTransport` trait (see `TODO.md` Phase 5):
//!
//! - **Trouter** (`wss://go.trouter.io/v2/a`) — *primary*: websocket, required for call
//!   signalling anyway, so it is built first.
//! - **HTTP long-poll** — *fallback*: endpoint registration
//!   (`POST /v1/users/ME/endpoints/{id}/active`, ~45 s keepalive) + subscriptions with
//!   `channelType: httpLongPoll` and `POST …/subscriptions/{n}/poll`.
//!
//! Responsibilities: connection lifecycle with jittered backoff, resume-from-cursor (a
//! persisted event watermark), and decoding raw frames into typed `TeamEvent`s —
//! including `IncomingCall`, which the call stack consumes.

#![forbid(unsafe_code)]

/// Placeholder so the crate has a symbol to document/test against.
///
/// Replaced by `RealtimeTransport` / `TeamEvent` in Phase 5.
pub const REALTIME_CRATE: &str = "teams-realtime";

/// Trouter websocket endpoint (verify exact URL + subprotocol in the Phase 2B spike).
pub const TROUTER_URL: &str = "https://go.trouter.io/v2/a";

#[cfg(test)]
mod tests {
    use super::{REALTIME_CRATE, TROUTER_URL};

    #[test]
    fn crate_identity() {
        assert_eq!(REALTIME_CRATE, "teams-realtime");
    }

    #[test]
    fn trouter_url_is_https() {
        assert!(TROUTER_URL.starts_with("https://"));
    }
}
