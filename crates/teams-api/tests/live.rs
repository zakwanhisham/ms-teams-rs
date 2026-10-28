//! Live integration tests, gated behind the `TEAMS_TEST_TOKENS` env variable.
//!
//! `TEAMS_TEST_TOKENS` points at a **gitignored** token-cache file (same shape as
//! `~/.local/state/teams-rs/token-cache.json` — owner-owned data, never committed).
//! Without the variable every test returns early (`Ok`), so default CI/`cargo test`
//! runs stay green and fast; the mocked suites in `tests/ops.rs` cover the wire shapes.
//!
//! Run against the real service with:
//!
//! ```bash
//! TEAMS_TEST_TOKENS=/path/to/token-cache.json cargo test -p teams-api --test live -- --ignored
//! ```

use teams_auth::TokenCache;

/// Load the gated cache; `None` (skip) when unset or unusable.
fn gated_cache() -> Option<teams_auth::AuthzSession> {
    let path = std::env::var_os("TEAMS_TEST_TOKENS")?;
    match TokenCache::load_from(std::path::Path::new(&path)) {
        Ok(Some(cache)) => cache.authz,
        _ => None,
    }
}

/// Sanity of the environment contract only: the gate is respected in both directions
/// (unset ⇒ nothing runs, so this must pass everywhere without touching the network).
#[test]
fn gate_is_off_by_default() {
    if std::env::var_os("TEAMS_TEST_TOKENS").is_none() {
        assert!(gated_cache().is_none());
    }
}

#[tokio::test]
#[ignore = "needs TEAMS_TEST_TOKENS (live owner data)"]
async fn send_and_delete_round_trip_live() -> Result<(), Box<dyn std::error::Error>> {
    let Some(session) = gated_cache() else { return Ok(()) };
    let client = teams_api::ChatClient::from_session(&session)?;
    // Safe probe target is the user's own notifications feed; content is inert text.
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis()
        .try_into()
        .map_err(|_| "millisecond timestamp exceeds i64")?;
    let body = format!("<div>teams-rs live round-trip {stamp}</div>");
    let sent = client.send_message("48:notifications", &body, stamp).await?;
    let Some(message_id) = sent.message_id.as_deref() else {
        return Err("no Location id on the 201".into());
    };
    client.delete_message("48:notifications", message_id).await?;
    Ok(())
}
