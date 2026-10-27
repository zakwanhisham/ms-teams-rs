//! Device-code OAuth 2.0 flow against the Microsoft identity platform.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{AuthError, LOGIN_BASE};

/// Response of `POST /{tenant}/oauth2/v2.0/devicecode`.
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceCodeRequest {
    /// Opaque blob to poll with; must never be logged or committed.
    pub device_code: String,
    /// Short code shown to the user.
    pub user_code: String,
    /// URL the user opens to enter the code.
    pub verification_uri: String,
    /// Pre-filled URL including the code, when the service provides one.
    #[serde(default)]
    pub verification_uri_complete: Option<String>,
    /// Lifetime of the code, in seconds.
    pub expires_in: u64,
    /// Minimum polling interval, in seconds.
    pub interval: u64,
    /// Service-provided instructions for the user.
    #[serde(default)]
    pub message: String,
}

impl DeviceCodeRequest {
    /// The "open this URL" line for the terminal, preferring the code-complete variant.
    #[must_use]
    pub fn login_hint(&self) -> String {
        match &self.verification_uri_complete {
            Some(complete) => complete.clone(),
            None => format!("{} and enter code {}", self.verification_uri, self.user_code),
        }
    }
}

/// A token set as returned by the token endpoint.
///
/// `obtained_at` is stamped locally (it is not part of the wire response) so expiry can be
/// computed without trusting `expires_in` alone.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenSet {
    /// JWT bearer for the requested audience.
    pub access_token: String,
    /// Family refresh token (present because we always request `offline_access`).
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Token type, normally `Bearer`.
    pub token_type: String,
    /// Lifetime in seconds as advertised at issuance.
    pub expires_in: u64,
    /// Scopes actually granted.
    #[serde(default)]
    pub scope: String,
    /// Id token (OIDC claims) when `openid` was requested.
    #[serde(default)]
    pub id_token: Option<String>,
    /// Local issuance timestamp, set by [`TokenSet::stamp`].
    #[serde(default)]
    pub obtained_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl TokenSet {
    /// Fill in the local issuance timestamp after deserialization.
    pub fn stamp(&mut self) {
        self.obtained_at = Some(chrono::Utc::now());
    }

    /// Advertised lifetime as a [`chrono::Duration`], saturating on absurd values.
    fn lifetime(&self) -> chrono::Duration {
        secs_to_duration(self.expires_in)
    }

    /// Absolute expiry derived from issuance time, falling back to "now + lifetime".
    #[must_use]
    pub fn expires_at(&self) -> chrono::DateTime<chrono::Utc> {
        self.obtained_at.unwrap_or_else(chrono::Utc::now) + self.lifetime()
    }

    /// Whether the access token is expired (or about to expire by `skew_secs`).
    #[must_use]
    pub fn is_expired(&self, skew_secs: i64) -> bool {
        let Some(at) = self.obtained_at else {
            return true; // never stamped: treat as expired (defensive default)
        };
        chrono::Utc::now() + chrono::Duration::seconds(skew_secs) >= at + self.lifetime()
    }
}

/// Convert wire `expiresIn`-style seconds to a duration without panicking on overflow.
pub(crate) fn secs_to_duration(secs: u64) -> chrono::Duration {
    chrono::Duration::seconds(i64::try_from(secs).unwrap_or(i64::MAX))
}

/// Microsoft identity platform (`v2.0`) device-code + refresh grants.
#[derive(Debug, Clone)]
pub struct DeviceCodeClient {
    http: reqwest::Client,
    /// Tenant path segment: `common`, `organizations`, `consumers` or a tenant id.
    tenant: String,
    /// OAuth client id (FOCI family client for Teams).
    client_id: String,
}

