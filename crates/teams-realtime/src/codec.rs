//! Socket.io 0.9 packet codec, as the official trouter client (v4 protocol) speaks it.
//!
//! Encoding derived from the bundled parser (official client bundle quoted in
//! `docs/endpoints.md` "Trouter websocket"): a frame is colon-separated
//! `<type>:<id>[+]:<endpoint>[:<data>]`, and an event packet's data is the JSON object
//! `{"name": <event>, "args": [<args>]}` — **not** the bare array some socket.io versions
//! emit. Empty id/endpoint collapse: four empty-ish samples seen live are `1::` (connect),
//! `0:::{"reason":…}` (disconnect), `4:::<json>` (json data), `5:::{…}` (event).
//!
//! Frames seen live (2026-10-29):
//! - server → client on successful upgrade: `1::`
//! - server → client after ~10 s with no auth: `0:::{"reason":"timeout"}`;
//!   rejected auth: `reason: "badrequest"` / `"unauthorized"`
//! - client → server auth emit: `5:::{"name":"user.authenticate","args":[…]}`

use serde_json::Value;
use thiserror::Error;

/// Socket.io 0.9 packet type byte values seen on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketType {
    /// `0` — disconnect (`0:::{"reason":"…"}`).
    Disconnect,
    /// `1` — namespace connect (`1::`).
    Connect,
    /// `2` — heartbeat (ping/pong), unverified live.
    Heartbeat,
    /// `3` — message data (`3:::<payload>`), unverified live.
    Message,
    /// `4` — JSON data (`4:::<json>`), unverified live.
    Json,
    /// `5` — event emission.
    Event,
    /// `6` — acknowledgement, unverified live.
    Ack,
    /// `7` — error, unverified live.
    Error,
}

impl PacketType {
    /// Byte on the wire.
    #[must_use]
    pub const fn as_byte(self) -> u8 {
        match self {
            Self::Disconnect => 0,
            Self::Connect => 1,
            Self::Heartbeat => 2,
            Self::Message => 3,
            Self::Json => 4,
            Self::Event => 5,
            Self::Ack => 6,
            Self::Error => 7,
        }
    }

    /// Parse from a wire byte (an **ASCII digit**, e.g. `b'1'`); `None` for others.
    #[must_use]
    pub const fn from_byte(b: u8) -> Option<Self> {
        match b.checked_sub(b'0') {
            Some(0) => Some(Self::Disconnect),
            Some(1) => Some(Self::Connect),
            Some(2) => Some(Self::Heartbeat),
            Some(3) => Some(Self::Message),
            Some(4) => Some(Self::Json),
            Some(5) => Some(Self::Event),
            Some(6) => Some(Self::Ack),
            Some(7) => Some(Self::Error),
            _ => None,
        }
    }
}

/// One decoded socket.io packet.
#[derive(Debug, Clone, PartialEq)]
pub enum Packet {
    /// Namespace connect; carries the optional query tail after `1::`.
    Connect {
        /// Optional query string (empty in the live probe).
        qs: String,
    },
    /// Disconnect with raw payload (`{"reason":"timeout"}` etc.).
    Disconnect {
        /// Payload as received (empty when bare).
        payload: String,
    },
    /// An emitted event.
    Event {
        /// Event name (e.g. `user.authenticate`).
        name: String,
        /// Parsed args (empty when undecodable — degrade gracefully).
        args: Vec<Value>,
        /// Payload as received.
        raw: String,
    },
    /// Any packet shape this codec does not fully model; carried verbatim.
    Other(PacketType, String),
}

/// Codec errors.
///
/// Failure means one thing only: the frame matched no socket.io packet shape.
#[derive(Debug, Error)]
#[error("malformed socket.io frame: {0:?}")]
pub struct CodecError(pub String);

/// Parse the colon fields: `(type, id(+ack), endpoint, data?)`.
fn split_fields(frame: &str) -> Option<(PacketType, String, bool, String, Option<String>)> {
    let mut parts = frame.splitn(4, ':');
    let raw_type: &str = parts.next()?;
    let ptype = PacketType::from_byte(*raw_type.as_bytes().first()?)?;
    if raw_type.len() != 1 {
        return None;
    }
    let mut id = parts.next().unwrap_or("").to_string();
    let ack = id.ends_with('+');
    if ack {
        id.pop();
    }
    let endpoint = parts.next().unwrap_or("").to_string();
    let data = parts.next().map(str::to_string);
    Some((ptype, id, ack, endpoint, data))
}

