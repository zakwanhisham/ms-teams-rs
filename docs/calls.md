# Calls & meetings protocol research

> **This document is the Phase 2B GO/NO-GO gate.** The rest of the plan (TODO.md Phase 8)
> proceeds only when a **GO verdict** is written at the bottom.
> Never paste real tokens or personal data; sanitize all captures into `docs/traces/`
> (gitignored).

## Objective

Within ~2 weeks: join one real 1:1 Teams call and one scheduled meeting from a headless Rust
binary, with **audio flowing both ways**.

## Ground truth capture

Method: MITM proxy (or browser devtools on `teams.microsoft.com`) while the official client:

- [ ] places a 1:1 video call (invite → ring → accept → hangup)
- [ ] receives a 1:1 call
- [ ] joins a scheduled meeting (incl. lobby)
- [ ] shares screen
- [ ] leaves the meeting

Save sanitized traces to `docs/traces/`. Record payload encodings (JSON / protobuf /
skypemessage) for each message type.

## Signalling (Trouter)

- [ ] Websocket endpoint + handshake/auth: `wss://go.trouter.io/v2/a` (confirm subprotocol)
- [ ] Registrar: `https://prod.registrar.skype.com/v2/registrations` — endpoint registration
      with `transport: TROUTER`, `ttl ≥ 300 s`, refresh ≥ 30 s before expiry
- [ ] Message envelope: framing, correlation ids, MS-CV handling
- [ ] Lifecycle catalogue:

| Stage | Message(s) | Encoding | Notes |
|-------|-----------|----------|-------|
| invite / ring | | | |
| accept | | | |
| reject / decline | | | |
| SDP offer | | | |
| SDP answer | | | |
| ICE candidates | | | |
| connected / media-up | | | |
| mute / camera / presence | | | |
| hangup | | | |
| group participant join/leave | | | |
| error / busy / no-answer | | | |

## Meeting join path

- [ ] Resolve meeting join URL → meeting metadata (organizer, thread/conversation ids)
- [ ] Join request endpoints (middleTier / substrate) and required payload
- [ ] Lobby behaviour: state signalling until admitted; admit/deny as organizer
- [ ] Roster/participant state messages (mute, camera, hand-raise, pin, presenter)
- [ ] Content sharing: who is presenting, content track negotiation
- [ ] Meeting chat correlation (ties into the Phase 5 message stream)

## Media

- [ ] WebRTC stack decision — **`str0m` vs `webrtc-rs` vs libwebrtc FFI** (record the why)
- [ ] SDP quirks: required extensions, bundle policy, mid/msid usage
- [ ] Codecs observed: Opus / OpusRed, H.264 / VP8 / VP9, simulcast layers, NACK/FEC/RTX
- [ ] TURN/STUN servers handed out by signalling; allocation method (TURN REST credentials?)
- [ ] PoC: two of our own processes complete offer/answer + ICE + DTLS-SRTP and exchange audio

## Device capture

- [ ] Audio in/out: `cpal` → device enumeration, hot-plug, levels/echo cancellation path
- [ ] Camera: `v4l2` / PipeWire capture, format negotiation
- [ ] Screen share (Wayland): `ashpd` → `org.freedesktop.portal.ScreenCast` (PipeWire node)
- [ ] Screen share (X11 fallback): XComposite/XShm
- [ ] Verified on GNOME, KDE, Sway — note per-DE caveats

## GUI feasibility (feeds the §0 toolkit decision)

- [ ] Can the chosen toolkit blit a `wgpu` video texture into the widget tree at 30+ fps
      for N remote tiles? Prototype required before Phase 6 completes.

## Open questions

- Is there any audio/video **eligibility** check or tenant policy gate at join time?
- Recording/transcription control — reachable without Graph?
- What happens on network change (ICE restart vs full re-join)?

---

## VERDICT

**Status:** ⬜ pending

**Summary:** _(written at the end of the spike: what works, what doesn't, effort estimate
for Phase 8, and the chosen WebRTC stack)_
