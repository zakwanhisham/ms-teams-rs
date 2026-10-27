//! Authentication for the unofficial Microsoft Teams APIs.
//!
//! Spike-harness implementation of Phase 2A/3 (see `TODO.md`):
//!
//! 1. **Device-code login** (`device_code` module) against the Teams FOCI public client
//!    [`FOCI_CLIENT_ID`] yields an access token plus a family refresh token (FRT).
//! 2. Tokens are persisted in `token_cache` at `$XDG_STATE_HOME/teams-rs/token-cache.json`
//!    (mode `0600`), keyed by audience.
//! 3. `authz` exchanges the `api.spaces.skype.com` bearer token for a **skypetoken**
//!    plus the `region` / `regionGtms` service map.
//!
//! Verified against the live identity platform during the Phase 2A probe (2026-10-27):
//! - `openid offline_access https://api.spaces.skype.com/.default` → device code **accepted**
//! - `https://api.spaces.skype.com/.default` → **accepted**
//! - `00000003-0000-0ff1-ce00-000000000000/.default` → **accepted**
//! - `https://graph.microsoft.com/.default` and delegated `User.Read` → **rejected**
//!   (`AADSTS70002`, "not enabled for consumers") — Graph acquisition needs another path.
//!
//! This crate performs no endpoint calls beyond login/authz — everything else lives in
//! `teams-api` / `teams-realtime` / `teams-call`.

#![forbid(unsafe_code)]

mod authz;
mod device_code;
mod jwt;
mod token_cache;

pub use authz::{AuthzClient, AuthzSession};
pub use device_code::{DeviceCodeClient, DeviceCodeRequest, TokenSet};
pub use jwt::{decode_claims, identity_from_claims};
pub use token_cache::{CachedIdentity, TokenCache};

/// The Microsoft first-party client id used by the Teams clients (FOCI family).
///
/// Confirmed accepting device-code grants for the scopes documented above.
pub const FOCI_CLIENT_ID: &str = "1fec8e78-bce4-4aaf-ab1b-5451cc387264";

/// Base URL of the Microsoft identity platform.
pub const LOGIN_BASE: &str = "https://login.microsoftonline.com";

/// The Teams authz endpoint that mints the skypetoken + region map.
pub const AUTHZ_URL: &str = "https://authsvc.teams.microsoft.com/v1.0/authz";

/// Default scope for the chat audience: verified accepted by the device-code endpoint.
pub const SCOPE_SPACES: &str = "openid offline_access https://api.spaces.skype.com/.default";

/// Audience label used for the `api.spaces.skype.com` token in the cache.
pub const AUDIENCE_SPACES: &str = "spaces.skype";

/// Audience label used for the CSA (chat service aggregator / team roster) token.
pub const AUDIENCE_CSA: &str = "chatsvcagg";

/// Scope that mints a bearer for the CSA roster endpoints
/// (`Authorization: Bearer …` on `teams.microsoft.com/api/csa/…`).
///
/// Same family-refresh-token redemption as `SCOPE_SPACES`; live-verified 2026-10-27.
pub const SCOPE_CSA: &str = "openid offline_access https://chatsvcagg.teams.microsoft.com/.default";

/// Audience label used for the substrate (search / people profile) token.
pub const AUDIENCE_SUBSTRATE: &str = "substrate";

/// Scope that mints a bearer for `https://substrate.office.com` (people search).
///
/// Live-verified 2026-10-27: the FRT redeems it and the resulting token carries
/// `SubstrateSearch-Internal.ReadWrite` among its scopes.
pub const SCOPE_SUBSTRATE: &str = "openid offline_access https://substrate.office.com/.default";

/// Refresh tokens can be redeemed slightly before actual expiry.
pub const EXPIRY_SKEW_SECS: i64 = 300;

/// All authentication failures funnel into this error type.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// Transport-level failure (DNS, TLS, timeout, …).
    #[error("http request failed: {0}")]
    Http(#[from] reqwest::Error),

    /// The Microsoft identity platform rejected a request
    /// (`{"error": …, "error_description": …}`).
    #[error("identity platform error {status}: {error} — {description}")]
    Identity {
        /// HTTP status of the response.
        status: u16,
        /// Machine-readable error code, e.g. `AADSTS`-prefixed descriptions are in `description`.
        error: String,
        /// Human-readable description from the service.
        description: String,
    },

    /// Device-code grant outcome that is not a transient pending state.
    #[error("device code flow: {0}")]
    DeviceCode(String),

    /// The authz service returned a non-success status with a non-JSON or unexpected body.
    #[error("authz exchange failed ({status}): {body}")]
    Authz {
        /// HTTP status.
        status: u16,
        /// Raw response body (truncated) for diagnostics.
        body: String,
    },

    /// The authz response could not be interpreted (missing skypetoken).
    #[error("unexpected authz response: {0}")]
    AuthzShape(String),

    /// Token cache could not be read/written.
    #[error("token cache io: {0}")]
    Io(#[from] std::io::Error),

    /// JSON (de)serialization failure.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// Requested audience is not in the cache.
    #[error("no cached token for audience `{0}`")]
    NoToken(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foci_client_id_is_a_uuid() {
        assert_eq!(FOCI_CLIENT_ID.len(), 36);
        assert_eq!(FOCI_CLIENT_ID.matches('-').count(), 4);
    }

    #[test]
    fn spaces_scope_requests_offline_access() {
        // The FRT (needed for per-audience minting) only comes back with `offline_access`.
        assert!(SCOPE_SPACES.contains("offline_access"));
        assert!(SCOPE_SPACES.contains("api.spaces.skype.com"));
    }
}
