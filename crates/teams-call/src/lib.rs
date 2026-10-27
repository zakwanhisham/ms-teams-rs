//! Teams audio/video calls and meetings.
//!
//! This crate is gated on the Phase 2B **go/no-go** spike (see `TODO.md`):
//! until `docs/calls.md` exists with a GO verdict, only scaffolding lives here.
//!
//! Planned contents (Phase 8):
//! - `CallSession` state machine:
//!   `Idle → Outgoing → Ringing → Connecting → Connected → Reconnecting → Ended`
//! - Trouter-based signalling (offer/answer/ICE/hangup message catalogue)
//! - WebRTC media layer (stack decided in the spike: `str0m` vs `webrtc-rs` vs libwebrtc FFI)
//! - Device I/O: `cpal` audio, PipeWire/V4L2 video, portal screencast for screen share
//!
//! The WebRTC/audio/video dependencies are deliberately **not** declared yet — they are
//! chosen by the spike, not on paper.

#![forbid(unsafe_code)]

/// Placeholder so the crate has a symbol to document/test against.
///
/// Replaced by `CallSession` etc. in Phase 8 (M5a).
pub const CALL_CRATE: &str = "teams-call";

/// Meeting deep-link scheme registered in Phase 7 (`x-scheme-handler/ms-teams`).
pub const MEETING_SCHEME: &str = "ms-teams";

#[cfg(test)]
mod tests {
    use super::{CALL_CRATE, MEETING_SCHEME};

    #[test]
    fn crate_identity() {
        assert_eq!(CALL_CRATE, "teams-call");
    }

    #[test]
    fn deep_link_scheme() {
        assert_eq!(MEETING_SCHEME, "ms-teams");
    }
}
