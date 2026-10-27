//! Persistent token cache, serialized as one JSON document behind a [`CredentialStore`].
//!
//! The public API comes in three flavours: [`TokenCache::load`]/[`TokenCache::save`]/
//! [`TokenCache::clear`] use the store selected by [`crate::default_store`] (the file
//! store unless `TEAMS_RS_CREDENTIAL_STORE` says otherwise), the
//! [`TokenCache::load_store`]/[`TokenCache::save_store`]/[`TokenCache::clear_store`]
//! trio takes an explicit store, and [`TokenCache::load_from`]/[`TokenCache::save_to`]
//! take an explicit path (always the file store — what tests use, so they never mutate
//! process environment: `std::env::set_var` is unsafe in edition 2024 and this crate
//! forbids unsafe code).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::store::{CredentialStore, FileStore, default_store};
use crate::{AuthError, TokenSet};

/// Current on-disk schema version (bump to invalidate old caches).
const CACHE_VERSION: u32 = 1;

/// Identity decoded from the access token's claims (display/metadata only, unverified).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedIdentity {
    /// Tenant (directory) id.
    pub tid: String,
    /// User principal name or preferred username, when present.
    #[serde(default)]
    pub upn: Option<String>,
    /// Display name, when present.
    #[serde(default)]
    pub name: Option<String>,
}

/// On-disk token cache document.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenCache {
    /// Schema version.
    pub version: u32,
    /// Last successful write.
    #[serde(default)]
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Signed-in identity (from the cached spaces token claims).
    #[serde(default)]
    pub identity: Option<CachedIdentity>,
    /// Access/refresh tokens keyed by audience label (see [`crate::AUDIENCE_SPACES`]).
    #[serde(default)]
    pub tokens: BTreeMap<String, TokenSet>,
    /// Last successful authz exchange (skypetoken + region map).
    #[serde(default)]
    pub authz: Option<crate::AuthzSession>,
}

