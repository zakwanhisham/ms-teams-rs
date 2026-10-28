//! Registrar client: `POST {allocation.registrar_url}`.
//!
//! Live-verified 2026-10-29 (`docs/endpoints.md`): the URL comes from the allocation
//! response (`…/registrar/prod/V3/registrations` on `edge.skype.com`), headers
//! `X-Skypetoken` + `X-MS-Migration: True`, body `{clientDescription, registrationId,
//! nodeId: "", transports: {TROUTER: [{context: "", path: <allocation.surl>, ttl}]}}` —
//! success is **202 Accepted with an empty body**.

use serde::Serialize;
use thiserror::Error;
use uuid::Uuid;

use crate::alloc::Allocation;
use crate::{TEAMS_APP_ID, TEAMS_TEMPLATE_KEY};

/// `clientDescription` — the identity of this endpoint to the registrar.
///
/// Live-verified field set and values; `product_context` reached the server as a
/// **stringified JSON object** (an object here is a server error).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ClientDescription {
    /// Per-endpoint UUID — mirrored in the top-level field. (Kept here because the
    /// registrar validated the *description* as the primary carrier in our live probe.)
    #[serde(rename = "registrationId")]
    pub registration_id: String,
    /// Application identity (`"Microsoft Teams Web"` accepted live).
    pub app_id: String,
    /// Always empty in the official client.
    #[serde(rename = "aesKey")]
    pub aes_key: String,
    /// UI language (`en-US`).
    #[serde(rename = "languageId")]
    pub language_id: String,
    /// Platform (`web` in the official web client).
    pub platform: String,
    /// The template key — `MicrosoftTeams` is the only value we saw live.
    #[serde(rename = "templateKey")]
    pub template_key: String,
    /// Client version string.
    #[serde(rename = "platformUIVersion")]
    pub platform_ui_version: String,
    /// Must be **a JSON string** (e.g. `{"appType":…, "productNum":…}`) — an object form
    /// fails the registrar with `Unexpected token StartObject`.
    #[serde(rename = "productContext")]
    pub product_context: String,
}

/// One transport the registrar routes messages to.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TrouterTransport {
    /// Empty in the observed shape.
    pub context: String,
    /// Exactly the allocation's `surl`.
    pub path: String,
    /// Requested TTL in seconds; client uses 7200 (the allocation ttl governs the ws).
    pub ttl: u32,
}

/// The full registration body.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Registration {
    /// Endpoint identity sub-object.
    #[serde(rename = "clientDescription")]
    pub client_description: ClientDescription,
    /// Top-level registration id — same value as inside `clientDescription`.
    #[serde(rename = "registrationId")]
    pub registration_id: String,
    /// Empty in the observed shape.
    #[serde(rename = "nodeId")]
    pub node_id: String,
    /// Transports keyed by name (`"TROUTER"`).
    pub transports: std::collections::BTreeMap<String, Vec<TrouterTransport>>,
}

/// Build the registration body the official client sends for an allocation.
///
/// `registration_id` stays *caller-owned* so reconnects can reuse one endpoint identity
/// across allocations (its refresh semantics are verified in the capture session).
#[must_use]
pub fn build_registration(
    registration_id: &str,
    allocation: &Allocation,
    platform_ui_version: &str,
) -> Registration {
    let mut transports = std::collections::BTreeMap::new();
    transports.insert(
        "TROUTER".to_string(),
        vec![TrouterTransport { context: String::new(), path: allocation.surl.clone(), ttl: 7200 }],
    );
    Registration {
        client_description: ClientDescription {
            registration_id: registration_id.to_string(),
            app_id: TEAMS_APP_ID.to_string(),
            aes_key: String::new(),
            language_id: "en-US".to_string(),
            platform: "web".to_string(),
            template_key: TEAMS_TEMPLATE_KEY.to_string(),
            platform_ui_version: platform_ui_version.to_string(),
            product_context: r#"{"appType":"Microsoft Teams Web","productNum":"13"}"#.to_string(),
        },
        registration_id: registration_id.to_string(),
        node_id: String::new(),
        transports,
    }
}

/// Fresh UUID for an endpoint that needs a new registration identity.
#[must_use]
pub fn new_registration_id() -> String {
    Uuid::new_v4().to_string()
}

