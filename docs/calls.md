# Calls & meetings protocol research

> **This document is the Phase 2B GO/NO-GO gate.** The rest of the plan (TODO.md Phase 8)
> proceeds only when a **GO verdict** is written at the bottom.
> Never paste real tokens or personal data; sanitize all captures into `docs/traces/`
> (gitignored).

## Objective

Within ~2 weeks: join one real 1:1 Teams call and one scheduled meeting from a headless Rust
binary, with **audio flowing both ways**.

## Prior art — how teams-for-linux does calls

[`IsmaelMartinez/teams-for-linux`](https://github.com/IsmaelMartinez/teams-for-linux) (Electron,
GPL-3.0) is the project everyone points at for "calls on Linux". It **does not implement the call
protocol at all**: it loads Microsoft's own Teams web client in a `BrowserWindow` (`contextIsolation`
off + an injected `app/browser/preload.js`), so Trouter signalling, SDP/ICE and media are Microsoft's
JavaScript running on Chromium's WebRTC — the same code path as the browser/PWA. Their ADR-032 says
it outright: *"the call lives in the renderer"* and cannot be moved between windows without tearing
it down. Everything they add is environment glue:

- **User agent** — ships a Chrome UA with the Electron token stripped, because "Microsoft sign-in
  and calls misbehave when the Electron token is present" (`docs-site/docs/troubleshooting.md`).
