//! Credential storage backends for the serialized token cache.
//!
//! The cache is one JSON document (identity, per-audience tokens, authz session). *Where*
//! those bytes live is pluggable behind [`CredentialStore`]:
//!
//! - [`FileStore`] — the default: a single `0600` file under `$XDG_STATE_HOME/teams-rs/`.
//! - [`KeyringStore`] (Linux) — the OS keyring via the freedesktop Secret Service
//!   (`org.freedesktop.secrets`: GNOME Keyring, `KWallet`'s secret-service wrapper,
//!   `oo7-daemon`), opt-in with `TEAMS_RS_CREDENTIAL_STORE=keyring`.
//!
//! # Backend decision (Phase 3, 2026-10-28)
//!
//! The kernel keyrings (keyutils) were evaluated first — they need no daemon and work
//! headless — and **rejected on measurement**:
//!
//! - The per-user quota `/proc/sys/kernel/keys/maxbytes` defaults to 20 000 bytes and is
//!   shared by every key the user holds. A realistic cache measured **30 320 bytes**
//!   (three audiences + authz session), and `add_key` fails with *Disk quota exceeded*
//!   (EDQUOT) well before that — even a second 15 KB key overflows the quota.
//! - Kernel keyrings are cleared on reboot, so tokens would not survive a restart; the
//!   `0600` file already does.
//!
//! The Secret Service has no such quota, persists across reboots, and is the standard
//! desktop keystore — so it is the keyring backend behind this trait. Notes:
//!
//! - `KWallet`'s secret-service wrapper only stores UTF-8 strings; our payload is JSON
//!   (always UTF-8), so it round-trips through `KWallet` as-is.
//! - WSL has no default collection, so the keyring store does not work there (use the
//!   file store).
//! - The store is *not* a migration: switching `TEAMS_RS_CREDENTIAL_STORE` does not move
//!   an existing cache between backends — re-run `teams-cli auth login`.

use std::fmt::Debug;
use std::path::{Path, PathBuf};

use crate::AuthError;

/// Environment variable selecting the store behind [`crate::TokenCache::load`] /
/// [`crate::TokenCache::save`] / [`crate::TokenCache::clear`].
///
/// Recognised values: `file` (default) and `keyring`.
pub const STORE_ENV: &str = "TEAMS_RS_CREDENTIAL_STORE";

/// Production keyring entry: one cache document, labelled for password-manager UIs as
/// `keyring:token-cache@teams-rs`.
const KEYRING_SERVICE: &str = "teams-rs";
/// See `KEYRING_SERVICE`.
const KEYRING_USER: &str = "token-cache";

/// Where the serialized token cache bytes live.
///
/// Object-safe on purpose: the CLI and the 401 refreshers hold `Box<dyn CredentialStore>`
/// so one process reads and writes the same backend no matter which code path got there.
pub trait CredentialStore: Debug + Send + Sync {
    /// The stored bytes, or `None` when nothing is stored yet.
    ///
    /// # Errors
    ///
    /// [`AuthError::Io`] (file), [`AuthError::Store`] (keyring) on backend failures.
    /// "Not found" is `Ok(None)`, never an error.
    fn read(&self) -> Result<Option<Vec<u8>>, AuthError>;

    /// Persist `bytes`, replacing any previous value.
    ///
    /// # Errors
    ///
    /// Same as [`CredentialStore::read`].
    fn write(&self, bytes: &[u8]) -> Result<(), AuthError>;

    /// Delete whatever is stored; `true` when a value was actually removed.
    ///
    /// # Errors
    ///
    /// Same as [`CredentialStore::read`].
    fn remove(&self) -> Result<bool, AuthError>;

    /// Short human-readable description for status output and logs.
    #[must_use]
    fn describe(&self) -> String;
}

/// The default backend: one JSON file with mode `0600`.
#[derive(Debug, Clone)]
pub struct FileStore {
    path: PathBuf,
}

