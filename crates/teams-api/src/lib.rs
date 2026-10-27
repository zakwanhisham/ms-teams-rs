//! Typed HTTP clients for the unofficial Microsoft Teams APIs.
//!
//! Services covered (see `TODO.md` Phase 4):
//!
//! | Service | Base URL source | Auth header |
//! |---------|-----------------|-------------|
//! | Chat (IC3 / MSNP24) | `regionGtms.chatService` from authz | `Authentication: skypetoken=<jwt>` |
//! | Roster (CSA) | `regionGtms.chatSvcAggAfd` from authz | `Authorization: Bearer <jwt>` |
//! | People search (substrate) | `substrate.office.com/search/…` | `Authorization: Bearer <jwt>` |
//! | Calendar / mail (Graph) | `graph.microsoft.com/v1.0` | `Authorization: Bearer <jwt>` |
//! | Files (ASM) | `api.asm.skype.com` | `Authorization: skypetoken <jwt>` |
//!
//! Rules: base URLs always come from the authz `regionGtms` map (never hardcoded regions),
//! retries use jittered exponential backoff with `Retry-After` support, and every failure
//! maps into `ApiError`.

#![forbid(unsafe_code)]

use std::time::Duration;

use serde_json::Value;

/// Chat service auth header name. Note: `Authentication`, **not** `Authorization`.
pub const SKYPETOKEN_HEADER: &str = "Authentication";

/// Web-client-like User-Agent; some front doors reject unknown agents.
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) \
    Teams/1.7.00.27023 Chrome/122.0.0.0 Safari/537.36";

