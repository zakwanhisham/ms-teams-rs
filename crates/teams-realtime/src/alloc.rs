//! Trouter allocator: `POST {trouter}/v4/a`.
//!
//! Live-verified 2026-10-29 (`docs/endpoints.md`): bare headers `X-Skypetoken` (skypetoken
//! itself, no `Bearer`) + `X-MS-Migration: True`, `Content-Type: text/plain`, **empty
//! body**, and query `cor_id`, `con_num` (`<clientID>_<attempt>`), `tc` (URL-encoded JSON
//! `{ua, v}`), `timeout` (seconds). Response is the [`Allocation`].

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// Everything the server returns on a successful allocation. Lenient on purpose: any
/// field may gain server-side variants we have not seen; only the fields needed to
/// operate are accessed, unknown ones ride along.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Allocation {
    /// Client-correlation id minted by the server (`e-…`). Needed as the connect `ccid`.
    #[serde(default)]
    pub ccid: String,
    /// Session id (`<clientID?>_<ccid>` after the first underscore).
    #[serde(default)]
    pub id: String,
    /// Base URL of the socket.io gateway these events stream from (trailing `/`).
    #[serde(default)]
    pub socketio: String,
    /// Per-connection frontier path (`https://…/v4/f/<session-token>/`). The TROUTER
    /// registration must reference exactly this value, and the long-poll fallback
    /// POSTs read from it.
    #[serde(default)]
    pub surl: String,
    /// Alternate frontier URL (different port) for the same session.
    #[serde(default)]
    pub url: String,
    /// Session lifetime in **seconds** (arrives as a JSON string, ~83 000).
    #[serde(default)]
    pub ttl: String,
    /// Health-check endpoint for the gateway.
    #[serde(default)]
    pub health_url: String,
    /// Base URL for the c2c (client-to-client / long-poll) requests.
    #[serde(default)]
    pub curlb: String,
    /// Registrar that owns this allocation (`…/registrar/prod/V3/registrations`).
    #[serde(default)]
    pub registrar_url: String,
    /// Signed connection parameters bound to this session (`sig`, `se` epoch-ms, …).
    #[serde(default)]
    pub connectparams: ConnectParams,
}

/// Raw structural error body from the allocator (HTTP ≥ 400).
#[derive(Debug, Clone, PartialEq, Error)]
#[error("allocate failed with HTTP {0}")]
pub struct AllocateError(pub u16);

/// The signed web-token-ish parameters echoed back per session.
///
/// Typed only for the fields observed live; unknown fields tolerated.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectParams {
    /// Same value as `id` (session id).
    #[serde(default)]
    pub sr: String,
    /// Empty string observed; meaning unverified.
    #[serde(default)]
    pub issuer: String,
    /// Gateway node short name (`pub-csm-…-00`).
    #[serde(default)]
    pub sp: String,
    /// Epoch-ms expiry of `sig` (string).
    #[serde(default)]
    pub se: String,
    /// Epoch-ms issue time of `sig` (string).
    #[serde(default)]
    pub st: String,
    /// Protocol selector; `"1"` = v4c socket.io transport.
    #[serde(default)]
    pub scae: String,
    /// Signature over the above.
    #[serde(default)]
    pub sig: String,
}

/// Errors while allocating.
#[derive(Debug, Error)]
pub enum AllocatorError {
    /// Non-success HTTP status (no body observed on any failure so far).
    #[error("allocate rejected: HTTP {0}")]
    Status(u16),
    /// Body could not be deserialized as an allocation.
    #[error("allocation response unparseable: {0}")]
    Shape(String),
    /// The allocator URL itself failed to parse.
    #[error("invalid allocator URL: {0}")]
    Url(String),
    /// HTTP client failure.
    #[error("allocation request failed: {0}")]
    Http(#[from] reqwest::Error),
}

/// Selections for the allocation query.
#[derive(Debug, Clone, Serialize)]
pub struct ClientInfo<'a> {
    /// User-agent-ish product name (`tc` JSON `ua`).
    pub ua: &'a str,
    /// Client version string (`tc` JSON `v`).
    pub v: &'a str,
}

/// Client for the allocate call.
#[derive(Debug)]
pub struct Allocator {
    /// `https://host/v4/a` — base **URLs are data**, caller picks the region-specific one.
    url: String,
    http: reqwest::Client,
}

impl Allocator {
    /// New allocator for an exact `POST /v4/a` URL (typically `regionGtms`-derived).
    pub fn new(url: impl Into<String>, http: reqwest::Client) -> Self {
        Self { url: url.into(), http }
    }

    /// Perform an allocation.
    ///
    /// `client_id`/`connection_attempt` become `con_num`; `cor_id` is the fresh
    /// correlation UUID; `timeout` is in seconds. The skypetoken is passed bare in the
    /// `X-Skypetoken` header (the `Authentication: skypetoken=…` shape observed a 401).
    pub async fn allocate(
        &self,
        token: &str,
        client_info: &ClientInfo<'_>,
        // `&str` fields so callers can reuse leased ids.
        client_id: &str,
        connection_attempt: u32,
        timeout_secs: u32,
    ) -> Result<Allocation, AllocatorError> {
        let mut url =
            reqwest::Url::parse(&self.url).map_err(|e| AllocatorError::Url(e.to_string()))?;
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("cor_id", &Uuid::new_v4().to_string());
            q.append_pair("con_num", &format!("{client_id}_{connection_attempt}"));
            match serde_json::to_string(client_info) {
                Ok(tc) => q.append_pair("tc", &tc),
                Err(e) => return Err(AllocatorError::Shape(e.to_string())),
            };
            q.append_pair("timeout", &timeout_secs.to_string());
        }
        let resp = self
            .http
            .post(url)
            .header("X-Skypetoken", token)
            .header("X-MS-Migration", "True")
            .header("Content-Type", "text/plain")
            .body("")
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(AllocatorError::Status(status.as_u16()));
        }
        resp.json::<Allocation>().await.map_err(|e| AllocatorError::Shape(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::{Allocator, ClientInfo};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn allocates_from_fixture_shape() {
        let fixture = include_str!("../tests/fixtures/trouter-allocate-v4a.json");
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v4/a"))
            .respond_with(ResponseTemplate::new(200).set_body_string(fixture))
            .mount(&server)
            .await;
        let allocator = Allocator::new(format!("{}/v4/a", server.uri()), reqwest::Client::new());
        let info = ClientInfo { ua: "test", v: "1415" };
        let alloc = allocator.allocate("tok", &info, "0", 1, 30).await.expect("allocate");
        assert_eq!(alloc.ttl, "79421");
        assert_eq!(alloc.connectparams.scae, "1");
    }

    #[tokio::test]
    async fn non_2xx_maps_to_status_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v4/a"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let allocator = Allocator::new(format!("{}/v4/a", server.uri()), reqwest::Client::new());
        let info = ClientInfo { ua: "test", v: "1415" };
        let err =
            allocator.allocate("tok", &info, "0", 1, 30).await.expect_err("expected status error");
        assert!(err.to_string().contains("401"));
    }
}
