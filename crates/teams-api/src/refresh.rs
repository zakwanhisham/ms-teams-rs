//! 401 recovery: evict → re-mint → replay once.
//!
//! When a Teams front door answers **401** it means "this credential is dead" — not "the
//! network is flaky". Backoff must not burn its budget on it (transport errors and 401s
//! are excluded from [`crate::RetryPolicy`] on purpose, so the future write path can never
//! double-send); instead the HTTP layer asks a [`TokenRefresher`] for a fresh auth-header
//! value and replays the request exactly once.
//!
//! The bound lives in the HTTP layer: one `refreshed` flag per logical request, so a
//! service that keeps answering 401 surfaces the error instead of looping. The refreshers
//! below work from the on-disk [`teams_auth::TokenCache`]: each call **reloads the cache
//! from disk** (no lock is ever held across the await) and persists the re-minted
//! credential before returning.

use std::fmt::Debug;
use std::path::PathBuf;
use std::pin::Pin;

use teams_auth::{AUDIENCE_SPACES, SCOPE_SPACES, TokenCache};

use crate::ApiError;

/// Future returned by [`TokenRefresher::refresh`].
///
/// Boxed and `Send` so the trait stays object-safe and a refresher can run under any
/// async executor (the GUI's included).
pub type RefreshFuture<'a> = Pin<Box<dyn Future<Output = Result<String, ApiError>> + Send + 'a>>;

/// Produces a fresh auth-header value after a service answers 401.
///
/// The HTTP layer calls this at most once per logical request — only when a refresher is
/// attached (see `with_refresher` on the clients) and only on a 401 — then replays the
/// request with the returned value. Implementations evict the rejected credential and
/// re-mint from the on-disk token cache.
pub trait TokenRefresher: Debug + Send + Sync {
    /// Full replacement **value** for the auth header — `skypetoken=<jwt>` for the chat
    /// service, `Bearer <jwt>` for the bearer services.
    ///
    /// # Errors
    ///
    /// [`ApiError::Auth`] when re-minting fails (signed out, refresh token rejected, …),
    /// [`ApiError::Config`] when there is no token cache to mint from.
    fn refresh(&self) -> RefreshFuture<'_>;
}

/// The cache is gone (signed out in another process while we were running).
fn no_cache_error() -> ApiError {
    ApiError::Config("token cache not found — run `teams-cli auth login`".into())
}

/// Re-mints the chat skypetoken by re-running the authz exchange (401 from the chat
/// service).
///
/// Flow: reload the cache → [`TokenCache::evict`] the spaces token (a 401 says our
/// credentials are suspect; the family refresh token survives eviction) → redeem the FRT
/// → `POST /v1.0/authz` → persist the new session → hand back `skypetoken=<jwt>`.
#[derive(Debug, Clone)]
pub struct AuthzRefresher {
    /// Where the token cache lives (default [`TokenCache::path`]; injectable for tests).
    cache_path: PathBuf,
    /// Tenant path segment for the FRT redemption — `common`, like the login flow.
    tenant: String,
}

impl AuthzRefresher {
    /// Refresher over the default on-disk cache.
    #[must_use]
    pub fn new() -> Self {
        Self { cache_path: TokenCache::path(), tenant: "common".to_owned() }
    }

    /// Read/write a different cache file (tests, non-default state directories).
    #[must_use]
    pub fn with_cache_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.cache_path = path.into();
        self
    }

    /// Tenant path segment for the redemption (`organizations`, a tenant id, …).
    #[must_use]
    pub fn with_tenant(mut self, tenant: impl Into<String>) -> Self {
        self.tenant = tenant.into();
        self
    }
}

impl Default for AuthzRefresher {
    fn default() -> Self {
        Self::new()
    }
}

impl TokenRefresher for AuthzRefresher {
    fn refresh(&self) -> RefreshFuture<'_> {
        let cache_path = self.cache_path.clone();
        let tenant = self.tenant.clone();
        Box::pin(async move {
            let mut cache = TokenCache::load_from(&cache_path)?.ok_or_else(no_cache_error)?;

            // Never trust the cached spaces token mid-recovery: the 401 may be a
            // server-side revocation our local expiry clock cannot see. Eviction keeps
            // the FRT, which is what actually re-mints.
            let evicted = cache.evict(AUDIENCE_SPACES);
            tracing::debug!(evicted, "spaces credential evicted before re-mint");
            let access = cache.fresh_or_redeem(AUDIENCE_SPACES, SCOPE_SPACES, &tenant).await?;
            // Persist the re-minted token even if the exchange below fails (same policy
            // as `teams-cli auth authz`): the redemption already happened.
            cache.save_to(&cache_path)?;

            let session = teams_auth::AuthzClient::new().exchange(&access).await?;
            let header = format!("skypetoken={}", session.skype_token);
            cache.authz = Some(session);
            cache.save_to(&cache_path)?;
            tracing::info!("chat credential re-minted via authz exchange");
            Ok(header)
        })
    }
}

/// Re-mints a per-audience bearer (CSA roster, substrate, …) from the cached family
/// refresh token after a 401.
///
/// Flow: reload the cache → [`TokenCache::evict`] the audience (its access token was just
/// rejected; the FRT survives) → redeem → persist → hand back `Bearer <jwt>`.
#[derive(Debug, Clone)]
pub struct AudienceRefresher {
    audience: &'static str,
    scope: &'static str,
    tenant: String,
    cache_path: PathBuf,
}

impl AudienceRefresher {
    /// Refresher for one audience/scope pair over the default on-disk cache.
    #[must_use]
    pub fn new(audience: &'static str, scope: &'static str) -> Self {
        Self { audience, scope, tenant: "common".to_owned(), cache_path: TokenCache::path() }
    }

    /// Read/write a different cache file (tests, non-default state directories).
    #[must_use]
    pub fn with_cache_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.cache_path = path.into();
        self
    }

    /// Tenant path segment for the redemption (`organizations`, a tenant id, …).
    #[must_use]
    pub fn with_tenant(mut self, tenant: impl Into<String>) -> Self {
        self.tenant = tenant.into();
        self
    }
}

impl TokenRefresher for AudienceRefresher {
    fn refresh(&self) -> RefreshFuture<'_> {
        let audience = self.audience;
        let scope = self.scope;
        let tenant = self.tenant.clone();
        let cache_path = self.cache_path.clone();
        Box::pin(async move {
            let mut cache = TokenCache::load_from(&cache_path)?.ok_or_else(no_cache_error)?;

            let evicted = cache.evict(audience);
            tracing::debug!(audience, evicted, "credential evicted before re-mint");
            let access = cache.fresh_or_redeem(audience, scope, &tenant).await?;
            cache.save_to(&cache_path)?;
            tracing::info!(audience, "credential re-minted from the family refresh token");
            Ok(format!("Bearer {access}"))
        })
    }
}