/// All API failures funnel into this error type.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// Transport-level failure (DNS, TLS, timeout, …).
    #[error("http request failed: {0}")]
    Http(#[from] reqwest::Error),

    /// Non-2xx response from a Teams service.
    #[error("service returned {status}: {body}")]
    Status {
        /// HTTP status.
        status: u16,
        /// Truncated response body for diagnostics.
        body: String,
    },

    /// URL construction failed (bad base URL from `regionGtms`, …).
    #[error("url error: {0}")]
    Url(#[from] url::ParseError),

    /// Response was not the JSON we expected.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// Missing/invalid client configuration (base URL, token, …).
    #[error("client config: {0}")]
    Config(String),
}

/// Chat service (IC3 / MSNP24) client.
///
/// The base URL comes from the authz `regionGtms.chatService` entry at runtime — never
/// hardcoded — and every request carries `Authentication: skypetoken=<jwt>`.
#[derive(Debug)]
pub struct ChatClient {
    http: reqwest::Client,
    base: url::Url,
    /// Never logged, never serialized, never exposed to the GUI.
    skypetoken: String,
}

impl ChatClient {
    /// Build a client for a chat-service base URL (e.g. `https://apac.ng.msg.teams.microsoft.com`).
    ///
    /// # Errors
    ///
    /// [`ApiError::Url`] when `base_url` is not a valid absolute URL,
    /// [`ApiError::Http`] when the HTTP client cannot be built (invalid TLS roots).
    pub fn new(base_url: &str, skypetoken: impl Into<String>) -> Result<Self, ApiError> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .user_agent(USER_AGENT)
                .build()?,
            base: normalize_base(base_url)?,
            skypetoken: skypetoken.into(),
        })
    }

    /// Build from a cached authz session: base URL from `regionGtms.chatService`, token
    /// from the session's skypetoken. This is how callers should construct the client.
    ///
    /// # Errors
    ///
    /// [`ApiError::Config`] when the session has no `chatService` entry, plus the errors
    /// of [`ChatClient::new`].
    pub fn from_session(session: &teams_auth::AuthzSession) -> Result<Self, ApiError> {
        let base = session
            .host("chatService")
            .ok_or_else(|| ApiError::Config("authz session has no chatService entry".into()))?;
        Self::new(base, session.skype_token.clone())
    }

    /// List messages of a conversation (Phase 4 read path), typed.
    ///
    /// `GET {base}/v1/users/ME/conversations/{conv}/messages?pageSize=N&startTime=0`
    ///
    /// `{conv}` keeps the wire's raw `:` `@` `;` (that is how the service writes its own
    /// `messagesLink`); only path-breaking characters are percent-encoded.
    ///
    /// # Errors
    ///
    /// [`ApiError::Status`] on non-2xx (401 = skypetoken expired → re-run authz),
    /// [`ApiError::Json`] on malformed JSON or an unparseable typed shape,
    /// [`ApiError::Http`] on transport failure.
    pub async fn list_messages(
        &self,
        conv: &str,
        page_size: u32,
    ) -> Result<teams_protocol::MessagePage, ApiError> {
        let value = self.get(&messages_path(conv, page_size)).await?;
        Ok(serde_json::from_value(value)?)
    }

    /// Fetch the thread of a message: the root plus its replies.
    ///
    /// `GET {base}/v1/users/ME/conversations/{conv};messageid={root}/messages?…`
    ///
    /// The `;messageid=` suffix rides on the *conversation id segment* rather than being a
    /// separate path element — that is what the web client sends. The response is the same
    /// `{_metadata, messages, tenantId}` envelope as the plain messages page (verified live
    /// 2026-10-27), so it reuses [`teams_protocol::MessagePage`].
    ///
    /// # Errors
    ///
    /// [`ApiError::Status`] on non-2xx (404 = root id does not exist in this
    /// conversation), [`ApiError::Json`] on malformed JSON or an unparseable typed shape,
    /// [`ApiError::Http`] on transport failure.
    pub async fn get_thread(
        &self,
        conv: &str,
        root_id: &str,
        page_size: u32,
    ) -> Result<teams_protocol::MessagePage, ApiError> {
        let value = self.get(&thread_path(conv, root_id, page_size)).await?;
        Ok(serde_json::from_value(value)?)
    }

    /// Fetch a single message by id.
    ///
    /// `GET {base}/v1/users/ME/conversations/{conv}/messages/{id}`
    ///
    /// Unlike the pages, the response is a **bare message object** (no `_metadata`
    /// envelope) and additionally carries `fromTenantId`.
    ///
    /// # Errors
    ///
    /// [`ApiError::Status`] on non-2xx (404 when the message is gone or unknown),
    /// [`ApiError::Json`] on malformed JSON or an unparseable typed shape,
    /// [`ApiError::Http`] on transport failure.
    pub async fn get_message(
        &self,
        conv: &str,
        message_id: &str,
    ) -> Result<teams_protocol::Message, ApiError> {
        let value = self.get(&message_path(conv, message_id)).await?;
        Ok(serde_json::from_value(value)?)
    }

    /// `GET {base}/{path}` with the skypetoken header, returning parsed JSON.
    async fn get(&self, path: &str) -> Result<Value, ApiError> {
        let url = self.base.join(path)?;
        let resp = self
            .http
            .get(url)
            .header(SKYPETOKEN_HEADER, format!("skypetoken={}", self.skypetoken))
            .header("Accept", "application/json")
            .send()
            .await?;

        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            // Never log the body verbatim here: service errors can echo request context.
            tracing::warn!(status = status.as_u16(), body_len = body.len(), "chat service error");
            return Err(ApiError::Status {
                status: status.as_u16(),
                body: crate::truncate(&body, 300),
            });
        }
        Ok(serde_json::from_str(&body)?)
    }

    /// List conversations (both 1:1/group chats and team channels), typed.
    ///
    /// `GET /v1/users/ME/conversations?view=msnp24Equivalent&pageSize=N&startTime=0`
    ///
    /// # Errors
    ///
    /// [`ApiError::Status`] on non-2xx (401 = skypetoken expired → re-run authz),
    /// [`ApiError::Json`] on malformed JSON or an unparseable typed shape,
    /// [`ApiError::Http`] on transport failure.
    pub async fn list_conversations(
        &self,
        page_size: u32,
    ) -> Result<teams_protocol::ConversationList, ApiError> {
        let value = self.get(&conversations_path(page_size)).await?;
        Ok(serde_json::from_value(value)?)
    }
}

/// Team/channel roster client (CSA — chat service aggregator).
///
/// The base URL comes from the authz `regionGtms.chatSvcAggAfd` entry (region-encoded at
/// runtime, e.g. `…/api/csa/apac`) and every request carries a plain OAuth bearer
/// (`Authorization: Bearer <jwt>`) for the `chatsvcagg.teams.microsoft.com` audience —
/// not a skypetoken.
#[derive(Debug)]
pub struct RosterClient {
    http: reqwest::Client,
    base: url::Url,
    /// Never logged, never serialized, never exposed to the GUI.
    bearer: String,
}