impl FileStore {
    /// Store the cache at `path` (parent directories are created on write).
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The file this store reads and writes.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl CredentialStore for FileStore {
    fn read(&self) -> Result<Option<Vec<u8>>, AuthError> {
        match std::fs::read(&self.path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(AuthError::Io(err)),
        }
    }

    fn write(&self, bytes: &[u8]) -> Result<(), AuthError> {
        use std::io::Write;

        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts.open(&self.path)?;
        file.write_all(bytes)?;
        file.sync_all()?;

        // Tighten permissions even when the file pre-existed with a wider mode.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    fn remove(&self) -> Result<bool, AuthError> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(true),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(err) => Err(AuthError::Io(err)),
        }
    }

    fn describe(&self) -> String {
        format!("file ({})", self.path.display())
    }
}

/// OS keyring backend: the whole cache document as one Secret Service item.
///
/// See the module docs for why keyutils was rejected and this backend chosen. Construction
/// only opens a store handle; a missing daemon surfaces on the first [`CredentialStore`]
/// call as [`AuthError::Store`].
#[cfg(target_os = "linux")]
#[derive(Debug)]
pub struct KeyringStore {
    entry: keyring_core::Entry,
}

#[cfg(target_os = "linux")]
impl KeyringStore {
    /// The production entry (see `KEYRING_SERVICE` / `KEYRING_USER`).
    ///
    /// # Errors
    ///
    /// [`AuthError::Store`] when the Secret Service cannot be opened.
    pub fn new() -> Result<Self, AuthError> {
        Self::with_identity(KEYRING_SERVICE, KEYRING_USER)
    }

    /// An entry with explicit labels — tests use unique names so they can never touch a
    /// real login's keyring item.
    ///
    /// # Errors
    ///
    /// Same as [`KeyringStore::new`].
    pub fn with_identity(service: &str, user: &str) -> Result<Self, AuthError> {
        use keyring_core::api::CredentialStoreApi;

        let store =
            dbus_secret_service_keyring_store::Store::new().map_err(|err| keyring_error(&err))?;
        let entry = store.build(service, user, None).map_err(|err| keyring_error(&err))?;
        Ok(Self { entry })
    }
}

/// Map a keyring error onto [`AuthError::Store`] (the error enum is non-exhaustive, so
/// every variant funnels through its `Display`).
#[cfg(target_os = "linux")]
fn keyring_error(err: &keyring_core::Error) -> AuthError {
    AuthError::Store(err.to_string())
}

#[cfg(target_os = "linux")]
impl CredentialStore for KeyringStore {
    fn read(&self) -> Result<Option<Vec<u8>>, AuthError> {
        match self.entry.get_secret() {
            Ok(bytes) => Ok(Some(bytes)),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(err) => Err(keyring_error(&err)),
        }
    }

    fn write(&self, bytes: &[u8]) -> Result<(), AuthError> {
        self.entry.set_secret(bytes).map_err(|err| keyring_error(&err))
    }

    fn remove(&self) -> Result<bool, AuthError> {
        match self.entry.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring_core::Error::NoEntry) => Ok(false),
            Err(err) => Err(keyring_error(&err)),
        }
    }

    fn describe(&self) -> String {
        "OS keyring (org.freedesktop.secrets)".to_owned()
    }
}

/// The store named by `spec`: `file` (also the empty string) or `keyring`.
///
/// # Errors
///
/// [`AuthError::Store`] for unknown names and for a keyring that cannot be opened
/// (no Secret Service daemon, WSL, non-Linux target).
pub fn store_for(spec: &str) -> Result<Box<dyn CredentialStore>, AuthError> {
    match spec.trim().to_ascii_lowercase().as_str() {
        "" | "file" => Ok(Box::new(FileStore::new(crate::TokenCache::path()))),
        "keyring" => keyring_store(),
        other => Err(AuthError::Store(format!(
            "unknown credential store `{other}` (expected `file` or `keyring`)"
        ))),
    }
}

