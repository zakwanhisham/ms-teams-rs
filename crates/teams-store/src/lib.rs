//! Local persistence for the Teams client.
//!
//! SQLite (bundled, no system dependency) storing:
//! - `conversations`, `messages`, `users`, `reactions` — cached history for instant loads
//! - `sync_cursor` — the realtime resume watermark
//! - `pending_outbox` — optimistic sends awaiting server acknowledgement / retry
//!
//! Schema changes go through explicit migrations (Phase 5). The database is a **cache**:
//! wiping it must never lose data that only exists on the server.

#![forbid(unsafe_code)]

use std::path::Path;

use rusqlite::Connection;
use serde_json::Value;
use thiserror::Error;

/// Store failures.
#[derive(Debug, Error)]
pub enum StoreError {
    /// SQLite layer (bundled rusqlite).
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// A JSON payload could not be stored or recovered (broken cache row).
    #[error("json offline cache error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Local cache + outbox.
///
/// One writer at a time is the intended usage (`&mut self` on writes); `messages()` and
/// other reads take `&self`.
pub struct Store {
    conn: Connection,
}

/// Schema revision the file is migrated to. `0` = fresh file.
pub const SCHEMA_VERSION: i64 = 1;

impl Store {
    /// Open (creating when missing) and migrate to [`SCHEMA_VERSION`].
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`] when the file cannot be opened or a migration fails.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let conn = Connection::open(path)?;
        Self::migrate(conn)
    }

    /// In-memory store for tests and callers that only want the outbox ephemeral.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`] (allocation failure).
    pub fn in_memory() -> Result<Self, StoreError> {
        Self::migrate(Connection::open_in_memory()?)
    }

    /// Apply schema migrations up to [`SCHEMA_VERSION`].
    ///
    /// Migrations are idempotent, keyed on `user_version`. Row data is never rewritten
    /// by one later than it was written with (forward-only).
    fn migrate(conn: Connection) -> Result<Self, StoreError> {
        let mut store = Self { conn };
        let current = store.version();
        if current < 1 {
            store.migrate_v0_to_v1()?;
        }
        debug_assert_eq!(store.version(), SCHEMA_VERSION.max(current));
        Ok(store)
    }

    /// Current schema version (`user_version`; 0 for a fresh file).
    #[must_use]
    pub fn version(&self) -> i64 {
        self.conn.query_row("PRAGMA user_version", [], |row| row.get(0)).unwrap_or(0)
    }

    /// v1: the Phase 5 history cache, resume cursor and outbox.
    fn migrate_v0_to_v1(&mut self) -> Result<(), StoreError> {
        self.conn.execute_batch(
            r"
            BEGIN;

            CREATE TABLE conversations (
                id            TEXT PRIMARY KEY,
                kind          TEXT,              -- wire `type` or thread-type guess
                display       TEXT,              -- imdisplayname/topic, last seen worth
                last_arrival  INTEGER,           -- ms epoch of the newest message seen
                consumption   TEXT,              -- opaque consumptionhorizon triple
                raw           TEXT,              -- sanitized snapshot of the wire object
                updated_at    INTEGER NOT NULL
            );

            CREATE TABLE messages (
                id            TEXT PRIMARY KEY,
                conv_id       TEXT NOT NULL,
                clientmsg_id  TEXT,
                arrival_ms    INTEGER,           -- OriginalArrivalTime / composetime epoch
                message_type  TEXT,
                edited        TEXT,              -- properties.skypeeditedid marker
                deleted_at    TEXT,              -- properties.deletetime bookkeeping
                sequence_id   INTEGER,
                raw           TEXT NOT NULL,     -- full wire object (casing preserved)
                CONSTRAINT idx_messages_conv UNIQUE (id, conv_id)
            );
            CREATE INDEX idx_messages_conv_arrival
                ON messages (conv_id, arrival_ms);

            CREATE TABLE users (
                mri           TEXT PRIMARY KEY,  -- 8:orgid:<guid> / 8:teamsvisitor:…
                display       TEXT,              -- last imdisplayname seen (not authoritative)
                raw           TEXT
            );

            CREATE TABLE reactions (
                message_id    TEXT NOT NULL,
                conv_id       TEXT NOT NULL,
                user_mri      TEXT NOT NULL,
                key           TEXT NOT NULL,     -- like / heart / …
                time_ms       INTEGER,
                value         TEXT,
                PRIMARY KEY (message_id, user_mri, key)
            );

            CREATE TABLE sync_cursor (
                name          TEXT PRIMARY KEY,  -- e.g. 'longpoll', 'detailFrom'
                value         TEXT NOT NULL
            );

            CREATE TABLE pending_outbox (
                rowid_row     INTEGER PRIMARY KEY AUTOINCREMENT,
                conv_id       TEXT NOT NULL,
                clientmsg_id  INTEGER NOT NULL UNIQUE,
                content       TEXT NOT NULL,
                state         TEXT NOT NULL DEFAULT 'pending',  -- pending|sent|failed
                attempts      INTEGER NOT NULL DEFAULT 0,
                last_error    TEXT,
                server_id     TEXT,              -- filled on ack (the Location id)
                created_at    INTEGER NOT NULL
            );
            CREATE INDEX idx_outbox_pending ON pending_outbox (state, created_at);

            COMMIT;
            ",
        )?;
        self.set_version(1)
    }

