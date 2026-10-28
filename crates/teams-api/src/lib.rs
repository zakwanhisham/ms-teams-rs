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
//! retries use jittered exponential backoff with `Retry-After` support, a 401 is recovered
//! by evict → re-mint → replay-once ([`TokenRefresher`]), and every failure maps into
//! [`ApiError`].

#![forbid(unsafe_code)]

mod mscv;
mod refresh;
mod retry;
mod service;

pub use mscv::{CorrelationVector, MS_CV_HEADER};
pub use refresh::{AudienceRefresher, AuthzRefresher, RefreshFuture, TokenRefresher};
pub use retry::{RetryPolicy, parse_retry_after};
pub(crate) use service::HttpService;

/// Exposed for the integration tests: the `Location`-tail parser of
/// [`ChatClient::send_message`] verbatim.
#[doc(hidden)]
#[must_use]
pub fn __test_only_message_id_from_location(location: &str) -> Option<&str> {
    message_id_from_location(location)
}

use serde_json::{Value, json};

use service::build_http_client;

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

    /// Credential recovery failed while handling a 401 (evict → re-mint path).
    #[error("auth: {0}")]
    Auth(#[from] teams_auth::AuthError),

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
    /// Shared plumbing: auth header, `MS-CV`, retries (credentials redacted in `Debug`).
    service: HttpService,
    base: url::Url,
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
            service: HttpService::new(
                build_http_client()?,
                (SKYPETOKEN_HEADER, format!("skypetoken={}", skypetoken.into())),
                &[],
            ),
            base: normalize_base(base_url)?,
        })
    }

    /// Override the retry schedule (short delays in tests, tuned backoff in production).
    #[must_use]
    pub fn with_retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.service = self.service.with_retry_policy(policy);
        self
    }

    /// Attach 401 recovery: on a rejected credential the client evicts → re-mints through
    /// the [`TokenRefresher`] and replays the request **exactly once** (a second 401
    /// surfaces as [`ApiError::Status`]; a failing re-mint surfaces as [`ApiError::Auth`]).
    #[must_use]
    pub fn with_refresher(mut self, refresher: impl TokenRefresher + 'static) -> Self {
        self.service = self.service.with_refresher(refresher);
        self
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
        self.service.get_json(self.base.join(path)?).await
    }

    /// Send a text message (`RichText/Html` content) and capture the server-assigned id.
    ///
    /// `POST /v1/users/ME/conversations/{conv}/messages` (verified live 2026-10-29:
    /// `201 Created`, the message id **only** in the `Location` header, body
    /// `{"OriginalArrivalTime":<ms epoch>}`).
    ///
    /// The id tail of `Location` is `13` digits for consumer chats. Never trust the
    /// echoed `clientmessageid` — the server may echo the *arrival* time instead.
    ///
    /// # Errors
    ///
    /// [`ApiError::Status`], [`ApiError::Json`], [`ApiError::Http`] — see `service.send`.
    pub async fn send_message(
        &self,
        conv: &str,
        content: &str,
        clientmessageid: i64,
    ) -> Result<SentMessage, ApiError> {
        let body = json!({
            // The wire casing is clientmessageid (no `t`), contenttype, messagetype.
            "clientmessageid": clientmessageid.to_string(),
            "content": content,
            "contenttype": "text",
            "messagetype": "RichText/Html",
        });
        let (value, location) = self
            .service
            .post_json_with_location(self.base.join(&messages_post_path(conv))?, &body)
            .await?;
        // The server echoes {OriginalArrivalTime: <ms>} — an integer (seen live) but taken
        // leniently as it may arrive as a JSON string or number.
        let arrival = value.get("OriginalArrivalTime").map(json_scalar_to_string);
        let message_id = location.as_deref().and_then(message_id_from_location).map(str::to_owned);
        Ok(SentMessage { message_id, original_arrival_time: arrival })
    }

    /// Edit one message. `properties` is **replaced, not merged** — an edit rewrite re-sends
    /// every property (the caller is expected to do that), so the *edit marker*
    /// `properties.skypeeditedid` — conventionally the clientmessageid of the edit — is
    /// mandatory to make the edit visible (verified live 2026-10-29: `200`, empty body).
    ///
    /// # Errors
    ///
    /// [`ApiError::Status`] (404 = unknown message id), or `Json`/`Http`.
    pub async fn edit_message(
        &self,
        conv: &str,
        message_id: &str,
        edit_clientmessageid: i64,
        content: &str,
    ) -> Result<(), ApiError> {
        let body = json!({
            "content": content,
            "contenttype": "text",
            "messagetype": "RichText/Html",
            "properties": { "skypeeditedid": edit_clientmessageid.to_string() },
        });
        self.service.put_json(self.base.join(&message_path(conv, message_id))?, &body).await?;
        Ok(())
    }

    /// Reply to a thread root: the same POST as [`ChatClient::send_message`] with the
    /// conversation segment suffixed (`{conv};messageid={root}`), matching the read path
    /// ([`ChatClient::get_thread`]).
    ///
    /// **Not yet fully live-verified**: feed thread types (`48:notifications`,
    /// `48:notes`) reject replies with
    /// `400 "Reply chains is not allowed in this thread type"` (`StoreInvalidInput`) —
    /// the URL + body shape reach the store and are recognized as replies, but acceptance
    /// in ordinary 1:1/group chats still needs a live check with a real conversation
    /// (owner-visible), hence this stays unverified rather than verified.
    ///
    /// # Errors
    ///
    /// [`ApiError::Status`] (400 = thread type forbids reply chains), `Json`, `Http`.
    pub async fn reply_message(
        &self,
        conv: &str,
        root_id: &str,
        content: &str,
        clientmessageid: i64,
    ) -> Result<SentMessage, ApiError> {
        let body = json!({
            "clientmessageid": clientmessageid.to_string(),
            "content": content,
            "contenttype": "text",
            "messagetype": "RichText/Html",
        });
        let path = format!("{}/messages", thread_root_segment(conv, root_id));
        let (value, location) =
            self.service.post_json_with_location(self.base.join(&path)?, &body).await?;
        let arrival = value.get("OriginalArrivalTime").map(json_scalar_to_string);
        let message_id = location.as_deref().and_then(message_id_from_location).map(str::to_owned);
        Ok(SentMessage { message_id, original_arrival_time: arrival })
    }

    /// Delete (soft-delete) one message. Verified live 2026-10-29: `200`, body `null`.
    ///
    /// # Errors
    ///
    /// [`ApiError::Status`] on non-2xx, or `Json`/`Http`.
    pub async fn delete_message(&self, conv: &str, message_id: &str) -> Result<(), ApiError> {
        self.service.delete(self.base.join(&message_path(conv, message_id))?).await?;
        Ok(())
    }

    /// React with `key` (`like`/`heart`/`laugh`/`surprised`/`sad`/`angry`).
    ///
    /// `PUT …/messages/{id}/properties?name=emotions` with the body the official web
    /// client builds (`{key,value}` — **not** a `{<key>: <ts>}` map): shape
    /// `{"emotions":{"key":"like","value":<ms epoch>}}`. Verified live 2026-10-29: `200`
    /// and the reaction shows up on a follow-up `GET` as `annotationsSummary.emotions.like: 1`.
    /// (Earlier shape attempts returned `400 errorCode:201 "Invalid format of emotions
    /// property"`.)
    ///
    /// # Errors
    ///
    /// [`ApiError::Status`] on a rejected payload/unknown message, or `Json`/`Http`.
    pub async fn react(
        &self,
        conv: &str,
        message_id: &str,
        key: &str,
        at_ms: i64,
    ) -> Result<(), ApiError> {
        let body = json!({ "emotions": { "key": key, "value": at_ms } });
        self.service
            .put_json(self.base.join(&message_prop_path(conv, message_id, "emotions"))?, &body)
            .await?;
        Ok(())
    }

    /// Remove a reaction. The official client PUTs `{"emotions":{"key":"<key>"}}`
    /// (bundle helper `deleteMessageProperty` — no `value` field) to the same
    /// `properties?name=emotions` endpoint.
    ///
    /// # Errors
    ///
    /// [`ApiError::Status`], `Json`, `Http`.
    pub async fn unreact(&self, conv: &str, message_id: &str, key: &str) -> Result<(), ApiError> {
        let body = json!({ "emotions": { "key": key } });
        self.service
            .put_json(self.base.join(&message_prop_path(conv, message_id, "emotions"))?, &body)
            .await?;
        Ok(())
    }

    /// Type-indicator broadcast: `POST {conv}/messages` with `messagetype:
    /// "Control/Typing"`, empty content (verified live 2026-10-29: `201` like a normal
    /// send, but nothing is rendered as a message).
    ///
    /// # Errors
    ///
    /// [`ApiError::Status`], `Json`, `Http`.
    pub async fn send_typing(&self, conv: &str) -> Result<(), ApiError> {
        let body = json!({
            "content": "",
            "contenttype": "text",
            "messagetype": "Control/Typing",
        });
        self.service.post_json(self.base.join(&messages_post_path(conv))?, &body).await?;
        Ok(())
    }

    /// Per-message read receipt: `{"isread":"<ms epoch as string>"}` on the message
    /// `properties?name=isread` endpoint (verified live 2026-10-29: `200`).
    ///
    /// # Errors
    ///
    /// [`ApiError::Status`], `Json`, `Http`.
    pub async fn mark_message_read(
        &self,
        conv: &str,
        message_id: &str,
        at_ms: i64,
    ) -> Result<(), ApiError> {
        let body = json!({ "isread": at_ms.to_string() });
        self.service
            .put_json(self.base.join(&message_prop_path(conv, message_id, "isread"))?, &body)
            .await?;
        Ok(())
    }

    /// Conversation-level read position (unread cursor). The property name is the
    /// lowercase `consumptionhorizon`, and the value is the `;`-delimited triple seen
    /// on read-side responses:
    /// `<firstUnreadOrigArrival>;\<lastReadOrSelf?>;<syncBookmark>`.
    ///
    /// Only a client that has already observed a real value should rewrite the bookmark
    /// field; callers without one can reuse the format-free variant by forwarding their
    /// own triple. Verified live 2026-10-29: `200` (camelCase name rejected with
    /// `400 errorCode:201`).
    ///
    /// # Errors
    ///
    /// [`ApiError::Status`], `Json`, `Http`.
    pub async fn set_consumption_horizon(&self, conv: &str, value: &str) -> Result<(), ApiError> {
        let body = json!({ "consumptionhorizon": value });
        self.service
            .put_json(self.base.join(&conversation_prop_path(conv, "consumptionhorizon"))?, &body)
            .await?;
        Ok(())
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
    /// Shared plumbing: auth header, `MS-CV`, retries (credentials redacted in `Debug`).
    service: HttpService,
    base: url::Url,
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
            service: HttpService::new(
                build_http_client()?,
                ("Authorization", format!("Bearer {}", bearer.into())),
                &[],
            ),
            base: normalize_base(base_url)?,
        })
    }

    /// Override the retry schedule (short delays in tests, tuned backoff in production).
    #[must_use]
    pub fn with_retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.service = self.service.with_retry_policy(policy);
        self
    }

    /// Attach 401 recovery: on a rejected credential the client evicts → re-mints through
    /// the [`TokenRefresher`] and replays the request **exactly once** (a second 401
    /// surfaces as [`ApiError::Status`]; a failing re-mint surfaces as [`ApiError::Auth`]).
    #[must_use]
    pub fn with_refresher(mut self, refresher: impl TokenRefresher + 'static) -> Self {
        self.service = self.service.with_refresher(refresher);
        self
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
        self.service.get_json(self.base.join(path)?).await
    }
}