/// Errors while registering.
#[derive(Debug, Error)]
pub enum RegistrarError {
    /// Unaccepted registration (`400` for shape errors — `templateKey`/`productContext` —
    /// or `401` for bad auth). The body note stays short (max 120 chars) so no future
    /// body ever dumps wholesale into an error or log line.
    #[error("registration rejected: HTTP {status}{body}")]
    Rejected {
        /// HTTP status code.
        status: u16,
        /// Trimmed body note ("" when empty — all live failures were empty).
        body: String,
    },
    /// HTTP client failure.
    #[error("registration request failed: {0}")]
    Http(#[from] reqwest::Error),
    /// The allocation carried no registrar URL.
    #[error("allocation carries no registrarUrl")]
    NoRegistrarUrl,
}

/// Client for the register call against one allocation's registrar.
#[derive(Debug)]
pub struct Registrar {
    http: reqwest::Client,
}

impl Registrar {
    /// New registrar client (share one HTTP client with the allocator).
    #[must_use]
    pub const fn new(http: reqwest::Client) -> Self {
        Self { http }
    }

    /// POST the registration. Success is **202 with an empty body** — anything else maps
    /// to [`RegistrarError::Rejected`].
    pub async fn register(
        &self,
        allocation: &Allocation,
        registration: &Registration,
        token: &str,
    ) -> Result<(), RegistrarError> {
        if allocation.registrar_url.is_empty() {
            return Err(RegistrarError::NoRegistrarUrl);
        }
        let resp = self
            .http
            .post(allocation.registrar_url.as_str())
            .header("X-Skypetoken", token)
            .header("X-MS-Migration", "True")
            .header("Content-Type", "application/json")
            .header("accept", "application/json, text/javascript")
            .json(registration)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(RegistrarError::Rejected {
                status: status.as_u16(),
                body: truncate_note(&body),
            });
        }
        Ok(())
    }
}

/// Trimmed body note for errors, at most 120 chars (char-boundary safe).
fn truncate_note(body: &str) -> String {
    body.trim().chars().take(120).collect()
}

#[cfg(test)]
mod tests {
    use super::{Registrar, build_registration, new_registration_id};
    use crate::alloc::Allocation;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Load the sanitized allocation fixture and point its registrar at a mock server.
    fn fixture_allocation(server_uri: &str) -> Allocation {
        let mut alloc: Allocation =
            serde_json::from_str(include_str!("../tests/fixtures/trouter-allocate-v4a.json"))
                .expect("fixture");
        alloc.registrar_url = format!("{server_uri}/registrar/prod/V3/registrations");
        alloc
    }

    #[tokio::test]
    async fn registers_against_allocation_url() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/registrar/prod/V3/registrations"))
            .respond_with(ResponseTemplate::new(202))
            .mount(&server)
            .await;
        let alloc = fixture_allocation(&server.uri());
        let reg = build_registration("reg-1", &alloc, "1415/26.01.00");
        Registrar::new(reqwest::Client::new())
            .register(&alloc, &reg, "tok")
            .await
            .expect("202 expected");
    }

    #[tokio::test]
    async fn registration_body_shape_is_official() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/registrar/prod/V3/registrations"))
            .respond_with(ResponseTemplate::new(400).set_body_string(
                r#"{"code":40041,"message":"client description field \"templateKey\" is required for set registrations"}"#,
            ))
            .mount(&server)
            .await;
        let alloc = fixture_allocation(&server.uri());
        let reg = build_registration("reg-1", &alloc, "1415/26.01.00");
        let err = Registrar::new(reqwest::Client::new())
            .register(&alloc, &reg, "tok")
            .await
            .expect_err("400");
        assert!(err.to_string().contains("40041"));
        let cd = &reg.client_description;
        assert_eq!(cd.template_key, "MicrosoftTeams");
        assert_eq!(cd.registration_id, reg.registration_id);
        // productContext must be a JSON *string*
        assert!(cd.product_context.starts_with('{'));
        assert!(serde_json::from_str::<serde_json::Value>(&cd.product_context).is_ok());
        assert_eq!(new_registration_id().len(), 36);
    }
}