impl RosterClient {
    /// Build a client for a CSA base URL (e.g. `https://teams.microsoft.com/api/csa/apac`).
    ///
    /// # Errors
    ///
    /// [`ApiError::Url`] when `base_url` is not a valid absolute URL,
    /// [`ApiError::Http`] when the HTTP client cannot be built (invalid TLS roots).
    pub fn new(base_url: &str, bearer: impl Into<String>) -> Result<Self, ApiError> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .user_agent(USER_AGENT)
                .build()?,
            base: normalize_base(base_url)?,
            bearer: bearer.into(),
        })
    }

    /// Build from a cached authz session: base URL from `regionGtms.chatSvcAggAfd`, token
    /// from the caller (minted for the `chatsvcagg` audience via
    /// [`teams_auth::TokenCache::fresh_or_redeem`]).
    ///
    /// # Errors
    ///
    /// [`ApiError::Config`] when the session has no `chatSvcAggAfd` entry, plus the errors
    /// of [`RosterClient::new`].
    pub fn from_session(
        session: &teams_auth::AuthzSession,
        bearer: impl Into<String>,
    ) -> Result<Self, ApiError> {
        let base = session
            .host("chatSvcAggAfd")
            .ok_or_else(|| ApiError::Config("authz session has no chatSvcAggAfd entry".into()))?;
        Self::new(base, bearer)
    }

    /// Team/channel roster (Phase 4 read path), typed.
    ///
    /// `GET {base}/api/v1/teams/users/me/updates?isPrefetch=false&enableMembershipSummary=true`
    ///
    /// The response also carries the caller's chats and scheduled meetings (with their join
    /// URLs), so this is the one call that answers "what is in my Teams?".
    ///
    /// # Errors
    ///
    /// [`ApiError::Status`] on non-2xx (401 = CSA bearer expired → `teams-cli auth mint`),
    /// [`ApiError::Json`] on malformed JSON or an unparseable typed shape, [`ApiError::Http`]
    /// on transport failure.
    pub async fn roster(&self) -> Result<teams_protocol::RosterUpdate, ApiError> {
        let value = self.get(team_updates_path()).await?;
        Ok(serde_json::from_value(value)?)
    }

    /// `GET {base}/{path}` with the CSA bearer, returning parsed JSON.
    async fn get(&self, path: &str) -> Result<Value, ApiError> {
        let url = self.base.join(path)?;
        let resp = self
            .http
            .get(url)
            .header("Authorization", format!("Bearer {}", self.bearer))
            .header("Accept", "application/json")
            .send()
            .await?;

        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            // Never log the body verbatim: service errors can echo request context.
            tracing::warn!(status = status.as_u16(), body_len = body.len(), "CSA service error");
            return Err(ApiError::Status {
                status: status.as_u16(),
                body: crate::truncate(&body, 300),
            });
        }
        Ok(serde_json::from_str(&body)?)
    }
}

/// Parse a service base URL for `Url::join`-based path building.
///
/// Drops query/fragment and guarantees a trailing `/` on the path: the CSA base ends in a
/// region segment (`…/api/csa/apac`) and `Url::join("api/v1/…")` would otherwise *replace*
/// `apac` instead of appending after it.
fn normalize_base(base_url: &str) -> Result<url::Url, ApiError> {
    let mut base = url::Url::parse(base_url)?;
    base.set_query(None);
    base.set_fragment(None);
    if !base.path().ends_with('/') {
        let mut path = base.path().to_owned();
        path.push('/');
        base.set_path(&path);
    }
    Ok(base)
}

/// `GET /v1/users/ME/conversations` page path (`msnp24Equivalent` view, first page).
fn conversations_path(page_size: u32) -> String {
    format!("v1/users/ME/conversations?view=msnp24Equivalent&pageSize={page_size}&startTime=0")
}

/// Messages page path of one conversation.
fn messages_path(conv: &str, page_size: u32) -> String {
    format!(
        "v1/users/ME/conversations/{}/messages?pageSize={page_size}&startTime=0",
        encode_path_segment(conv)
    )
}

/// Thread path — `;messageid={root}` rides on the **conversation id segment** instead of
/// being its own path element, matching what the web client sends.
fn thread_path(conv: &str, root_id: &str, page_size: u32) -> String {
    format!(
        "v1/users/ME/conversations/{};messageid={}/messages?pageSize={page_size}&startTime=0",
        encode_path_segment(conv),
        encode_path_segment(root_id)
    )
}

/// Single-message path.
fn message_path(conv: &str, message_id: &str) -> String {
    format!(
        "v1/users/ME/conversations/{}/messages/{}",
        encode_path_segment(conv),
        encode_path_segment(message_id)
    )
}

/// Team/channel roster path under the CSA base (`…/api/csa/{region}/`).
fn team_updates_path() -> &'static str {
    "api/v1/teams/users/me/updates?isPrefetch=false&enableMembershipSummary=true"
}