/// People-search client (substrate suggestions).
///
/// **Base-URL exception**: every other service takes its base from the authz `regionGtms`
/// map, but that map has no `substrate` entry (checked against all 112 live services,
/// 2026-10-27) — the global `substrate.office.com` front door is therefore a documented
/// constant exception, recorded in `docs/endpoints.md`.
#[derive(Debug)]
pub struct SubstrateClient {
    /// Shared plumbing: auth header, `Origin`/`Referer`, `MS-CV`, retries.
    service: HttpService,
    /// Fully-built suggestions URL (`…/suggestions?scenario=powerbar`).
    suggestions_url: url::Url,
}

/// Documented constant exception to "base URLs are data": `regionGtms` has no
/// `substrate` key, so the service is addressed by its well-known global host.
pub const SUBSTRATE_BASE: &str = "https://substrate.office.com";

/// The suggestions endpoint rejects requests without a Teams `Origin`/`Referer` (HTTP 400).
const TEAMS_ORIGIN: &str = "https://teams.microsoft.com";

/// Static headers sent with every suggestions request.
const SUGGESTIONS_HEADERS: &[(&str, &str)] =
    &[("Origin", TEAMS_ORIGIN), ("Referer", "https://teams.microsoft.com/")];

/// Suggestions path; the `scenario` query parameter is required, not decorative.
const SUGGESTIONS_PATH: &str = "search/api/v1/suggestions?scenario=powerbar";