/// The store selected by [`STORE_ENV`] — the file store unless the environment says
/// otherwise. Read per call so a long-lived process never caches a stale selection.
///
/// # Errors
///
/// Same as [`store_for`].
pub fn default_store() -> Result<Box<dyn CredentialStore>, AuthError> {
    store_for(&std::env::var(STORE_ENV).unwrap_or_default())
}

#[cfg(target_os = "linux")]
fn keyring_store() -> Result<Box<dyn CredentialStore>, AuthError> {
    Ok(Box::new(KeyringStore::new()?))
}

#[cfg(not(target_os = "linux"))]
fn keyring_store() -> Result<Box<dyn CredentialStore>, AuthError> {
    Err(AuthError::Store("the keyring store requires Linux".to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CachedIdentity, TokenCache};

    /// In-memory backend: proves the abstraction is store-agnostic and gives
    /// `TokenCache` tests a store with no filesystem or keyring involved.
    #[derive(Debug, Default)]
    pub(crate) struct MemoryStore {
        bytes: std::sync::Mutex<Option<Vec<u8>>>,
    }

    impl MemoryStore {
        pub(crate) fn new() -> Self {
            Self::default()
        }
    }

    impl CredentialStore for MemoryStore {
        fn read(&self) -> Result<Option<Vec<u8>>, AuthError> {
            Ok(self.bytes.lock().expect("poisoned").clone())
        }

        fn write(&self, bytes: &[u8]) -> Result<(), AuthError> {
            *self.bytes.lock().expect("poisoned") = Some(bytes.to_vec());
            Ok(())
        }

        fn remove(&self) -> Result<bool, AuthError> {
            Ok(self.bytes.lock().expect("poisoned").take().is_some())
        }

        fn describe(&self) -> String {
            "memory".to_owned()
        }
    }

    /// A unique keyring identity so a test can never collide with the production entry
    /// (or with a parallel test run).
    fn test_identity(test: &str) -> String {
        format!(
            "{test}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        )
    }

    /// Open a throwaway keyring entry, or skip (locally) / fail (CI, where
    /// `TEAMS_RS_REQUIRE_KEYRING=1` is set) when no Secret Service is reachable.
    fn keyring_or_skip(test: &str) -> Option<KeyringStore> {
        // CI requires the backend to be tested: a missing Secret Service is a failure
        // there, but a plain skip on a developer machine without a daemon.
        let required = std::env::var_os("TEAMS_RS_REQUIRE_KEYRING").is_some();
        let unavailable = |err: AuthError| {
            assert!(
                !required,
                "{test}: Secret Service unavailable with TEAMS_RS_REQUIRE_KEYRING set: {err}"
            );
            eprintln!("skipping {test}: no Secret Service available ({err})");
        };

        let user = test_identity(test);
        let store = match KeyringStore::with_identity("teams-rs-test", &user) {
            Ok(store) => store,
            Err(err) => {
                unavailable(err);
                return None;
            }
        };
        // The bus can be up while the daemon is not: only a real round-trip proves it.
        match store.write(b"probe") {
            Ok(()) => {
                let _ = store.remove();
                Some(store)
            }
            Err(err) => {
                unavailable(err);
                None
            }
        }
    }

    #[test]
    fn file_store_roundtrip_and_permissions() {
        let path = std::env::temp_dir().join(format!(
            "teams-rs-store-test-{}-{}.json",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let store = FileStore::new(&path);
        let _guard = scopeguard(&path);

        assert_eq!(store.read().expect("read empty"), None, "nothing stored yet");
        assert!(!store.remove().expect("remove empty"), "nothing to remove");

        store.write(b"{\"hello\":\"world\"}\n").expect("write");
        assert_eq!(
            store.read().expect("read").as_deref(),
            Some(b"{\"hello\":\"world\"}\n".as_slice())
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "credentials must not be group/world readable");
        }

        assert!(store.remove().expect("remove"), "a value was removed");
        assert_eq!(store.read().expect("read after remove"), None);
        assert!(!store.remove().expect("remove again"), "idempotent");
    }

    #[test]
    fn unknown_store_spec_is_an_error() {
        let err = store_for("shiny-thing").expect_err("must not guess a backend");
        assert!(matches!(err, AuthError::Store(msg) if msg.contains("shiny-thing")));
    }

    #[test]
    fn empty_spec_selects_the_file_store() {
        let store = store_for("").expect("file store");
        assert!(store.describe().starts_with("file ("), "got: {}", store.describe());
    }

    /// The cache must behave identically over *any* store — that is what makes
    /// `TEAMS_RS_CREDENTIAL_STORE` a pure backend choice rather than a mode switch.
    #[test]
    fn token_cache_roundtrips_through_an_arbitrary_store() {
        let store = MemoryStore::new();
        assert!(TokenCache::load_store(&store).expect("load").is_none(), "empty store");

        let mut cache = TokenCache::new();
        cache.identity = Some(CachedIdentity {
            tid: "00000000-0000-0000-0000-000000000000".to_owned(),
            upn: Some("user@example.invalid".to_owned()),
            name: Some("Test User".to_owned()),
        });
        cache.save_store(&store).expect("save");

        let loaded = TokenCache::load_store(&store).expect("load").expect("stored");
        let identity = loaded.identity.expect("identity round-tripped");
        assert_eq!(identity.tid, "00000000-0000-0000-0000-000000000000");
        assert_eq!(identity.upn.as_deref(), Some("user@example.invalid"));
        assert_eq!(loaded.version, cache.version);

        assert!(TokenCache::clear_store(&store).expect("clear"), "a value was removed");
        assert!(TokenCache::load_store(&store).expect("load after clear").is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn keyring_store_roundtrip() {
        let Some(store) = keyring_or_skip("keyring_store_roundtrip") else {
            return;
        };

        // A realistic worst case: the measured cache is ~30 KB (three audiences plus the
        // authz session); the payload is JSON so UTF-8-only stores (KWallet) accept it.
        let payload = format!("{{\"pad\":\"{}\"}}", "x".repeat(40 * 1024)).into_bytes();

        store.write(&payload).expect("write");
        assert_eq!(store.read().expect("read").as_deref(), Some(payload.as_slice()));
        assert!(store.remove().expect("remove"), "value was removed");
        assert_eq!(store.read().expect("read after remove"), None, "gone");
        assert!(!store.remove().expect("remove again"), "idempotent");
    }

    /// Two refresh paths can 401 at the same time and both persist their re-minted
    /// credential: the backend must tolerate concurrent access from worker threads.
    #[cfg(target_os = "linux")]
    #[test]
    fn keyring_store_survives_concurrent_access() {
        let Some(store) = keyring_or_skip("keyring_store_survives_concurrent_access") else {
            return;
        };
        let store = std::sync::Arc::new(store);

        let threads: Vec<_> = (0..4)
            .map(|i| {
                let store = std::sync::Arc::clone(&store);
                std::thread::spawn(move || {
                    for round in 0..3 {
                        let payload = format!("{{\"t\":{i},\"r\":{round}}}");
                        store.write(payload.as_bytes()).expect("concurrent write");
                        let read = store.read().expect("concurrent read");
                        assert!(
                            read.is_some(),
                            "a write is in flight, a read must see *something*"
                        );
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().expect("worker panicked");
        }
        store.remove().expect("cleanup");
    }

    /// Remove a temp file on drop so a failing assertion does not leave litter behind.
    fn scopeguard(path: &Path) -> impl Drop {
        struct Guard(PathBuf);
        impl Drop for Guard {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        Guard(path.to_owned())
    }
}
