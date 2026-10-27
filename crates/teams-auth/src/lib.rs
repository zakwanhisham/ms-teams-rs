//! Authentication for the unofficial Microsoft Teams APIs.
//!
//! Flow (see `TODO.md` Phase 2A/3):
//!
//! 1. **Device-code login** against the Teams FOCI public client
//!    (`1fec8e78-bce4-4aaf-ab1b-5451cc387264`) yields a family refresh token (FRT).
//! 2. The FRT is redeemed per-audience (`api.spaces.skype.com`,
//!    `chatsvcagg.teams.microsoft.com`, `substrate.office.com`, `graph.microsoft.com`,
//!    `go.trouter.io`, …).
//! 3. `POST https://authsvc.teams.microsoft.com/v1.0/authz` with a bearer token exchanges
//!    it for a **skypetoken** plus the `region` / `regionGtms` service map.
//!
//! Tokens are cached under `$XDG_STATE_HOME/teams-rs/token-cache.json` with mode `0600`
//! (OS keyring behind a `CredentialStore` trait is a later option).
//!
//! This crate performs no endpoint calls beyond auth — everything else lives in
//! `teams-api` / `teams-realtime` / `teams-call`.

#![forbid(unsafe_code)]

/// Placeholder so the crate has a symbol to document/test against.
///
/// Replaced by `DeviceCodeFlow` / `TokenCache` / `AuthzClient` in Phase 3.
pub const AUTH_CRATE: &str = "teams-auth";

/// The Microsoft first-party client id used by the Teams clients (FOCI family).
///
/// Source: `AADInternals` / `xteams` / reverse engineering — verify in the Phase 2A spike.
pub const FOCI_CLIENT_ID: &str = "1fec8e78-bce4-4aaf-ab1b-5451cc387264";

/// Base URL of the Microsoft identity platform.
pub const LOGIN_BASE: &str = "https://login.microsoftonline.com";

/// The Teams authz endpoint that mints the skypetoken + region map.
pub const AUTHZ_URL: &str = "https://authsvc.teams.microsoft.com/v1.0/authz";

#[cfg(test)]
mod tests {
    use super::{AUTH_CRATE, FOCI_CLIENT_ID};

    #[test]
    fn crate_identity() {
        assert_eq!(AUTH_CRATE, "teams-auth");
    }

    #[test]
    fn foci_client_id_is_a_uuid() {
        assert_eq!(FOCI_CLIENT_ID.len(), 36);
        assert_eq!(FOCI_CLIENT_ID.matches('-').count(), 4);
    }
}