/// Directory fields requested for each suggestion. Live-verified set: one unknown field
/// name makes the whole call fail with `Invalid field name specified.` (HTTP 400).
const SUGGESTION_FIELDS: [&str; 6] =
    ["Id", "MRI", "DisplayName", "EmailAddresses", "JobTitle", "Department"];

impl SubstrateClient {
    /// Build a client from a substrate bearer (minted for the `substrate` audience via
    /// [`teams_auth::TokenCache::fresh_or_redeem`], scope `SCOPE_SUBSTRATE`).
    ///
    /// # Errors
    ///
    /// [`ApiError::Url`] when [`SUBSTRATE_BASE`] stops parsing,
    /// [`ApiError::Http`] when the HTTP client cannot be built (invalid TLS roots).
    pub fn new(bearer: impl Into<String>) -> Result<Self, ApiError> {
        Ok(Self {
            service: HttpService::new(
                build_http_client()?,
                ("Authorization", format!("Bearer {}", bearer.into())),
                SUGGESTIONS_HEADERS,
            ),
            suggestions_url: url::Url::parse(&format!("{SUBSTRATE_BASE}/{SUGGESTIONS_PATH}"))?,
        })
    }

    /// Override the retry schedule (short delays in tests, tuned backoff in production).
    #[must_use]
    pub fn with_retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.service = self.service.with_retry_policy(policy);
        self
    }

    /// Attach 401 recovery: on a rejected credential the client evicts → re-mints through
    /// the [`TokenRefresher`] and replays the request **exactly once** (a second 401
    /// surfaces as [`ApiError::Status`]; a failing re-mint surfaces as [`ApiError::Auth`]).
    #[must_use]
    pub fn with_refresher(mut self, refresher: impl TokenRefresher + 'static) -> Self {
        self.service = self.service.with_refresher(refresher);
        self
    }

    /// Search the directory for people (Phase 4 read path), typed.
    ///
    /// `POST https://substrate.office.com/search/api/v1/suggestions?scenario=powerbar`
    ///
    /// The body carries two fresh correlation ids per call (`cvid`, `logicalId`) and one
    /// `EntityRequests` entry of `EntityType: "People"`; the response is grouped by entity
    /// type, so [`teams_protocol::SearchSuggestions::people`] is what a caller wants.
    ///
    /// # Errors
    ///
    /// [`ApiError::Status`] on non-2xx (400 = bad `Fields` entry, 401 = substrate bearer
    /// expired → `teams-cli auth mint`), [`ApiError::Json`] on malformed JSON or an
    /// unparseable typed shape, [`ApiError::Http`] on transport failure.
    pub async fn search_people(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<teams_protocol::SearchSuggestions, ApiError> {
        let body = suggestions_body(query, limit);
        let value = self.service.post_json(self.suggestions_url.clone(), &body).await?;
        Ok(serde_json::from_value(value)?)
    }
}

