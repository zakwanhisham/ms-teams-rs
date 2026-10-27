//! Call-log event bodies — `Event/Call` messages (`contenttype: application/user+xml`).
//!
//! The chat surface records call activity as messages whose body is a compact XML
//! dialect (captured and documented in `docs/endpoints.md`; sanitized fixture
//! `tests/fixtures/event-call.json`): a leading state element (`<ended/>` — absent on
//! call-started bodies), a `<partlist>` of attendees, and a doubled
//! `<meetingDetails>` tree carrying the organizer, meeting times, calendar ids, an
//! `<instanceDetails>` occurrence block, `<callEventType>`, `conversationUrl` and
//! `callId`. The `<ended/>` marker is redundant with `<callEventType>callEnded` and
//! therefore not modelled.
//!
//! The dialect is fixed, so this module hand-scans it leniently (no XML dependency):
//! unknown shapes degrade to `None`/empty lists and never fail the caller.

use serde::Serialize;

/// One `<part>` inside a call log's `<partlist>`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct CallParticipant {
    /// `identity` attribute (MRI-ish: `8:orgid:<guid>`, also `28:<guid>` seen live).
    pub identity: Option<String>,
    /// `<name>` leaf (same MRI family; the durable caller id).
    pub name: Option<String>,
    /// `<displayName>` leaf.
    pub display_name: Option<String>,
    /// `<duration>` leaf in seconds (seconds on the wire for ended calls).
    pub duration_secs: Option<u64>,
}

/// `<instanceDetails>` — the meeting occurrence the call belongs to.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct CallOccurrence {
    /// `<type>` leaf (observed `Occurrence` — an enum, not a timestamp).
    pub kind: Option<String>,
    /// `<subject>` leaf.
    pub subject: Option<String>,
    /// `<startTime>` leaf, service-formatted.
    pub start_time: Option<String>,
    /// `<endTime>` leaf, service-formatted.
    pub end_time: Option<String>,
    /// `<iCalUid>` leaf of the occurrence.
    pub i_cal_uid: Option<String>,
}

/// A typed call-log body (the only `Event/*` variant captured so far).
///
/// The leading `<ended/>` element seen on ended-call bodies is NOT modelled — it
/// duplicates `event_type = callEnded`, which is present in all 37 captured bodies;
/// see docs/endpoints.md .
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct EventMessage {
    /// `<callEventType>` leaf (`callStarted` / `callEnded` observed).
    pub event_type: Option<String>,
    /// `<partlist>` attendees, in wire order (only ended calls carried them live).
    pub participants: Vec<CallParticipant>,
    /// `<organizerUpn>` leaf.
    pub organizer_upn: Option<String>,
    /// `<instanceDetails>` block, when the call belongs to a meeting.
    pub occurrence: Option<CallOccurrence>,
}

impl EventMessage {
    /// Leniently parse the XML body of an `Event/Call` message.
    ///
    /// Returns `None` when the body carries nothing this dialect recognizes (plain
    /// text or foreign shapes), so callers can fall back to `content` as-is.
    #[must_use]
    pub fn parse(content: &str) -> Option<Self> {
        let trimmed = content.trim_start();
        if !trimmed.starts_with('<') {
            return None;
        }

        let event_type = leaf(trimmed, "callEventType").map(str::to_owned);

        let mut participants = Vec::new();
        if let Some(start) = trimmed.find("<part ") {
            parse_parts(trimmed, start, &mut participants);
        }

        let organizer_upn = leaf(trimmed, "organizerUpn").map(str::to_owned);
        let occurrence = leaf(trimmed, "instanceDetails").map(|body| CallOccurrence {
            kind: leaf(body, "type").map(str::to_owned),
            subject: leaf(body, "subject").map(str::to_owned),
            start_time: leaf(body, "startTime").map(str::to_owned),
            end_time: leaf(body, "endTime").map(str::to_owned),
            i_cal_uid: leaf(body, "iCalUid").map(str::to_owned),
        });

        // Nothing recognizable → not an event body.
        if event_type.is_none()
            && participants.is_empty()
            && organizer_upn.is_none()
            && occurrence.is_none()
        {
            return None;
        }

        Some(Self { event_type, participants, organizer_upn, occurrence })
    }

    /// `true` when the log records a call that ended (`callEventType: callEnded`).
    #[must_use]
    pub fn is_ended(&self) -> bool {
        self.event_type.as_deref() == Some("callEnded")
    }
}

/// Parse one `<part …>…</part>` segment into a [`CallParticipant`]; lenient.
fn parse_part(raw: &str) -> Option<CallParticipant> {
    let name = leaf(raw, "name").map(str::to_owned);
    let display_name = leaf(raw, "displayName").map(str::to_owned);
    let duration_secs = leaf(raw, "duration").and_then(|value| value.parse::<u64>().ok());
    let identity = attribute(raw, "identity");
    if name.is_none() && display_name.is_none() && duration_secs.is_none() && identity.is_none() {
        return None;
    }
    Some(CallParticipant { identity, name, display_name, duration_secs })
}

/// Consume every `<part …>` segment from `start` on into `out`.
fn parse_parts(raw: &str, start: usize, out: &mut Vec<CallParticipant>) {
    let mut index = start;
    while let Some(open) = raw[index..].find("<part ") {
        let tag_open = index + open;
        // The dialect emits short parts; a missing close tag just means malformed
        // data that cannot be represented — skip to the next candidate instead of
        // failing the whole body.
        let Some(close) = raw[tag_open..].find("</part>") else { break };
        let segment = &raw[tag_open..tag_open + close];
        if let Some(participant) = parse_part(segment) {
            out.push(participant);
        }
        index = tag_open + close;
    }
}