/// Percent-encode one path segment.
///
/// The chat service puts conversation ids into paths **raw** — its own `messagesLink`
/// values contain literal `:` `@` `;` (verified live 2026-10-27) — so those are left
/// untouched. Everything that would actually break URL parsing (path separators, `?`,
/// `#`, `%`, controls) is encoded.
pub(crate) fn encode_path_segment(segment: &str) -> String {
    use percent_encoding::{AsciiSet, CONTROLS, percent_encode};

    /// URL path-segment set (` `, `"`, `#`, `<`, `>`, `?`, `` ` ``, `{`, `}`, `/`, `\`)
    /// plus `%` so an id can never be double-decoded.
    const SEGMENT: &AsciiSet = &CONTROLS
        .add(b' ')
        .add(b'"')
        .add(b'#')
        .add(b'<')
        .add(b'>')
        .add(b'?')
        .add(b'`')
        .add(b'{')
        .add(b'}')
        .add(b'/')
        .add(b'\\')
        .add(b'%');
    percent_encode(segment.as_bytes(), SEGMENT).to_string()
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
    fn skypetoken_header_is_not_authorization() {
        assert_eq!(SKYPETOKEN_HEADER, "Authentication");
    }

    #[test]
    fn bad_base_url_is_rejected() {
        assert!(matches!(ChatClient::new("not a url", "jwt"), Err(ApiError::Url(_))));
    }

    /// `Url::join` replaces the last path segment; the trailing slash is what makes the
    /// CSA base (`…/api/csa/apac`) append instead.
    #[test]
    fn base_urls_are_join_safe() {
        let base = normalize_base("https://teams.microsoft.com/api/csa/apac").expect("url");
        assert_eq!(
            base.join(team_updates_path()).expect("join").as_str(),
            "https://teams.microsoft.com/api/csa/apac/api/v1/teams/users/me/updates\
             ?isPrefetch=false&enableMembershipSummary=true"
        );

        let chat = normalize_base("https://apac.ng.msg.teams.microsoft.com").expect("url");
        assert_eq!(
            chat.join("v1/users/ME/conversations").expect("join").as_str(),
            "https://apac.ng.msg.teams.microsoft.com/v1/users/ME/conversations"
        );
    }

    #[test]
    fn truncate_respects_boundaries() {
        assert_eq!(truncate("short", 300), "short");
        assert!(truncate("héllo wörld", 5).ends_with('…'));
    }

    #[test]
    fn conversation_ids_keep_wire_safe_characters() {
        // Matches the service's own `messagesLink` formatting.
        let id = "19:3Vh2-xiNZrPlFoEx2yicJiUhP5BTvscVMP_EJW4ASrk1@thread.tacv2";
        assert_eq!(encode_path_segment(id), id);
        assert_eq!(
            encode_path_segment("19:meeting_abc;def@thread.v2"),
            "19:meeting_abc;def@thread.v2"
        );
    }

    #[test]
    fn path_breaking_characters_are_encoded() {
        assert_eq!(encode_path_segment("a?b#c"), "a%3Fb%23c");
        assert_eq!(encode_path_segment("a b"), "a%20b");
        assert_eq!(encode_path_segment("50%off"), "50%25off");
        assert_eq!(encode_path_segment("a/b"), "a%2Fb");
    }

    #[test]
    fn conversations_path_is_the_first_msnp24_page() {
        assert_eq!(
            conversations_path(5),
            "v1/users/ME/conversations?view=msnp24Equivalent&pageSize=5&startTime=0"
        );
    }

    /// The `;messageid=` form is protocol surface: it must stay glued to the conv id.
    #[test]
    fn thread_path_keeps_the_semicolon_form() {
        assert_eq!(
            thread_path("19:abc@thread.tacv2", "1700000000001", 10),
            "v1/users/ME/conversations/19:abc@thread.tacv2;\
             messageid=1700000000001/messages?pageSize=10&startTime=0"
        );
    }

    #[test]
    fn message_paths_encode_only_path_breaking_bytes() {
        let conv = "19:meeting_x?y@thread.v2"; // a literal `?` would start the query string
        assert_eq!(
            messages_path(conv, 5),
            "v1/users/ME/conversations/19:meeting_x%3Fy@thread.v2/messages?pageSize=5&startTime=0"
        );
        assert_eq!(
            message_path(conv, "1700000000001"),
            "v1/users/ME/conversations/19:meeting_x%3Fy@thread.v2/messages/1700000000001"
        );
    }
}