/// Body of one suggestions call: correlation ids + the `EntityRequests` envelope.
///
/// Kept as a free function so the exact wire shape is unit-testable without a server.
fn suggestions_body(query: &str, limit: u32) -> Value {
    serde_json::json!({
        "cvid": uuid::Uuid::new_v4().to_string(),
        "logicalId": uuid::Uuid::new_v4().to_string(),
        "EntityRequests": [{
            "Query": {"QueryString": query, "DisplayQueryString": query},
            "EntityType": "People",
            "Size": limit,
            "Fields": SUGGESTION_FIELDS,
        }],
    })
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
    format!("{}/messages?pageSize={page_size}&startTime=0", thread_root_segment(conv, root_id))
}

/// Single-message path.
fn message_path(conv: &str, message_id: &str) -> String {
    format!(
        "v1/users/ME/conversations/{}/messages/{}",
        encode_path_segment(conv),
        encode_path_segment(message_id)
    )
}

/// Message POST (send/typing) path.
fn messages_post_path(conv: &str) -> String {
    format!("v1/users/ME/conversations/{}/messages", encode_path_segment(conv))
}

/// Conversation-segment prefix of a reply POST: `{conv};messageid={root}` (same segment
/// form the read-side thread path uses).
fn thread_root_segment(conv: &str, root_id: &str) -> String {
    format!(
        "v1/users/ME/conversations/{};messageid={}",
        encode_path_segment(conv),
        encode_path_segment(root_id)
    )
}

