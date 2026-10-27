//! Minimal, signature-**unverified** JWT claim decoding for display/metadata.
//!
//! Verification is pointless here: the signature was produced by Microsoft and we only
//! ever decode tokens we just received over TLS. Never trust these values for authz
//! decisions — the server does that.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use crate::AuthError;

/// Decode the payload (second segment) of a JWT into JSON claims.
///
/// # Errors
///
/// [`AuthError::DeviceCode`] — reused as a generic "malformed token" variant — when the
/// input is not three dot-separated segments, is not base64url, or is not JSON.
pub fn decode_claims(jwt: &str) -> Result<serde_json::Value, AuthError> {
    let payload = jwt.split('.').nth(1).ok_or_else(|| {
        AuthError::DeviceCode("token is not a JWT (missing payload segment)".into())
    })?;

    let decoded = URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .map_err(|_| AuthError::DeviceCode("token payload is not base64url".into()))?;

    serde_json::from_slice(&decoded)
        .map_err(|_| AuthError::DeviceCode("token payload is not JSON".into()))
}

/// Extract the well-known identity fields from claims, tolerating their absence.
///
/// # Errors
///
/// Same as [`decode_claims`].
pub fn identity_from_claims(
    claims: &serde_json::Value,
) -> Result<crate::CachedIdentity, AuthError> {
    let tid = claims
        .get("tid")
        .and_then(serde_json::Value::as_str)
        .or_else(|| claims.get("oid").and_then(serde_json::Value::as_str))
        .ok_or_else(|| AuthError::DeviceCode("claims carry no tenant id".into()))?
        .to_owned();

    let upn = ["upn", "preferred_username", "email", "unique_name"]
        .iter()
        .find_map(|key| claims.get(*key).and_then(serde_json::Value::as_str))
        .map(str::to_owned);

    let name = claims.get("name").and_then(serde_json::Value::as_str).map(str::to_owned);

    Ok(crate::CachedIdentity { tid, upn, name })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an unsigned JWT from a claims object (tests only).
    fn fake_jwt(claims: &serde_json::Value) -> String {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string());
        format!("eyJhbGciOiJub25lIn0.{payload}.c2ln")
    }

    #[test]
    fn decodes_claims() {
        let jwt =
            fake_jwt(&serde_json::json!({"tid":"t-1","upn":"user@example.com","name":"User"}));
        let claims = decode_claims(&jwt).expect("decode");
        let identity = identity_from_claims(&claims).expect("identity");
        assert_eq!(identity.tid, "t-1");
        assert_eq!(identity.upn.as_deref(), Some("user@example.com"));
        assert_eq!(identity.name.as_deref(), Some("User"));
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode_claims("not-a-jwt").is_err());
        assert!(decode_claims("a.!!!not-base64!!!.c").is_err());
    }

    #[test]
    fn missing_tid_is_an_error() {
        let claims = serde_json::json!({"name": "User"});
        assert!(identity_from_claims(&claims).is_err());
    }
}