- **Screen share** — `app/screenSharing/`: in-app picker + per-session
  `setDisplayMediaRequestHandler`, injected `getUserMedia` constraint overrides (resolution
  remove/override), preview window. Wayland needs the xdg-desktop-portal/PipeWire; XWayland
  whole-screen share is black (#3066); Electron 41's GPU regression breaks incoming share (#2459).
- **Call-state detection** — `speakingIndicator` reads WebRTC stats + the `#inCall` DOM node →
  `call-connected`/`call-disconnected` IPC → wake lock, `powerSaveBlocker`, MQTT in-call status,
  and deferred deep links (PR #2988: never reload mid-call).
- **Incoming calls** — preload watches Teams' own UI/`Notification` → native toast,
  `incomingCallCommand`, MQTT `incoming-call` topic. No signalling of its own.
- **Quirk fixes** — per-`webContents` `setWebRTCIPHandlingPolicy` (multi-NIC ICE drops calls to
  OnHold, #2349); `performance.disableGpu` auto-enabled on Wayland (call lag, #2410); mic/camera
  constraint shims (`overrideMicConstraints`, `disableAutogain`, camera geometry).

What this means for the spike:

- It **validates our environment risk list** (Wayland/XWayland capture, GPU path, multi-NIC ICE,
  UA sensitivity) and gives the NO-GO fallback a real-world data point: embedding/deferring to the
  web client demonstrably works for calls.
- The Teams web client has a desktop-host windowing layer (`CallingMultiWindowService`,
  `MultiWindowExperience` vs `SingleWindowExperience`, `isDesktopApp`) only the official desktop app
  can drive; a browser — and therefore our headless client — runs the single-window experience.
- It is **not a template for a native implementation**: as far as we know, no open-source project
  has a from-scratch Teams call stack. The protocol work below is genuinely novel territory.

## Plan to close this spike

> Ownership: 🧑 = needs the account owner, 🤖 = agent. Stages 0 and 2 are deliberately
> capture-independent so they run **in parallel** with the Phase 3/4 close-out; only stage 1
> needs the owner's calendar, and stage 3 starts the moment traces land.

| Stage | What | Owner | Depends on |
|------:|------|:-----:|------------|
| 0 | Capture-pipeline readiness + live Trouter/registrar probes | 🤖 | — |
| 1 | Capture session (5 scenarios + bonus authz body) | 🧑 | stage 0 |
| 2 | Local proofs: WebRTC stack decision, two-process audio, devices | 🤖 | — (parallel) |
| 3 | Trace analysis → fill the catalogues below, risk call on encoding | 🤖 | stage 1 |
| 4 | Integration: Trouter client + signalling + media → the exit demo | 🤖 | stages 2+3 |
| 5 | Verdict + timeline re-evaluation | 🤖 | stage 4 |

### Stage 0 — pipeline readiness (🤖, before spending any of the owner's time)

- [ ] **Add websocket recording to `tools/capture/teams_trace.py`** — the addon currently
      hooks HTTP responses only, so a session today would capture the Trouter *handshake*
      but none of the invite/SDP/ICE frames that are the point of this spike. Add
      `websocket_start`/`websocket_message`/`websocket_end`; record frames as base64 when
      binary (decode locally, never commit) and apply the same JWT/credential scrubbing.
- [ ] **Smoke-test the whole pipeline headless** (no user needed — we hold live
      credentials): `mitmdump` + `https_proxy=… teams-cli conversations` → JSONL lands in
      `docs/traces/`, redaction greps in `tools/capture/README.md` all pass.
- [ ] **Probe Trouter auth with our own tokens** (live, same style as the substrate probe):
      the unauthenticated upgrade answers 401 (observed) — try `skypetoken` header/query
      variants and a spaces bearer against `wss://go.trouter.io/v2/a`, and register against
      `prod.registrar.skype.com/v2/registrations`. If this cracks the auth question, one
      item is answered *before* any capture.
- [ ] Update `tools/capture/README.md` with the stage-1 runbook outcome (which client went
      through the proxy cleanly, CA caveats on this distro).

### Stage 1 — capture session (🧑, ~half day; runbook: `tools/capture/README.md`)

Preconditions (in order): check Conditional Access tolerates a proxied login → install the
mitmproxy CA cert → smoke-test with a browser on `teams.microsoft.com`. If the desktop
client ignores the proxy or pins certificates, capture through the **browser PWA** instead;
HAR export via devtools is the last-resort fallback (HTTP only — WS frames must come from
the proxy, this is why stage 0 matters).

Scenarios (1–3 min each; one trace file is fine):

1. [ ] place a 1:1 video call (invite → ring → accept → hangup) — 🧑
2. [ ] receive a 1:1 call — 🧑 (phone Teams app as caller; the far side needn't be proxied —
       one side's capture contains both SDP offer and answer, both ICE directions)
3. [ ] join a scheduled meeting incl. lobby wait + admission — 🧑 (join as *attendee* with
       lobby on; a second participant admits; the roster already lists upcoming meetings)
4. [ ] share screen (start → a few seconds → stop) — 🧑
5. [ ] leave the meeting — 🧑
6. [ ] **bonus (closes a Phase 2A gap):** proxied `teams-cli auth login` → raw authz body
       (`expiresIn` + full `regionGtms` key list still uncaptured) — 🤖 runs it, 🧑 just
       approves the device code.

Deliverable: `docs/traces/teams-<stamp>.jsonl` (gitignored) + the redaction greps run
before anything is quoted anywhere.

### Stage 2 — local proofs (🤖, parallel — no Teams dependency)

- [ ] **WebRTC stack decision memo** (`str0m` vs `webrtc-rs` vs libwebrtc FFI) recorded in
      §Media with the why: cargo-deny license fit, the workspace's `forbid(unsafe_code)`
      bias (FFI needs a documented exception), video/simulcast support, maintenance, how
      much raw SDP control we need for Teams' quirks.
- [ ] **Two-process audio PoC**: our own processes complete offer/answer + ICE +
      DTLS-SRTP and move audio (tone first, `cpal` loopback second). Proves the stack can
      move audio *at all* before Teams enters the picture.
- [ ] **Device probes**: `cpal` device enumeration (PipeWire 1.6.9 present, Pulse shim
      ✓); `ashpd` → `org.freedesktop.portal.ScreenCast` on this Sway/Wayland box (X11
      fallback stays a checkbox for other machines). *This machine has no camera*
      (`/dev/video*` absent) — fine: the 2B exit criterion is audio; video capture is a
      Phase 8 caveat to note in §Device capture.

### Stage 3 — trace analysis (🤖, needs stage 1)

- [ ] **Frame format first** — text JSON vs binary (protobuf / skypemessage): this is the
      GO/NO-GO risk item and gets looked at before anything else in the trace.
- [ ] Fill the §Signalling lifecycle table (invite/ring/accept/SDP/ICE/hangup + group and
      meeting variants), envelope/correlation/MS-CV notes.
- [ ] Registrar payload + TTL/refresh behaviour.
- [ ] §Meeting join path: join URL → metadata → middleTier/substrate endpoints, lobby
      state messages, roster/content-track messages.
- [ ] Transcribe **structure only** (field names, types, status codes) into this document;
      HTTP-shaped payloads additionally get a sanitized fixture + test per `AGENTS.md`.

**Timebox decision point (the actual GO/NO-GO):** if the payloads are protobuf/skypemessage
with no discoverable schema, spend a fixed, short timebox trying to decode one complete
message path (Skype-heritage signalling schemas have public history to mine). Blow the
timebox → write a **NO-GO** recommendation and re-scope to browser-fallback calls per
`TODO.md`.

### Stage 4 — integration to exit criteria (🤖)

- [ ] `teams-realtime`: Trouter websocket client (auth from stages 0/3), registrar
      registration/refresh, envelope encode/decode.
- [ ] `teams-call`: lifecycle state machine (`invite → … → hangup`), meeting join +
      lobby handling.
- [ ] Media: signalling-provided TURN/STUN wired into the chosen stack; devices attached.
- [ ] **Exit demo**: headless binary places (or joins) a 1:1 call, audio both ways with
      the official client (phone) on the other end — and the same against a scheduled
      meeting.
- [ ] Opportunistically answer the §Open questions (join-time eligibility, ICE restart).

### Stage 5 — verdict & re-evaluation

- [ ] All checklists below filled; **VERDICT** written: GO / CONDITIONAL GO / NO-GO, the
      chosen stack with its why, and a rough Phase 8 effort estimate.
- [ ] Tick Phase 2B in `TODO.md`, re-evaluate the whole timeline (as the checkpoint
      demands), ungate Phase 8 — or execute the browser-fallback re-scope.

**GO** = the exit criteria literally met (headless 1:1, two-way audio, this document
complete). **CONDITIONAL GO** = audio works but a named area (lobby, content share, group
variants) is still unproven → Phase 8 opens with those as explicit spike items.
**NO-GO** = stage 3's timebox blown, or media needs a non-Rust/proprietary stack, or org
policy blocks the path.

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

- [ ] Websocket endpoint + handshake/auth: `wss://go.trouter.io/v2/a`
      — 🔶 endpoint confirmed live: unauthenticated websocket upgrade → **HTTP 401**, empty
      body (2026-10-27). Auth mechanism (bearer? cookie? skypetoken?) TBD from capture.
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