impl TokenCache {
    /// Fresh, empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self { version: CACHE_VERSION, ..Self::default() }
    }

    /// `$XDG_STATE_HOME/teams-rs/token-cache.json`, falling back to
    /// `$HOME/.local/state/…` per the XDG base directory spec.
    ///
    /// # Panics
    ///
    /// Panics if neither `XDG_STATE_HOME` nor `HOME` is set — a headless environment
    /// where no state can be stored anyway.
    #[must_use]
    pub fn path() -> PathBuf {
        let base = match std::env::var_os("XDG_STATE_HOME") {
            Some(dir) if !dir.is_empty() && Path::new(&dir).is_absolute() => PathBuf::from(dir),
            _ => {
                let home = std::env::var_os("HOME")
                    .expect("neither XDG_STATE_HOME nor HOME is set")
                    .to_string_lossy()
                    .into_owned();
                PathBuf::from(home).join(".local/state")
            }
        };
        base.join("teams-rs").join("token-cache.json")
    }

    /// Load from the store selected by [`crate::default_store`]; `Ok(None)` when nothing
    /// is stored yet (signed out).
    ///
    /// # Errors
    ///
    /// [`AuthError::Io`] for file failures other than "not found", [`AuthError::Json`]
    /// when the document is corrupt, [`AuthError::Store`] when the selected backend is
    /// unavailable (e.g. `keyring` without a Secret Service daemon).
    pub fn load() -> Result<Option<Self>, AuthError> {
        let store = default_store()?;
        Self::load_store(&*store)
    }

    /// Load from an explicit store.
    ///
    /// # Errors
    ///
    /// Same as [`TokenCache::load`].
    pub fn load_store(store: &dyn CredentialStore) -> Result<Option<Self>, AuthError> {
        let Some(bytes) = store.read()? else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_slice(&bytes)?))
    }

    /// Load from an explicit file path (used by tests and alternative state locations).
    ///
    /// # Errors
    ///
    /// Same as [`TokenCache::load`].
    pub fn load_from(path: &Path) -> Result<Option<Self>, AuthError> {
        Self::load_store(&FileStore::new(path))
    }

    /// Persist to the store selected by [`crate::default_store`].
    ///
    /// # Errors
    ///
    /// [`AuthError::Io`] / [`AuthError::Store`] on backend failures, [`AuthError::Json`]
    /// on serialization.
    pub fn save(&self) -> Result<(), AuthError> {
        let store = default_store()?;
        self.save_store(&*store)
    }

    /// Persist to an explicit store.
    ///
    /// # Errors
    ///
    /// Same as [`TokenCache::save`].
    pub fn save_store(&self, store: &dyn CredentialStore) -> Result<(), AuthError> {
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        store.write(&bytes)
    }

    /// Persist to an explicit file path (parent directories are created).
    ///
    /// # Errors
    ///
    /// Same as [`TokenCache::save`].
    pub fn save_to(&self, path: &Path) -> Result<(), AuthError> {
        self.save_store(&FileStore::new(path))
    }

    /// Delete the credentials in the store selected by [`crate::default_store`].
    /// Returns `true` when something was removed.
    ///
    /// # Errors
    ///
    /// [`AuthError::Io`] / [`AuthError::Store`] on backend failures.
    pub fn clear() -> Result<bool, AuthError> {
        let store = default_store()?;
        Self::clear_store(&*store)
    }

    /// Delete the credentials in an explicit store.
    ///
    /// # Errors
    ///
    /// Same as [`TokenCache::clear`].
    pub fn clear_store(store: &dyn CredentialStore) -> Result<bool, AuthError> {
        store.remove()
    }

    /// Delete an explicit cache file (used by tests).
    ///
    /// # Errors
    ///
    /// Same as [`TokenCache::clear`].
    pub fn clear_at(path: &Path) -> Result<bool, AuthError> {
        Self::clear_store(&FileStore::new(path))
    }

    /// Look up a token by audience label.
    ///
    /// # Errors
    ///
    /// [`AuthError::NoToken`] when the audience is absent.
    pub fn token(&self, audience: &str) -> Result<&TokenSet, AuthError> {
        self.tokens.get(audience).ok_or_else(|| AuthError::NoToken(audience.to_owned()))
    }

    /// Store (or replace) a token for `audience`, updating `updated_at`.
    pub fn put_token(&mut self, audience: impl Into<String>, token: TokenSet) {
        self.tokens.insert(audience.into(), token);
        self.updated_at = Some(chrono::Utc::now());
        if self.version == 0 {
            self.version = CACHE_VERSION;
        }
    }

    /// A usable (non-expired) token for `audience`, or `None` if missing/expired.
    #[must_use]
    pub fn fresh_token(&self, audience: &str) -> Option<&TokenSet> {
        self.tokens.get(audience).filter(|t| !t.is_expired(crate::EXPIRY_SKEW_SECS))
    }

    /// Forget the cached access token for `audience` while keeping its refresh token.
    ///
    /// This is the "evict" half of 401 recovery: a service may reject a credential that
    /// still looks fresh locally (server-side revocation, clock skew), so the next mint
    /// must redeem the family refresh token instead of reusing the rejected value. The
    /// refresh token is deliberately kept — it *is* the means of re-minting, and for the
    /// first-party audience it may be the only copy in the cache.
    ///
    /// Returns `true` when an access token was actually dropped.
    #[must_use]
    pub fn evict(&mut self, audience: &str) -> bool {
        let Some(token) = self.tokens.get_mut(audience) else {
            return false;
        };
        if token.access_token.is_empty() && token.obtained_at.is_none() {
            return false; // already evicted
        }
        token.access_token = String::new();
        token.obtained_at = None; // `TokenSet::is_expired` treats never-stamped as expired
        true
    }

    /// A refresh token to redeem when minting a token for `audience`.
    ///
    /// Preference order: the audience's own refresh token (a mint already happened once),
    /// then the spaces token's — that is the one `auth login` always writes — then any
    /// cached token. All of these are family refresh tokens (FOCI), so any of them can
    /// redeem for another first-party audience.
    #[must_use]
    pub fn refresh_token_for(&self, audience: &str) -> Option<&str> {
        let own = self.tokens.get(audience).and_then(|token| token.refresh_token.as_deref());
        let preferred = own.or_else(|| {
            self.tokens.get(crate::AUDIENCE_SPACES).and_then(|token| token.refresh_token.as_deref())
        });
        preferred.or_else(|| self.tokens.values().find_map(|token| token.refresh_token.as_deref()))
    }

    /// A usable access token for `audience`, redeeming a cached refresh token for `scope`
    /// when the cached one is missing or expired, and caching the result under `audience`.
    ///
    /// This is the per-audience minting path (CSA roster, substrate, …): one device-code
    /// sign-in yields a family refresh token that mints bearers for every first-party
    /// audience. No network happens when the cached token is still fresh.
    ///
    /// # Errors
    ///
    /// [`AuthError::NoToken`] when no refresh token is cached anywhere, [`AuthError::Http`]
    /// / [`AuthError::Identity`] when the identity platform rejects the redemption.
    pub async fn fresh_or_redeem(
        &mut self,
        audience: &str,
        scope: &str,
        tenant: &str,
    ) -> Result<String, AuthError> {
        if let Some(token) = self.fresh_token(audience) {
            return Ok(token.access_token.clone());
        }

        let refresh = self
            .refresh_token_for(audience)
            .map(str::to_owned)
            .ok_or_else(|| AuthError::NoToken(audience.to_owned()))?;

        let minted = crate::DeviceCodeClient::new(tenant, crate::FOCI_CLIENT_ID)
            .refresh(&refresh, scope)
            .await?;
        let access = minted.access_token.clone();
        self.put_token(audience, minted);
        Ok(access)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique path under the system temp dir; removed on drop.
    struct TempCache(PathBuf);

    impl TempCache {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "teams-rs-cache-test-{}-{}.json",
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
            ));
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempCache {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn sample_token() -> TokenSet {
        TokenSet {
            access_token: "header.payload.signature".into(),
            refresh_token: Some("frt".into()),
            token_type: "Bearer".into(),
            expires_in: 3600,
            scope: "s".into(),
            id_token: None,
            obtained_at: Some(chrono::Utc::now()),
        }
    }

    #[test]
    fn save_load_roundtrip_and_permissions() {
        let temp = TempCache::new();
        let mut cache = TokenCache::new();
        cache.put_token("spaces.skype", sample_token());
        cache.save_to(temp.path()).expect("save");

        let loaded = TokenCache::load_from(temp.path()).expect("load").expect("file exists");
        assert_eq!(loaded.version, CACHE_VERSION);
        assert!(loaded.token("spaces.skype").is_ok());
        assert!(loaded.token("graph").is_err());
        assert!(loaded.identity.is_none());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode =
                std::fs::metadata(temp.path()).expect("metadata").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "cache must not be group/world readable");
        }

        assert!(TokenCache::clear_at(temp.path()).expect("clear"));
        assert!(!TokenCache::clear_at(temp.path()).expect("clear again"));
    }

    #[test]
    fn missing_file_is_none_not_error() {
        let temp = TempCache::new();
        assert!(TokenCache::load_from(temp.path()).expect("no error").is_none());
    }

    #[test]
    fn corrupt_file_is_an_error() {
        let temp = TempCache::new();
        std::fs::write(temp.path(), b"{not json").expect("write");
        assert!(TokenCache::load_from(temp.path()).is_err());
    }

    #[test]
    fn fresh_token_filters_expired() {
        let mut cache = TokenCache::new();
        let mut token = sample_token();
        token.expires_in = 60; // inside the 300 s skew window
        cache.put_token("spaces.skype", token);
        assert!(cache.fresh_token("spaces.skype").is_none());
    }

    /// 401 recovery: forget the rejected access token, keep the FRT that re-mints it.
    #[test]
    fn evict_drops_the_access_token_but_keeps_the_refresh_token() {
        let mut cache = TokenCache::new();
        cache.put_token("chatsvcagg", sample_token());

        assert!(cache.evict("chatsvcagg"), "a live access token was dropped");
        assert!(cache.fresh_token("chatsvcagg").is_none(), "must not be handed out again");
        assert_eq!(
            cache.refresh_token_for("chatsvcagg"),
            Some("frt"),
            "the family refresh token is the means of re-minting and must survive"
        );

        let evicted = cache.token("chatsvcagg").expect("entry still present");
        assert!(evicted.access_token.is_empty(), "the rejected value is forgotten");
        assert!(evicted.obtained_at.is_none(), "never-stamped reads as expired");

        // Idempotent: a second evict has nothing left to drop.
        assert!(!cache.evict("chatsvcagg"));
    }

    /// Evicting an audience that was never cached is a no-op, not a panic.
    #[test]
    fn evict_of_unknown_audience_reports_false() {
        let mut cache = TokenCache::new();
        assert!(!cache.evict("graph"));
        assert!(cache.tokens.is_empty());
    }

    /// Preference: the audience's own FRT, then spaces, then any cached token.
    #[test]
    fn refresh_token_prefers_own_then_spaces_then_any() {
        let mut cache = TokenCache::new();
        // No tokens at all.
        assert_eq!(cache.refresh_token_for("chatsvcagg"), None);

        // Only an unrelated audience: any refresh token will do (all are family RTs).
        let mut other = sample_token();
        other.refresh_token = Some("other-frt".into());
        cache.put_token("substrate", other);
        assert_eq!(cache.refresh_token_for("chatsvcagg"), Some("other-frt"));

        // Spaces present: preferred over an unrelated audience.
        cache.put_token(crate::AUDIENCE_SPACES, sample_token());
        assert_eq!(cache.refresh_token_for("chatsvcagg"), Some("frt"));

        // The audience's own FRT wins once a mint has happened before.
        let mut own = sample_token();
        own.refresh_token = Some("own-frt".into());
        cache.put_token("chatsvcagg", own);
        assert_eq!(cache.refresh_token_for("chatsvcagg"), Some("own-frt"));
    }

    /// A still-fresh cached token must be returned without touching the network.
    #[tokio::test]
    async fn fresh_or_redeem_uses_cache_before_redeeming() {
        let mut cache = TokenCache::new();
        cache.put_token("chatsvcagg", sample_token());
        // A scope/tenant pair that could never reach the identity platform without
        // failing proves no redemption was attempted.
        let access = cache
            .fresh_or_redeem("chatsvcagg", "not a real scope", "not-a-tenant")
            .await
            .expect("fresh token short-circuits");
        assert_eq!(access, "header.payload.signature");
    }

    /// Expired token + no refresh token anywhere → `NoToken`, not a network call.
    #[tokio::test]
    async fn fresh_or_redeem_without_refresh_token_is_notoken() {
        let mut cache = TokenCache::new();
        let mut expired = sample_token();
        expired.expires_in = 1;
        expired.refresh_token = None;
        cache.put_token("chatsvcagg", expired);

        let err = cache
            .fresh_or_redeem("chatsvcagg", "s", "common")
            .await
            .expect_err("nothing to redeem");
        assert!(matches!(err, AuthError::NoToken(audience) if audience == "chatsvcagg"));
    }
}