/// Per-message property path — `properties?name=<name>` under the message.
fn message_prop_path(conv: &str, message_id: &str, name: &str) -> String {
    format!(
        "v1/users/ME/conversations/{}/messages/{}/properties?name={}",
        encode_path_segment(conv),
        encode_path_segment(message_id),
        name
    )
}

/// Conversation property path — `properties?name=<name>` under the conversation.
fn conversation_prop_path(conv: &str, name: &str) -> String {
    format!("v1/users/ME/conversations/{}/properties?name={}", encode_path_segment(conv), name)
}

/// The server-assigned message id: the `Location` header's path tail (verified live:
/// 13 digits). The header is an absolute URL of this service.
fn message_id_from_location(location: &str) -> Option<&str> {
    Some(location.rsplit('/').next()?.trim_start_matches('/').split('?').next()?)
        .filter(|id| !id.is_empty())
}

/// A JSON scalar (number or string) as its string form; objects/arrays come back empty.
fn json_scalar_to_string(value: &serde_json::Value) -> String {
    match value {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        _ => String::new(),
    }
}

/// Result of a successful [`ChatClient::send_message`].
#[derive(Debug)]
pub struct SentMessage {
    /// Server-assigned id (the `Location` tail). `None` if the header was missing or
    /// empty — a degraded response the caller must handle by reflection on the feed.
    pub message_id: Option<String>,
    /// Server echo of `OriginalArrivalTime` (ms epoch as string).
    pub original_arrival_time: Option<String>,
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

    /// The body shape was probed live (2026-10-27): a missing `EntityRequests` fails with
    /// `ArgumentNullException`, an unknown `Fields` entry with `Invalid field name specified.`
    #[test]
    fn suggestions_body_matches_the_live_shape() {
        let body = suggestions_body("ken", 5);
        let requests =
            body.get("EntityRequests").and_then(Value::as_array).expect("EntityRequests array");
        assert_eq!(requests.len(), 1, "one entity request per call");

        let entry = &requests[0];
        assert_eq!(entry["EntityType"], "People");
        assert_eq!(entry["Size"], 5);
        // The query is mirrored into the display copy — that is what the picker shows.
        assert_eq!(entry["Query"]["QueryString"], "ken");
        assert_eq!(entry["Query"]["DisplayQueryString"], "ken");

        let fields = entry.get("Fields").and_then(Value::as_array).expect("Fields array").clone();
        let fields: Vec<&str> =
            fields.iter().map(Value::as_str).collect::<Option<Vec<_>>>().unwrap();
        assert_eq!(fields, SUGGESTION_FIELDS);
    }

    /// `cvid`/`logicalId` are per-request correlation ids: fresh `UUIDv4` on every call.
    #[test]
    fn suggestions_body_mints_fresh_uuid_correlation_ids() {
        let first = suggestions_body("x", 1);
        let second = suggestions_body("x", 1);
        let cvid = first["cvid"].as_str().expect("cvid is a string");

        assert_eq!(cvid.len(), 36, "canonical UUID length");
        assert_eq!(&cvid[14..15], "4", "UUIDv4 version nibble");
        assert_ne!(first["cvid"], second["cvid"], "never reused between calls");
        assert_ne!(first["logicalId"], second["logicalId"]);
    }

    #[test]
    fn suggestions_url_keeps_the_required_scenario() {
        let client = SubstrateClient::new("jwt").expect("client");
        assert_eq!(
            client.suggestions_url.as_str(),
            "https://substrate.office.com/search/api/v1/suggestions?scenario=powerbar"
        );
    }
}