    fn set_version(&mut self, version: i64) -> Result<(), StoreError> {
        self.conn.pragma_update(None, "user_version", version)?;
        Ok(())
    }

    /// Cache one decoded wire message (full object kept, casing preserved).
    ///
    /// Idempotent on (id, conv) — an update (edit/delete/reaction) rewrites the raw
    /// snapshot and the derived columns. Arrival is read from the message's
    /// `originalarrivaltime` when present; it is a timestamp *string* on the wire, so
    /// numeric-looking values are stored as ms epoch for ordering and anything else
    /// stays unindexed (`NULL`).
    ///
    /// # Errors
    ///
    /// [`StoreError::Json`] on a non-serializable payload, [`StoreError::Sqlite`].
    pub fn put_message(&mut self, message: &teams_protocol::Message) -> Result<(), StoreError> {
        let raw = serde_json::to_string(message)?;
        let arrival_ms = message.original_arrival_time.as_deref().and_then(hermes_epoch_ms);
        let conv_id = message.conversation_id.clone().unwrap_or_default();
        self.conn.execute(
            r"INSERT INTO messages (id, conv_id, clientmsg_id, arrival_ms, message_type,
                                     edited, deleted_at, sequence_id, raw)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
               ON CONFLICT(id, conv_id) DO UPDATE SET
                   clientmsg_id = excluded.clientmsg_id,
                   arrival_ms   = excluded.arrival_ms,
                   message_type = excluded.message_type,
                   edited       = excluded.edited,
                   deleted_at   = excluded.deleted_at,
                   sequence_id  = excluded.sequence_id,
                   raw          = excluded.raw",
            rusqlite::params![
                message.id,
                conv_id,
                message.client_message_id,
                arrival_ms,
                message.message_type,
                message.skype_edited_id,
                message.properties.extra.get("deletetime").and_then(Value::as_str),
                message.sequence_id,
                raw,
            ],
        )?;
        Ok(())
    }

    /// Read back the cached messages of one conversation, oldest arrival first.
    ///
    /// Rows whose snapshot fails to re-parse are skipped (cache degradation, never a
    /// broken UI) — the raw JSON tolerates unknown fields by design.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn messages(
        &self,
        conv_id: &str,
        limit: u32,
    ) -> Result<Vec<teams_protocol::Message>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT raw FROM messages WHERE conv_id = ?1
             ORDER BY COALESCE(arrival_ms, 0), sequence_id DESC LIMIT ?2",
        )?;
        let rows = statement
            .query_map(rusqlite::params![conv_id, limit], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for raw in rows {
            let value: Value = serde_json::from_str(&raw?)?;
            if let Ok(message) = serde_json::from_value(value) {
                out.push(message);
            }
        }
        Ok(out)
    }

    /// Remember a conversation's read cursor (the opaque consumptionhorizon triple).
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn put_consumption_horizon(
        &mut self,
        conv_id: &str,
        triple: &str,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO conversations (id, consumption, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET consumption = excluded.consumption,
                                           updated_at  = excluded.updated_at",
            rusqlite::params![conv_id, triple, now_ms()],
        )?;
        Ok(())
    }

    /// Resume watermark of one cursor name, if it was ever set.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn get_cursor(&self, name: &str) -> Result<Option<String>, StoreError> {
        let value =
            self.conn.query_row("SELECT value FROM sync_cursor WHERE name = ?1", [name], |row| {
                row.get::<_, String>(0)
            });
        match value {
            Ok(stored) => Ok(Some(stored)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    /// Set the resume watermark of one cursor name.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn set_cursor(&mut self, name: &str, value: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO sync_cursor (name, value) VALUES (?1, ?2)
             ON CONFLICT(name) DO UPDATE SET value = excluded.value",
            rusqlite::params![name, value],
        )?;
        Ok(())
    }

    /// Queue a send for the outbox (deduplicated on `clientmessageid`).
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn enqueue_outbox(
        &mut self,
        conv_id: &str,
        clientmessageid: i64,
        content: &str,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT OR IGNORE INTO pending_outbox (conv_id, clientmsg_id, content, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![conv_id, clientmessageid, content, now_ms()],
        )?;
        Ok(())
    }

    /// Pending rows, oldest first, for idempotent flush on reconnect.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn pending_outbox(&self) -> Result<Vec<OutboxEntry>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT clientmsg_id, conv_id, content, attempts FROM pending_outbox
             WHERE state = 'pending' ORDER BY created_at LIMIT 100",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(OutboxEntry {
                clientmessageid: row.get(0)?,
                conv_id: row.get(1)?,
                content: row.get(2)?,
                attempts: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Mark one outbox row sent (server acked with a `Location` id).
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn mark_outbox_sent(
        &mut self,
        clientmessageid: i64,
        server_id: Option<&str>,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE pending_outbox SET state = 'sent', last_error = NULL,
                                     server_id = ?2 WHERE clientmsg_id = ?1",
            rusqlite::params![clientmessageid, server_id],
        )?;
        Ok(())
    }

    /// Bump the attempt counter of a failed flush (row stays `pending` — state and
    /// backoff are the caller's job).
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn mark_outbox_failed(
        &mut self,
        clientmessageid: i64,
        error: &str,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE pending_outbox SET attempts = attempts + 1, last_error = ?2
             WHERE clientmsg_id = ?1",
            rusqlite::params![clientmessageid, truncate(error, 300)],
        )?;
        Ok(())
    }
}