/// Text between `<tag …>…</tag>` (attribute variants tolerated).
fn leaf<'a>(raw: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}");
    let start_tag = raw.find(&open)?;
    let tag_end = raw[start_tag..].find('>')? + start_tag;
    let close = format!("</{tag}>");
    let close_at = raw[tag_end + 1..].find(&close)? + tag_end + 1;
    Some(&raw[tag_end + 1..close_at])
}

/// Attribute value from the tag at `raw`'s start: `identity="…"` in `<part …>`.
fn attribute(raw: &str, name: &str) -> Option<String> {
    let tag_start = raw.find('<')?;
    let tag_end = raw[tag_start..].find('>')? + tag_start;
    let tag = &raw[tag_start..=tag_end];
    let needle = format!("{name}=");
    let at = tag.find(&needle)? + needle.len();
    let after = tag[at..].strip_prefix(['"', '\''])?;
    let end = after.find(['"', '\''])?;
    Some(after[..end].to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wire-faithful shape (attrs, doubled `meetingDetails`, occurrence block) with
    /// synthetic values only.
    const ENDED: &str = "<ended/><partlist alt=\"\" count=\"2\">\
<part identity=\"8:orgid:00000000-0000-0000-0000-000000000000\"><name>8:orgid:00000000-0000-0000-0000-000000000000</name><displayName>Rex</displayName><duration>868</duration></part>\
<part identity=\"28:00000000-0000-0000-0000-000000000000\"><name>28:00000000-0000-0000-0000-000000000000</name><displayName></displayName><duration>12</duration></part>\
</partlist><meetingDetails><meetingDetails><organizerUpn>rex@example.invalid</organizerUpn>\
<meetingType>Recurring</meetingType><startTime>10/27/2026 00:52:56</startTime><endTime>10/27/2026 01:52:56</endTime>\
<exchangeId>AAAA</exchangeId><iCalUid>AAAA-BBBB</iCalUid><hasEphemeralTranscript>false</hasEphemeralTranscript>\
<isRecapEnabled>true</isRecapEnabled><instanceDetails><iCalUid>AAAA-BBBB</iCalUid>\
<startTime>10/27/2026 01:00:00</startTime><endTime>10/27/2026 02:00:00</endTime>\
<subject>sync</subject><type>Occurrence</type></instanceDetails></meetingDetails>\
<callEventType>callEnded</callEventType><conversationStartTime>10/27/2026 00:52:56</conversationStartTime>\
<conversationUrl>https://api.invalid/conv</conversationUrl><callId>00000000-0000-0000-0000-000000000000</callId>\
<wasGcpBotInvited>false</wasGcpBotInvited></meetingDetails>";

    const STARTED: &str = "<partlist alt =\"\"></partlist><meetingDetails><meetingDetails>\
<organizerUpn>rex@example.invalid</organizerUpn><meetingType>Recurring</meetingType>\
<startTime>10/27/2026 00:00:00</startTime><endTime>10/27/2026 01:00:00</endTime>\
<iCalUid>AAAA-BBBB</iCalUid></meetingDetails><callEventType>callStarted</callEventType></meetingDetails>";

    #[test]
    fn ended_calls_parse_with_parts_and_occurrence() {
        let event = EventMessage::parse(ENDED).expect("ended body parses");
        assert!(event.is_ended());
        assert_eq!(event.event_type.as_deref(), Some("callEnded"));
        assert_eq!(event.participants.len(), 2);
        assert_eq!(
            event.participants[0],
            CallParticipant {
                identity: Some("8:orgid:00000000-0000-0000-0000-000000000000".into()),
                name: Some("8:orgid:00000000-0000-0000-0000-000000000000".into()),
                display_name: Some("Rex".into()),
                duration_secs: Some(868),
            }
        );
        assert_eq!(event.participants[1].duration_secs, Some(12));
        assert_eq!(
            event.participants[1].identity.as_deref(),
            Some("28:00000000-0000-0000-0000-000000000000")
        );
        assert_eq!(event.organizer_upn.as_deref(), Some("rex@example.invalid"));
        let occurrence = event.occurrence.expect("instanceDetails present");
        assert_eq!(occurrence.kind.as_deref(), Some("Occurrence"));
        assert_eq!(occurrence.subject.as_deref(), Some("sync"));
        assert_eq!(occurrence.start_time.as_deref(), Some("10/27/2026 01:00:00"));
    }

    #[test]
    fn started_calls_have_no_marker_and_no_parts() {
        let event = EventMessage::parse(STARTED).expect("started body parses");
        assert!(!event.is_ended(), "call-started logs are not ended");
        assert_eq!(event.event_type.as_deref(), Some("callStarted"));
        assert_eq!(event.participants, [] as [CallParticipant; 0]);
    }

    #[test]
    fn foreign_and_empty_bodies_degrade() {
        assert!(EventMessage::parse("plain text reply").is_none());
        assert!(EventMessage::parse("").is_none());
        assert!(
            EventMessage::parse("<unknown>body</unknown>").is_none(),
            "no recognized leaves → None"
        );
    }

    #[test]
    fn malformed_part_segments_do_not_fail_the_body() {
        // Truncated attendees degrade to the ones that did parse.
        let event = EventMessage::parse(
            "<partlist><part identity=\"8:orgid:1\"><name>8:orgid:1</name></part>",
        )
        .expect("parseable");
        assert_eq!(event.participants.len(), 1);
        assert_eq!(event.participants[0].identity.as_deref(), Some("8:orgid:1"));
    }
}