/// Encode an event emit frame (object payload form): `5:::{"name":…,"args":[…]}`.
///
/// The optional `ack_id`/`ack` fields of the wire format are not emitted here — the
/// official client emits authentication without an ack — revisit if a reply-bearing
/// emit turns out to need one.
///
/// # Errors
/// Only when `args` cannot be serialized (practically unreachable for `Value`).
pub fn encode_event(name: &str, args: &[Value]) -> Result<String, CodecError> {
    let data = serde_json::to_string(&serde_json::json!({ "name": name, "args": args }))
        .map_err(|e| CodecError(format!("serialize {name}: {e}")))?;
    Ok(format!("5:::{data}"))
}

/// Decode one wire frame leniently: undecodable event payloads degrade to empty
/// `name`/`args` with the raw payload preserved (never lose server data).
///
/// # Errors
/// Fails only when the frame does not match the socket.io 0.9 shape at all.
pub fn decode(frame: &str) -> Result<Packet, CodecError> {
    let Some((ptype, _id, _ack, _endpoint, data)) = split_fields(frame) else {
        return Err(CodecError(frame.to_string()));
    };
    let data = data.unwrap_or_default();
    Ok(match ptype {
        PacketType::Connect => Packet::Connect { qs: data },
        PacketType::Disconnect => Packet::Disconnect { payload: data },
        PacketType::Event => decode_event(&data),
        PacketType::Json
        | PacketType::Message
        | PacketType::Heartbeat
        | PacketType::Ack
        | PacketType::Error => Packet::Other(ptype, data),
    })
}

fn decode_event(data: &str) -> Packet {
    let (name, args) = match serde_json::from_str::<Value>(data) {
        // Object form (what the official client sends).
        Ok(Value::Object(map)) => (
            map.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
            match map.get("args") {
                Some(Value::Array(args)) => args.clone(),
                _ => Vec::new(),
            },
        ),
        // Bare-array form tolerated (`["name", args…]`).
        Ok(Value::Array(items)) if items.len() == 2 => (
            items[0].as_str().unwrap_or_default().to_string(),
            match &items[1] {
                Value::Array(args) => args.clone(),
                other => vec![other.clone()],
            },
        ),
        _ => (String::new(), Vec::new()),
    };
    Packet::Event { name, args, raw: data.to_string() }
}

/// The disconnect reason of a `Disconnect` packet, when it parses as
/// `{"reason":"…"}` (owned: the parsed JSON is local to this function).
#[must_use]
pub fn disconnect_reason(packet: &Packet) -> Option<String> {
    let Packet::Disconnect { payload } = packet else { return None };
    let json: Value = serde_json::from_str(payload).ok()?;
    json.get("reason")?.as_str().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::{Packet, decode, disconnect_reason, encode_event};
    use serde_json::json;

    #[test]
    fn encodes_event_object_payload() {
        let frame =
            encode_event("user.authenticate", &[json!({"headers": {"X-Skypetoken": "<token>"}})])
                .expect("encode");
        assert!(frame.starts_with("5:::"));
        assert!(frame.contains("\"name\":\"user.authenticate\""));
        assert!(frame.contains("\"args\":["));
    }

    #[test]
    fn decodes_server_connect() {
        assert_eq!(decode("1::").expect("decode"), Packet::Connect { qs: String::new() });
    }

    #[test]
    fn decodes_live_disconnect_reasons() {
        for (frame, reason) in [
            (r#"0:::{"reason":"timeout"}"#, "timeout"),
            (r#"0:::{"reason":"badrequest"}"#, "badrequest"),
            (r#"0:::{"reason":"unauthorized"}"#, "unauthorized"),
        ] {
            let p = decode(frame).expect(frame);
            assert_eq!(disconnect_reason(&p), Some(reason.to_string()));
        }
    }

    #[test]
    fn decodes_own_encode() {
        let frame = encode_event("trouter.test", &[json!(1), json!("two")]).expect("encode");
        let Packet::Event { name, args, .. } = decode(&frame).expect("decode") else {
            panic!("expected event");
        };
        assert_eq!(name, "trouter.test");
        assert_eq!(args.len(), 2);
    }

    #[test]
    fn decodes_array_form_leniently() {
        let Packet::Event { name, args, .. } =
            decode(r#"5:::["user.activity",{"state":"active"}]"#).expect("decode")
        else {
            panic!("expected event");
        };
        assert_eq!(name, "user.activity");
        assert_eq!(args.len(), 1);
    }

    #[test]
    fn degraded_when_payload_not_json() {
        let Packet::Event { name, args, raw, .. } = decode("5:::not json").expect("lenient") else {
            panic!("expected event");
        };
        assert_eq!(name, "");
        assert_eq!(args, [] as [serde_json::Value; 0]);
        assert_eq!(raw, "not json");
    }

    #[test]
    fn malformed_frames_error() {
        assert!(decode("").is_err());
        assert!(decode("x:::").is_err());
        assert!(decode("99:::").is_err());
        assert!(decode("35:::").is_err()); // two-digit "type" — not the 0.9 shape
    }
}