impl DeviceCodeClient {
    /// Create a client for `tenant` (defaults applied by callers via [`crate::FOCI_CLIENT_ID`]).
    #[must_use]
    pub fn new(tenant: impl Into<String>, client_id: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .user_agent("ms-teams-rs/0.1 (unofficial Teams client)")
                .build()
                .expect("reqwest client builder only fails on invalid TLS roots"),
            tenant: tenant.into(),
            client_id: client_id.into(),
        }
    }

    fn token_url(&self) -> String {
        format!("{}/{}/oauth2/v2.0/token", LOGIN_BASE, self.tenant)
    }

    fn devicecode_url(&self) -> String {
        format!("{}/{}/oauth2/v2.0/devicecode", LOGIN_BASE, self.tenant)
    }

    /// Step 1: request a user code for `scope`.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError::Identity`] when the client id/scope combination is rejected
    /// (e.g. `AADSTS70002` for Graph scopes) or [`AuthError::Http`] on transport failure.
    pub async fn request_code(&self, scope: &str) -> Result<DeviceCodeRequest, AuthError> {
        let resp = self
            .http
            .post(self.devicecode_url())
            .form(&[("client_id", self.client_id.as_str()), ("scope", scope)])
            .send()
            .await?;

        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            return Err(identity_error(status, &body));
        }
        Ok(serde_json::from_str(&body)?)
    }

    /// Step 2: poll the token endpoint until the user completes the flow or the code dies.
    ///
    /// Handles `authorization_pending`, `slow_down` (interval + 5 s) and terminal errors.
    ///
    /// # Errors
    ///
    /// [`AuthError::DeviceCode`] on decline/expiry, [`AuthError::Identity`] on other
    /// grant failures, [`AuthError::Http`] on transport failure.
    pub async fn poll_for_token(&self, request: &DeviceCodeRequest) -> Result<TokenSet, AuthError> {
        let mut interval = Duration::from_secs(request.interval.max(5));
        let deadline = chrono::Utc::now() + secs_to_duration(request.expires_in);

        loop {
            tokio::time::sleep(interval).await;
            if chrono::Utc::now() >= deadline {
                return Err(AuthError::DeviceCode("authorization code expired".into()));
            }

            let resp = self
                .http
                .post(self.token_url())
                .form(&[
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ("client_id", self.client_id.as_str()),
                    ("device_code", request.device_code.as_str()),
                ])
                .send()
                .await?;

            let status = resp.status();
            let body = resp.text().await?;
            if status.is_success() {
                let mut tokens: TokenSet = serde_json::from_str(&body)?;
                tokens.stamp();
                return Ok(tokens);
            }

            match parse_error_code(&body).as_deref() {
                Some("authorization_pending") => {}
                Some("slow_down") => {
                    interval += Duration::from_secs(5);
                }
                Some("authorization_declined") => {
                    return Err(AuthError::DeviceCode("authorization declined".into()));
                }
                Some("expired_token") => {
                    return Err(AuthError::DeviceCode("authorization code expired".into()));
                }
                _ => return Err(identity_error(status, &body)),
            }
        }
    }

    /// Redeem the refresh token for `scope` (per-audience minting / silent refresh).
    ///
    /// # Errors
    ///
    /// [`AuthError::Identity`] when the refresh token is rejected (evict and re-login),
    /// [`AuthError::Http`] on transport failure.
    pub async fn refresh(&self, refresh_token: &str, scope: &str) -> Result<TokenSet, AuthError> {
        let resp = self
            .http
            .post(self.token_url())
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", self.client_id.as_str()),
                ("refresh_token", refresh_token),
                ("scope", scope),
            ])
            .send()
            .await?;

        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            return Err(identity_error(status, &body));
        }
        let mut tokens: TokenSet = serde_json::from_str(&body)?;
        tokens.stamp();
        Ok(tokens)
    }
}

/// Extract the `error` field from an error response body, if it is JSON.
fn parse_error_code(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("error")?.as_str().map(str::to_owned))
}

/// Build an [`AuthError::Identity`] from a non-2xx body.
fn identity_error(status: reqwest::StatusCode, body: &str) -> AuthError {
    let value: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    AuthError::Identity {
        status: status.as_u16(),
        error: value
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown_error")
            .to_owned(),
        description: value
            .get("error_description")
            .and_then(serde_json::Value::as_str)
            .map_or_else(|| truncate(body, 300), str::to_owned),
    }
}

/// Truncate a string to `max` characters, appending an ellipsis marker.
pub(crate) fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_expiry_math() {
        let mut token = TokenSet {
            access_token: "jwt".into(),
            refresh_token: Some("frt".into()),
            token_type: "Bearer".into(),
            expires_in: 3600,
            scope: "s".into(),
            id_token: None,
            obtained_at: None,
        };
        // Unstamped tokens must be treated as expired (defensive default).
        assert!(token.is_expired(300));
        token.stamp();
        assert!(!token.is_expired(300));
        assert!(token.expires_at() > chrono::Utc::now());
    }

    #[test]
    fn error_body_parsing_is_lenient() {
        assert_eq!(
            parse_error_code(r#"{"error":"authorization_pending"}"#).as_deref(),
            Some("authorization_pending")
        );
        assert_eq!(parse_error_code("not json").as_deref(), None);
        assert_eq!(parse_error_code("{}").as_deref(), None);
    }

    #[test]
    fn identity_error_falls_back_to_body() {
        let err = identity_error(reqwest::StatusCode::UNAUTHORIZED, "<html>gateway</html>");
        let AuthError::Identity { status, error, .. } = err else {
            panic!("expected AuthError::Identity");
        };
        assert_eq!(status, 401);
        assert_eq!(error, "unknown_error");
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        let s = "héllo wörld"; // multi-byte chars
        let out = truncate(s, 3);
        assert!(out.ends_with('…'));
        assert!(out.chars().count() <= 4);
    }
}
