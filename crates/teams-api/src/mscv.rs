//! MS-CV correlation vectors (the `MS-CV` request header).
//!
//! Format follows Microsoft's correlation-vector spec (v1):
//! `<16 base64 chars of a GUID>.<extension>`, where the extension starts at `0` for the
//! seed and is incremented **before** each outbound request, so a service that receives a
//! vector extends it with its own `.0` component (observed live on the chat service,
//! 2026-10-27: sending `…i1` came back as `…i1.0` in both `MS-CV` and `ContextId`).
//!
//! Keeping a per-process vector is what makes a request findable in Microsoft's support
//! tooling: quote the value from the response (or our own log line) when reporting a bad
//! response — `docs/endpoints.md` says which services echo it.

use std::sync::atomic::{AtomicU32, Ordering};

/// Header name carrying the correlation vector.
pub const MS_CV_HEADER: &str = "MS-CV";

/// Base length of a v1 vector: the first 16 characters of the base64-encoded GUID.
const V1_BASE_LEN: usize = 16;

/// A client-side MS-CV generator: one per process/client, extension counting up.
#[derive(Debug)]
pub struct CorrelationVector {
    /// Random per-process base (16 base64 characters).
    base: String,
    /// Per-request extension; incremented before each outbound request.
    extension: AtomicU32,
}

impl CorrelationVector {
    /// Seed a new vector from a fresh random GUID.
    #[must_use]
    pub fn new() -> Self {
        Self::from_uuid(uuid::Uuid::new_v4())
    }

    /// Seed a new vector from a specific GUID (deterministic, used by tests).
    #[must_use]
    pub fn from_uuid(id: uuid::Uuid) -> Self {
        use base64::{Engine as _, engine::general_purpose::STANDARD};

        let encoded = STANDARD.encode(id.into_bytes()); // 24 chars for 16 bytes
        Self {
            base: encoded.get(..V1_BASE_LEN).unwrap_or(&encoded).to_owned(),
            extension: AtomicU32::new(0),
        }
    }

    /// Value for the next outbound request — increments first, per the spec
    /// ("increment before passing the value to an outbound message header").
    #[must_use]
    pub fn next(&self) -> String {
        let extension = self.extension.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        format!("{}.{extension}", self.base)
    }

    /// The unextended base, for log correlation across all requests of this client.
    #[must_use]
    pub fn base(&self) -> &str {
        &self.base
    }
}

impl Default for CorrelationVector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The GUID is what the spec derives the base from: 16 base64 characters.
    #[test]
    fn base_is_the_first_16_base64_chars_of_the_guid() {
        use base64::Engine as _;

        // A nil GUID has a known base64 encoding, so the expected prefix is exact.
        let vector = CorrelationVector::from_uuid(uuid::Uuid::nil());
        let encoded = base64::engine::general_purpose::STANDARD.encode([0u8; 16]);
        assert_eq!(vector.base().len(), V1_BASE_LEN);
        assert_eq!(vector.base(), &encoded[..V1_BASE_LEN]);
        assert!(vector.base().chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/'));
    }

    #[test]
    fn extension_starts_at_one_and_counts_up() {
        let vector = CorrelationVector::from_uuid(uuid::Uuid::nil());
        let base = vector.base().to_owned();

        assert_eq!(vector.next(), format!("{base}.1"), "seed .0 is not sent on the wire");
        assert_eq!(vector.next(), format!("{base}.2"));
        assert_eq!(vector.next(), format!("{base}.3"));
    }

    /// Distinct GUIDs must not produce the same vector (else traces collide).
    #[test]
    fn fresh_vectors_have_distinct_bases() {
        let first = CorrelationVector::new();
        let second = CorrelationVector::new();
        assert_ne!(first.base(), second.base());
    }
}
