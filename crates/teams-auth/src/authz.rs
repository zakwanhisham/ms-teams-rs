//! `POST https://authsvc.teams.microsoft.com/v1.0/authz` — skypetoken + region map.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{AUTHZ_URL, AuthError};

/// Result of a successful authz exchange.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthzSession {
    /// Skype token (JWT) used as `Authentication: skypetoken=…` on the chat service.
    pub skype_token: String,
    /// Discovered region (e.g. `EU`); never hardcode a region.
    #[serde(default)]
    pub region: Option<String>,
    /// Service name → host/base-url map returned by authz.
    ///
    /// Values are kept as [`serde_json::Value`] because the service mixes plain strings
    /// and objects; use [`AuthzSession::host`] for the common string case.
    #[serde(default)]
    pub region_gtms: BTreeMap<String, serde_json::Value>,
    /// `expiresIn` from the response, when provided.
    #[serde(default)]
    pub expires_in: Option<u64>,
    /// Local issuance timestamp.
    #[serde(default)]
    pub obtained_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl AuthzSession {
    /// Base URL for a service name when the entry is a plain string.
    #[must_use]
    pub fn host(&self, service: &str) -> Option<&str> {
        match self.region_gtms.get(service)? {
            serde_json::Value::String(s) => Some(s.as_str()),
            other => other.get("url")?.as_str(),
        }
    }

    /// Service names known to this session, sorted (safe to print — not secrets).
    #[must_use]
    pub fn service_names(&self) -> Vec<&str> {
        self.region_gtms.keys().map(String::as_str).collect()
    }

    /// Whether the skypetoken looks expired based on `expires_in` + issuance.
    #[must_use]
    pub fn is_expired(&self, skew_secs: i64) -> bool {
        let (Some(at), Some(expires)) = (self.obtained_at, self.expires_in) else {
            return false; // unknown lifetime: optimistically usable; a 401 re-runs authz
        };
        chrono::Utc::now() + chrono::Duration::seconds(skew_secs)
            >= at + crate::device_code::secs_to_duration(expires)
    }
}

/// Wire shape of the authz response — parsed as [`serde_json::Value`] first because
/// enterprise and consumer responses differ (see `docs/auth.md`).
#[derive(Debug, Deserialize)]
struct AuthzResponse {
    #[serde(default)]
    tokens: Option<serde_json::Value>,
    #[serde(default)]
    region: Option<String>,
    #[serde(default, rename = "regionGtms")]
    region_gtms: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(default, rename = "skypeToken")]
    skype_token: Option<serde_json::Value>,
    #[serde(default, rename = "expiresIn")]
    expires_in: Option<u64>,
}

/// Client for the Teams authz service.
#[derive(Debug, Clone)]
pub struct AuthzClient {
    http: reqwest::Client,
    url: String,
}

impl Default for AuthzClient {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthzClient {
    /// Client pointed at the production authz endpoint.
    #[must_use]
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .user_agent("ms-teams-rs/0.1 (unofficial Teams client)")
                .build()
                .expect("reqwest client builder only fails on invalid TLS roots"),
            url: AUTHZ_URL.to_owned(),
        }
    }

    /// Exchange a bearer token (audience `api.spaces.skype.com`) for a skypetoken + region map.
    ///
    /// # Errors
    ///
    /// [`AuthError::Authz`] on non-2xx, [`AuthError::AuthzShape`] when the response has no
    /// usable skypetoken, [`AuthError::Http`] on transport failure.
    pub async fn exchange(&self, access_token: &str) -> Result<AuthzSession, AuthError> {
        let resp = self
            .http
            .post(&self.url)
            .bearer_auth(access_token)
            .json(&serde_json::json!({}))
            .send()
            .await?;

        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            return Err(AuthError::Authz {
                status: status.as_u16(),
                body: crate::device_code::truncate(&body, 300),
            });
        }

        let parsed: AuthzResponse = serde_json::from_str(&body)?;
        let skype_token = extract_skype_token(&parsed)
            .ok_or_else(|| AuthError::AuthzShape(crate::device_code::truncate(&body, 300)))?;

        Ok(AuthzSession {
            skype_token,
            region: parsed.region,
            region_gtms: parsed.region_gtms.unwrap_or_default().into_iter().collect(),
            expires_in: parsed.expires_in,
            obtained_at: Some(chrono::Utc::now()),
        })
    }
}

/// Lenient skypetoken extraction covering the two known response shapes:
/// - enterprise: `{ "tokens": { "skypeToken": "<jwt>" }, … }`
/// - consumer:   `{ "skypeToken": { "skypetoken": "<jwt>", "expiresIn": … }, … }`
fn extract_skype_token(parsed: &AuthzResponse) -> Option<String> {
    let from_tokens =
        parsed.tokens.as_ref().and_then(|tokens| tokens.get("skypeToken").and_then(token_string));
    from_tokens.or_else(|| parsed.skype_token.as_ref().and_then(token_string))
}

/// Stringify a token value that is either a plain JWT or `{"skypetoken": …}`.
fn token_string(value: &serde_json::Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.get("skypetoken").and_then(serde_json::Value::as_str).map(str::to_owned))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(json: &serde_json::Value) -> AuthzSession {
        let mut region_gtms = std::collections::BTreeMap::new();
        if let Some(map) = json.get("regionGtms").and_then(serde_json::Value::as_object) {
            region_gtms.extend(map.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        AuthzSession {
            skype_token: "jwt".into(),
            region: json.get("region").and_then(serde_json::Value::as_str).map(str::to_owned),
            region_gtms,
            expires_in: json.get("expiresIn").and_then(serde_json::Value::as_u64),
            obtained_at: Some(chrono::Utc::now()),
        }
    }

    #[test]
    fn host_reads_string_and_object_entries() {
        let session = session(&serde_json::json!({
            "region": "EU",
            "regionGtms": {
                "chatService": "https://EU.ng.msg.teams.microsoft.com",
                "substrate": {"url": "https://substrate.office.com"}
            }
        }));
        assert_eq!(session.host("chatService"), Some("https://EU.ng.msg.teams.microsoft.com"));
        assert_eq!(session.host("substrate"), Some("https://substrate.office.com"));
        assert_eq!(session.host("missing"), None);
        assert_eq!(session.service_names(), vec!["chatService", "substrate"]);
        assert!(!session.is_expired(crate::EXPIRY_SKEW_SECS));
    }

    #[test]
    fn enterprise_shape_extracts_token() {
        let parsed: AuthzResponse = serde_json::from_str(
            r#"{"tokens":{"skypeToken":"aaa.bbb.ccc","expiresIn":86400},
                "region":"EU","regionGtms":{"chatService":"https://x"}}"#,
        )
        .expect("parse");
        assert_eq!(extract_skype_token(&parsed).as_deref(), Some("aaa.bbb.ccc"));
    }

    #[test]
    fn consumer_shape_extracts_token() {
        let parsed: AuthzResponse = serde_json::from_str(
            r#"{"skypeToken":{"skypetoken":"ddd.eee.fff","expiresIn":86400},
                "region":"NA","regionGtms":{}}"#,
        )
        .expect("parse");
        assert_eq!(extract_skype_token(&parsed).as_deref(), Some("ddd.eee.fff"));
    }

    #[test]
    fn missing_token_is_none() {
        let parsed: AuthzResponse = serde_json::from_str(r#"{"region":"EU"}"#).expect("parse");
        assert!(extract_skype_token(&parsed).is_none());
    }
}
