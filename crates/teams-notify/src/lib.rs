//! Linux desktop integration.
//!
//! Planned contents (Phase 7):
//! - `org.freedesktop.Notifications` via `zbus` (GNOME/KDE/Sway) with Reply/Open actions
//!   and a Do-Not-Disturb guard; high-priority **incoming-call** notifications with
//!   Answer(video)/Answer(audio)/Decline actions
//! - Tray icon + unread badge, in-call indicator, status menu
//! - MPRIS for media keys during calls
//! - Single-instance lock in `$XDG_RUNTIME_DIR` and `x-scheme-handler/ms-teams` deep links
//!
//! Protocol work (zbus proxy definitions) lands in Phase 7; only error types live here now.

#![forbid(unsafe_code)]

/// Placeholder so the crate has a symbol to document/test against.
///
/// Replaced by the notification/tray/MPRIS implementation in Phase 7.
pub const NOTIFY_CRATE: &str = "teams-notify";

/// Freedesktop notifications bus name.
pub const NOTIFICATIONS_BUS: &str = "org.freedesktop.Notifications";

#[cfg(test)]
mod tests {
    use super::{NOTIFICATIONS_BUS, NOTIFY_CRATE};

    #[test]
    fn crate_identity() {
        assert_eq!(NOTIFY_CRATE, "teams-notify");
    }

    #[test]
    fn notifications_bus_name() {
        assert_eq!(NOTIFICATIONS_BUS, "org.freedesktop.Notifications");
    }
}