/// One queued send awaiting a retry/acknowledgement.
#[derive(Debug)]
pub struct OutboxEntry {
    /// The dedup key (ms-epoch id the client assigns).
    pub clientmessageid: i64,
    /// Target conversation.
    pub conv_id: String,
    /// Prepared content string (already wrapped in its `<div>` body).
    pub content: String,
    /// How many flush attempts failed before this read.
    pub attempts: i64,
}

/// Current wall clock, ms epoch.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis().try_into().unwrap_or(i64::MAX))
}

/// A timestamp string ("1793085256125" ms epoch / RFC-3339) to a strict ms epoch, only
/// for all-digit values; anything else becomes `None` (unsortable in the cache).
fn hermes_epoch_ms(timestamp: &str) -> Option<i64> {
    let trimmed = timestamp.trim();
    if trimmed.is_empty() || !trimmed.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    trimmed.parse().ok()
}

/// Truncate a string to `max` characters for fault-only columns (`last_error`).
fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message_fixture(id: &str, conv: &str, content: &str) -> teams_protocol::Message {
        serde_json::from_value(serde_json::json!({
            "type": "Message", "id": id, "clientmessageid": id,
            "composetime": "2026-10-27T04:34:16.125Z",
            "originalarrivaltime": id, // the traces carry ms-epoch strings here too
            "conversationid": conv,
            "messagetype": "RichText/Html", "contenttype": "text",
            "content": content,
            "sequenceId": 10,
        }))
        .expect("fixture decodes")
    }

    #[test]
    fn fresh_file_migrates_to_v1() -> Result<(), StoreError> {
        let store = Store::in_memory()?;
        assert_eq!(store.version(), 1);
        Ok(())
    }

    #[test]
    fn message_roundtrip_and_ordering() -> Result<(), StoreError> {
        let mut store = Store::in_memory()?;
        store.put_message(&message_fixture("2000", "48:notifications", "older"))?;
        store.put_message(&message_fixture("3000", "48:notifications", "newer"))?;
        store.put_message(&message_fixture("1000", "48:other-conv", "elsewhere"))?;

        let messages = store.messages("48:notifications", 10)?;
        assert_eq!(
            messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>(),
            vec!["older", "newer"],
        );
        // Limit applies.
        assert_eq!(store.messages("48:notifications", 1)?.len(), 1);
        Ok(())
    }

    #[test]
    fn message_update_rewrites_row() -> Result<(), StoreError> {
        let mut store = Store::in_memory()?;
        store.put_message(&message_fixture("2000", "48:notifications", "before"))?;
        let edited: teams_protocol::Message = serde_json::from_value(serde_json::json!({
            "type": "Message", "id": "2000", "conversationid": "48:notifications",
            "messagetype": "RichText/Html", "contenttype": "text", "content": "after",
            "originalarrivaltime": "2000",
            "skypeeditedid": "9000",
        }))?;
        store.put_message(&edited)?;
        let messages = store.messages("48:notifications", 10)?;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content.as_deref(), Some("after"));
        assert_eq!(messages[0].skype_edited_id.as_deref(), Some("9000"));
        Ok(())
    }

    #[test]
    fn cursor_roundtrip() -> Result<(), StoreError> {
        let mut store = Store::in_memory()?;
        assert_eq!(store.get_cursor("longpoll")?, None);
        store.set_cursor("longpoll", "1793085256125|seq")?;
        assert_eq!(store.get_cursor("longpoll")?.as_deref(), Some("1793085256125|seq"));
        store.set_cursor("longpoll", "advanced")?;
        assert_eq!(store.get_cursor("longpoll")?.as_deref(), Some("advanced"));
        Ok(())
    }

    #[test]
    fn consumption_horizon_upsert_keeps_raw_none_free() -> Result<(), StoreError> {
        let mut store = Store::in_memory()?;
        store.put_consumption_horizon("48:notifications", "1793085256125;1793091035864;0")?;
        store.put_consumption_horizon("48:notifications", "advanced;…;1")?;
        // One row per conversation, updated value kept.
        let count = store
            .conn
            .query_row("SELECT COUNT(*) FROM conversations", [], |row| row.get::<_, i64>(0))?;
        assert_eq!(count, 1);
        Ok(())
    }

    #[test]
    fn outbox_dedup_ack_and_retry_paths() -> Result<(), StoreError> {
        let mut store = Store::in_memory()?;
        store.enqueue_outbox("48:notifications", 1_793_000_000_000, "<div>one</div>")?;
        store.enqueue_outbox("48:notifications", 1_793_000_000_000, "<div>one</div>")?; // same id
        store.enqueue_outbox("48:notifications", 1_793_000_000_001, "<div>two</div>")?;

        assert_eq!(store.pending_outbox()?.len(), 2);
        store.mark_outbox_failed(1_793_000_000_001, "transport: unavailable")?;
        assert_eq!(
            store
                .pending_outbox()?
                .iter()
                .find(|entry| entry.clientmessageid == 1_793_000_000_001)
                .map(|entry| entry.attempts),
            Some(1),
        );
        store.mark_outbox_sent(1_793_000_000_001, Some("1793000000001"))?;
        let remaining = store.pending_outbox()?;
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].content, "<div>one</div>");

        // Sent rows stay queryable for reconciliation, not re-flushed.
        let sent_id = store.conn.query_row(
            "SELECT server_id FROM pending_outbox WHERE state='sent'",
            [],
            |row| row.get::<_, Option<String>>(0),
        )?;
        assert_eq!(sent_id.as_deref(), Some("1793000000001"));
        Ok(())
    }

    #[test]
    fn epoch_parsing_is_strict_digits() {
        assert_eq!(hermes_epoch_ms("1793085256125"), Some(1_793_085_256_125));
        assert_eq!(hermes_epoch_ms("2026-10-27T04:34:16.125Z"), None);
        assert_eq!(hermes_epoch_ms(""), None);
    }
}
