# TODO — `ms-teams-rs`: a native Microsoft Teams client for Linux in Rust

> **Status legend:** `[ ]` not started · `[~]` in progress · `[x]` done · `[!]` blocked / needs a decision
> Microsoft discontinued the native Linux Teams client in 2022. Everything below targets the
> *unofficial* Teams HTTP APIs used by the web client (chat/IC3, authsvc, CSA, Graph, Trouter/WebRTC).
> This project is not affiliated with Microsoft — see **Risks** at the bottom.

---

## 0. Goals & non-goals

### Goals (v1 — "full-featured chat + calls client")
- Sign in with a Microsoft work/school or personal account and stay signed in across restarts.
- List teams, channels, 1:1 and group chats; read history and receive new messages in real time.
- Send, edit, delete, react to, and reply to messages; render mentions, code blocks, inline images.
- Desktop notifications (via `org.freedesktop.Notifications`), unread badges, tray icon.
- **Calls (full feature set):**
  - 1:1 and group audio/video calls, incoming-call popup with answer/decline.
  - Join scheduled meetings from the app: lobby, participant roster, in-meeting chat.
  - Screen/window sharing (PipeWire portal on Wayland, X11 fallback) and camera capture.
  - Mute/unmute, camera on/off, device selection (camera/mic/speaker), volume control.
  - In-call controls: hold, background blur/replace (stretch), reactions/raise-hand, pins,
    recording/ transcription indicator (watch-only unless reverse-engineered), leave/retry.
  - Presence status (Available/Away/Busy/Do-not-disturb) synced with calls.
- Runs natively on Linux (Wayland + X11), pure Rust, no Electron/WebView.

### Non-goals (v1)
- Windows/macOS support (keep it Linux-first, but don't gratuitously break portability).
- Live events / webinars / town halls (large, separate protocol — defer past v1).
- PSTN dial-out (defer; requires telecom-specific endpoints).

### Key decisions to lock in early `[!]`
- [ ] **GUI toolkit:** recommend **`iced` 0.14** (Elm/MVU, wgpu, retained mode — best fit for a
      long-lived, list-heavy chat UI). Alternatives: `egui` (fast prototype, "debug UI" look),
      `gtk-rs`/GTK4 (most GNOME-native, but ties you to GTK), `slint` (license caveat).
      **Call UI adds a hard requirement: GPU video compositing of remote streams.** Verify iced
      can blit `wgpu` textures into the widget tree cheaply before committing.
- [ ] **WebRTC stack:** `str0m` (pure Rust, no OpenSSL) vs `webrtc-rs` vs FFI to libwebrtc.
      Decide in the Phase 2 spike — this is the single highest-risk dependency choice.
- [ ] **Async runtime:** `tokio` (non-negotiable given reqwest + long-polling + WebRTC).
- [ ] **Local storage:** `rusqlite` (bundled SQLite) for message cache + `keyring` crate for tokens.
- [ ] **Minimum supported distros / MSRV** and packaging targets (`.deb`/`.rpm`/Flatpak/AppImage).

---

## Phase 1 — Project skeleton & tooling ✅ *(completed 2026-10-27)*

- [x] `cargo new` as a **workspace** with these crates:
      ```
      ms-teams-rs/
      ├── crates/
      │   ├── teams-auth/       # OAuth device-code, FRT cache, token refresh, authz exchange
      │   ├── teams-api/        # typed HTTP clients: chat (IC3), CSA, substrate, Graph, ASM
      │   ├── teams-protocol/   # wire types: Conversation, Message, Event, HTML content model
      │   ├── teams-realtime/   # endpoint registration, subscriptions, long-poll / Trouter
      │   ├── teams-call/       # Trouter signalling, SDP/ICE, media engine, devices
      │   ├── teams-store/      # SQLite cache, migrations, outbox for pending sends
      │   └── teams-notify/     # XDG desktop notifications, tray, MPRIS, deep links
      ├── apps/teams-gui/       # the iced application (toolkit decision pending → no deps yet)
      ├── tools/teams-cli/      # headless CLI used for protocol exploration & integration tests
      └── docs/                 # architecture / auth / endpoints / calls research
      ```
- [x] Tooling: `rustfmt` (`rustfmt.toml`) + `clippy -D warnings` with pedantic lints via
      `[workspace.lints]`, `cargo-deny` (`deny.toml`: advisories/licenses/sources/bans),
      `cargo-nextest` (CI), `cargo-insta` deferred until Phase 4 fixtures exist.
- [x] CI: GitHub Actions (`.github/workflows/ci.yml`) — fmt, clippy, nextest + doc-tests,
      cargo-deny, rustdoc with `-D warnings`, rust-cache.
- [x] `README.md` (incl. status table), `LICENSE-MIT` + `LICENSE-APACHE`, `CONTRIBUTING.md`,
      `.gitignore`, `.editorconfig`.
- [x] `docs/` scaffold: `docs/architecture.md` (layering, concurrency, data flows),
      `docs/auth.md` (auth checklist), `docs/endpoints.md` (endpoint tables),
      `docs/calls.md` (Phase 2B go/no-go template).

**Exit criteria:** ✅ `cargo test --workspace` green (16 suites), `cargo clippy` /
`cargo fmt --check` / `cargo doc -D warnings` all clean, `teams-cli --help` works.

**Follow-ups carried forward:**
- [ ] Install `cargo-nextest` / `cargo-deny` locally (CI already installs them).
- [ ] Push to a git remote to enable the CI workflow.
- [ ] Decide the §0 GUI toolkit (blocks Phase 6, not Phase 2).

---

## Phase 2 — Research spike (protocol reverse engineering)

*Do this before writing much production code. Output goes into `docs/` + fixture files.
**Two spikes run in parallel — Chat (2A) and Calls (2B). 2B is the go/no-go for the project.** *

### Phase 2A — Chat/auth protocol

- [ ] **Auth path**
      - [ ] Device-code flow against the Teams **FOCI public client**
            `1fec8e78-bce4-4aaf-ab1b-5451cc387264` at
            `https://login.microsoftonline.com/{tenant|common}/oauth2/v2.0/`:
            `devicecode` → poll `token`, obtain **FRT** (family refresh token).
      - [ ] Redeem FRT per-audience: `https://api.spaces.skype.com`,
            `https://chatsvcagg.teams.microsoft.com`, `substrate.office.com`,
            `graph.microsoft.com`, **`go.trouter.io`** (needed for calls).
      - [ ] **authz exchange:** `POST https://authsvc.teams.microsoft.com/v1.0/authz`
            with `Authorization: Bearer …` → `{ tokens.skypeToken, region, regionGtms{…} }`.
            Record the *shape* of `regionGtms` (never hardcode a region!).
      - [ ] Verify token refresh semantics: expiry claims, rotation of the FRT, what a 401 means.
      - [ ] MFA / Conditional Access / personal-vs-work account behaviour — document failures.
- [ ] **Chat service (IC3 / MSNP24)** — base `{chatService}` from `regionGtms`, header
      **`Authentication: skypetoken=<jwt>`** (not `Authorization`):
      - [ ] `GET /v1/users/ME/conversations?view=msnp24Equivalent&pageSize=N&startTime=1`
      - [ ] `GET /v1/users/ME/conversations/{id}/messages?pageSize=N&startTime=1`
      - [ ] `POST /v1/users/ME/conversations/{id}/messages` (+ `;messageid={root}` for replies)
      - [ ] `PUT …/messages/{id}` (edit, `skypeeditedid`), `DELETE`, reactions via
            `PUT …/messages/{id}/properties?name=emotions`
      - [ ] Conversation/channel id percent-encoding (`:` `@` `;` in ids).
      - [ ] Message HTML wire format: mentions (`properties.mentions`), code blocks,
            quotes, attachments, @everyone.
- [ ] **Realtime events** — pick one and prove it with a packet capture:
      - [ ] *(A)* **HTTP long-poll**: register endpoint → `POST /v1/users/ME/endpoints/{id}/active`
            (keepalive ~45 s, `timeout: 12`) → subscribe `interestedResources`
            (`/v1/threads/ALL`, `/v1/users/ME/conversations/ALL/messages`, …,
            `channelType: httpLongPoll`) → `POST …/subscriptions/{n}/poll` (blocks ~30 s).
      - [ ] *(B)* **Trouter** (`https://go.trouter.io/v2/a`) websocket — required for call
            signalling anyway; likely becomes the primary transport once Phase 5 lands.
      - [ ] Recommendation: **(B) is mandatory for calls**, so implement it first and keep the
            long-poll as a fallback transport behind a trait.
- [ ] **Team/channel roster**: `GET https://teams.microsoft.com/api/csa/{region}/api/v1/teams/users/me/updates`
      (audience `chatsvcagg.teams.microsoft.com`) — note `Origin`/`Referer` requirements.
- [ ] **People/search**: `POST https://substrate.office.com/search/api/v1/suggestions`
      (needs `Origin`/`Referer: https://teams.microsoft.com` else 400).
- [ ] Record **fixtures**: sanitized JSON samples for every endpoint into
      `crates/*/tests/fixtures/`. These become your regression tests.

### Phase 2B — Call protocol spike *(GO/NO-GO CHECKPOINT)*

**Goal: within ~2 weeks, join one real 1:1 Teams call and one scheduled meeting from a
headless Rust binary, with audio flowing both ways.** If this fails, the plan reverts to
browser-fallback calls and Phases 5/8 are re-scoped.

- [ ] **Capture ground truth:** run the official Teams web/desktop client through a MITM proxy
      (or browser devtools on `teams.microsoft.com`) during: 1:1 call, meeting join, screen
      share. Save sanitized traces to `docs/traces/` — you cannot guess these payloads.
- [ ] **Trouter client:** connect to `https://go.trouter.io/v2/a` (websocket), authenticate,
      understand the message envelope (message types, correlation ids, MS-CV headers).
      Also probe the related registrar: `https://prod.registrar.skype.com/v2/registrations`
      (endpoint registration with `transport: TROUTER`, TTL ≥ 300 s, refresh before expiry).
- [ ] **Call lifecycle signalling:** map the message sequence for
      `invite → ring → accept → SDP offer/answer → ICE candidates → connected → hangup`,
      including group-call and meeting-join variants. Identify the payload formats
      (JSON? protobuf? skypemessage?) and the media/turn server URLs handed out.
- [ ] **Meeting join path:** resolve a meeting join URL → meeting metadata (organizer, thread id,
      focus/conversation ids) → join request through the `middleTier`/substrate meeting endpoints.
      Document lobby/admission behaviour (you start in lobby until admitted).
- [ ] **Media proof-of-concept:**
      - [ ] Pick the WebRTC stack (`str0m` vs `webrtc-rs` vs libwebrtc FFI) and record why.
      - [ ] Complete an SDP offer/answer + ICE against the captured signalling, get DTLS-SRTP
            media flowing between two of your own processes.
      - [ ] Verify Teams' media requirements: Opus/OpusRed, H.264/VP8/VP9 video, NACK/FEC/RTX,
            and any non-standard SDP extensions the server expects.
- [ ] **Device capture proof-of-concept:**
      - [ ] Camera + mic: `cpal` (audio) + PipeWire/`pipewire` bindings or `v4l2` (video).
      - [ ] Screen share: `ashpd` → `org.freedesktop.portal.ScreenCast` (Wayland-native),
            X11 fallback via XComposite/XShm. Confirm portal works under GNOME/KDE/Sway.
- [ ] **Write `docs/calls.md`** with: full message catalogue, sequence diagrams, media
      requirements, and a verdict with a rough effort estimate for Phases 5/8.

**Exit criteria (2A):** `docs/endpoints.md` documents every chat endpoint with an authenticated
reproduction. **Exit criteria (2B):** a headless Rust binary completes a 1:1 call with two-way
audio, and `docs/calls.md` exists with a **GO** verdict. Re-evaluate the whole timeline here.

---

## Phase 3 — `teams-auth`

- [ ] `DeviceCodeFlow`: print the code + verification URL to the terminal, poll with backoff.
- [ ] `TokenCache`: FRT + per-audience access tokens + derived skype session
      (`skypetoken`, `region`, `regionGtms`, expiry), stored **`0600`** under
      `$XDG_STATE_HOME/ms-teams-rs/token-cache.json`, with OS keyring as an alternative backend
      behind a trait (`CredentialStore`).
- [ ] `AuthzClient`: POST authz, parse tokens + region map, surface `AuthError::{Unauthorized, Network, Parse}`.
- [ ] Auto-refresh: refresh access token N seconds before expiry; on chat 401 → evict session,
      re-mint, retry **once**.
- [ ] `teams-cli auth login | logout | status | whoami` (decodes JWT claims for display only,
      no signature verification).
- [ ] Unit tests with mocked HTTP (`wiremock` / `httpmock`).

**Exit criteria:** `teams-cli auth login && teams-cli auth status` works; tokens survive a restart.

---

## Phase 4 — `teams-protocol` + `teams-api` (read path)

- [ ] Strongly-typed models: `Conversation`, `Channel`, `Message`, `User`, `Mention`,
      `Reaction`, `MessageKind` (text/code/image/system), `EventMessage`.
- [ ] Serde deserializers tolerant of unknown fields (`#[serde(default)]`, ignore-unknown) —
      the API *will* change under you.
- [ ] `ChatClient` with methods:
      - [ ] `list_conversations(paging)` — merge chats + channels
      - [ ] `list_messages(conv, before, page_size)`
      - [ ] `get_message(conv, id)` / `get_thread(conv, root_id)` (via `;messageid=`)
      - [ ] `roster()` (CSA), `search_people(q)` (substrate)
- [ ] Central `ApiClient` handling: base URL from `regionGtms`, correct auth header per service,
      MS-CV correlation vectors, User-Agent, retries with jittered exponential backoff,
      429/`Retry-After`, structured `ApiError::{Http{status, body}, Auth, Decode, Network}`.
- [ ] HTML content model: parse Teams wire HTML → internal `RichText` AST (so the GUI can render
      it natively rather than embedding a webview). Crates: `scraper` or `html5ever` + custom walker.
      - [ ] mentions, bold/italic/underline/strike, links, code blocks, blockquote, lists,
            inline images, `@everyone`.
- [ ] Snapshot tests from Phase 2 fixtures.

**Exit criteria:** `teams-cli conversations` and `teams-cli history <conv>` print real data.

---

## Phase 5 — `teams-realtime` + write path

- [ ] **Trouter transport (primary)**: websocket client, auth handshake, message envelope
      codec, auto-reconnect with backoff, MS-CV headers. Shared with `teams-call` signalling.
- [ ] **HTTP long-poll transport (fallback)**: endpoint registration + `/active` keepalive
      (45 s cadence, jitter), subscription manager, resume-from-cursor (persist watermark).
- [ ] Transport abstraction: `RealtimeTransport` trait; pick Trouter when available, fall back
      to long-poll; connection state surfaced to the UI.
- [ ] Event decoding → typed `TeamEvent` (`MessageCreated`, `MessageEdited`, `MessageDeleted`,
      `ReactionChanged`, `ConversationUpdated`, `Typing`, `Presence`, `EndpointExpired`,
      **`IncomingCall`**).
- [ ] **Write operations:**
      - [ ] send text message (capture server id from the `Location` response header, falling
            back to `OriginalArrivalTime`) — never trust the echoed `clientmessageid`
      - [ ] reply-to-thread (`{conv};messageid={root}`), edit (remember: `properties` is
            *replaced*, not merged — re-send `mentions` on edit), delete, react, mark-as-read
      - [ ] typing indicator, presence (`presenceDocs/messagingService`)
- [ ] `teams-store`: SQLite schema + migrations
      (`conversations`, `messages`, `users`, `reactions`, `sync_cursor`, `pending_outbox`).
- [ ] **Outbox**: optimistic UI inserts + idempotent retry of unsent messages on reconnect.
- [ ] Integration tests gated behind `TEAMS_TEST_TOKENS` env variable (ignored by default).

**Exit criteria:** `teams-cli watch` streams live messages; `teams-cli send <conv> "hi"` works
and the message appears in the official client.

---

## Phase 6 — `teams-gui`: chat client

- [ ] App shell: three-pane layout (sidebar of conversations · message list · optional details pane).
- [ ] Message list: virtualized scrolling, infinite load-older, jump-to-first-unread,
      date separators, "new messages" divider.
- [ ] Message rendering from the `RichText` AST (no webview): mentions highlighted, code blocks
      monospaced/copyable, inline images loaded via `teams-api` with a disk cache, emoji.
- [ ] Composer: multiline input, `Ctrl+Enter` send, @-mention autocomplete (substrate search),
      reply preview bar, edit-in-place, draft persistence.
- [ ] Unread state, badges, and sorting of the conversation list; search/filter.
- [ ] Connection status indicator + reconnection banner; auth-expired → re-login prompt.
- [ ] Theming: light/dark following `prefers-color-scheme`, system accent, custom themes later.
- [ ] Settings screen (`$XDG_CONFIG_HOME/ms-teams-rs/config.toml`): notifications on/off,
      sound, startup behaviour, account switcher (multiple accounts), device defaults.
- [ ] a11y pass: AccessKit is wired by iced on winit — verify keyboard nav, screen-reader labels.
- [ ] Performance: message list must stay 60 fps with 10k+ messages; avoid cloning large blobs
      per frame (immediate-mode-friendly snapshot of view data).

**Exit criteria:** daily-driver for chat on a work account for a full week.

---

## Phase 7 — Desktop integration (`teams-notify`)

- [ ] Notifications via `zbus` → `org.freedesktop.Notifications` (works on GNOME/KDE/Sway),
      with `x-kde-urgency`, actions (*Reply*/*Open*), and a `do-not-disturb` guard;
      fallback to `notify-rust`.
- [ ] **Incoming-call notification**: high-priority, sticky notification / floating popup window
      with **Answer (video) / Answer (audio) / Decline** actions, ringing sound, auto-dismiss.
- [ ] Notification sounds (freedesktop `event-sound` / optional custom file); ringtone support.
- [ ] Tray icon + unread badge (`ksni` or `sntray`), left-click restore, right-click menu
      (Status: Available/Away/Busy/Do-not-disturb, Quit), **in-call indicator**.
- [ ] MPRIS (`org.mpris.MediaPlayer2`) for media-key mute/volume during calls.
- [ ] Single-instance lock (`$XDG_RUNTIME_DIR`) + second-launch focuses existing window.
- [ ] Deep links: register `x-scheme-handler/ms-teams` for `msteams://` URLs — meeting links
      must open **in-app** (Phase 8), falling back to the browser if calls are disabled.
- [ ] Wayland specifics: correct `app_id` for icon association, XDG activation for
      focus-on-notification-click, foreign-toplevel for window state.
- [ ] Packaging: `.desktop` file, icons (hicolor theme, symbolic + full colour), `.deb`, `.rpm`,
      Flatpak (org.freedesktop.Platform) with talk permission to notifications dbus names,
      **and** `--socket=wayland`, `--socket=fallback-x11`, `--talk-name=org.freedesktop.portal.*`
      for screen cast; note Flatpak + PipeWire portal caveats in docs.
- [ ] **Desktop audio routing**: verify PulseAudio/PipeWire device hot-plug mid-call.

**Exit criteria:** installable package; notifications clickable; tray badge reflects unread count;
incoming call rings with a working Answer/Decline from the notification.

---

## Phase 8 — Calls & meetings (full feature build-out)

*Requires the Phase 2B **GO** verdict. Broken into sub-milestones that each leave the app
shippable. Build under `crates/teams-call`, UI under `apps/teams-gui`.*

### M5a — Call core
- [ ] `teams-call` crate skeleton: Trouter signalling client reusing Phase 5 transport,
      `CallSession` state machine (`Idle → Outgoing → Ringing → Connecting → Connected →
      Reconnecting → Ended`) with exhaustive, tested transitions.
- [ ] WebRTC integration layer over the chosen stack: `PeerConnection` wrapper, SDP negotiation
      with glare handling, ICE with the server-provided STUN/TURN, DTLS-SRTP, stats collection.
- [ ] Media pipeline: `cpal` audio I/O (48 kHz Opus), jitter buffer, AEC/AGC/ANS
      (use `webrtc-audio-processing` or the stack's built-in), audio device enumeration.
- [ ] Video pipeline: capture (`v4l2`/PipeWire) → encode (H.264 HW via VAAPI/NVENC if available,
      VP8/VP9 SW fallback) → `webrtc-rs`/`str0m` video source; decode → render.
- [ ] `teams-cli call <user> | answer | hangup` — headless call for testing without GUI.

**Exit criterion:** CLI 1:1 audio+video call with another Teams account.

### M5b — 1:1 calls in the GUI
- [ ] Outgoing call flow: contact picker → ring → in-call window.
- [ ] Incoming call: popup window (positioned, always-on-top, Wayland-layer-shell where
      available) + notification actions from Phase 7.
- [ ] In-call UI: self-view preview, remote video surface (GPU texture blit into iced —
      validate this early!), mute/camera/hangup buttons, call duration, quality indicator.
- [ ] Device pickers (camera/mic/speaker) with live enumeration + hot-plug handling.
- [ ] Reconnect path: network drop → ICE restart → resume, with a "reconnecting…" state.
- [ ] Call history: recent calls list with missed/incoming/outgoing + durations (from chat
      conversation metadata).

**Exit criterion:** daily-usable 1:1 A/V calls.

### M5c — Group calls & meetings
- [ ] Meeting join: `msteams://` deep link → in-app join screen (preview: mic/camera check,
      device selection, "Join" / "Join muted") → join flow incl. **lobby** (waiting UI until
      admitted) and admitting others when you're organizer.
- [ ] Participant roster: presence, mute/camera state, speaking indicator (active speaker via
      WebRTC audio levels), hand-raise, pinning, spotlight, remove (organizer).
- [ ] Simulcast/SVC handling for N participants; adaptive subscriber layer based on
      available bandwidth; grid vs. speaker view layouts with virtualization for large meetings.
- [ ] In-meeting chat sidebar (reuses Phase 5 message stream), reactions & raise-hand that
      appear as floating reactions over video.
- [ ] Screen share: portal screencast with source picker (screen/window), share + speak,
      "sharing" indicator, stop-share button; X11 fallback; audio-optional.
- [ ] Content sharing inbound: view a presenter's shared screen/PPT as a video track,
      with a content-pinned layout.
- [ ] Meeting controls: mute-all, lock meeting, leave (with confirm), background blur
      (stretch: requires segmentation — ONNX/`tract` or the media stack's processor).
- [ ] Recording/live-transcription indicator (status display; actual recording control only if
      the signalling proves feasible — otherwise document as watch-only).

**Exit criterion:** join a real scheduled meeting, participate, share screen, be understood.

### M5d — Polish & hardening
- [ ] Quality/stats overlay (RTT, packet loss, bitrate, codec) + log bundle for bug reports.
- [ ] Push-to-talk key binding, global hotkeys (via `evdev`/portal), keyboard shortcuts for
      all call actions; hold/call-waiting if signalling supports it.
- [ ] Battery/CPU budget: hardware encode/decode paths, pause rendering when minimized.
- [ ] Failure matrix testing: callee offline, busy, declined, no answer, ICE failure,
      permission denied (mic/camera), portal dismissed, conference locked.

**Exit criteria:** full feature checklist from §0 demonstrable end-to-end.

---

## Phase 9 — Graph extras & content (post-v1 polish)

- [ ] Graph calendar (`Calendars.ReadWrite`): calendar view, join buttons for meetings.
- [ ] Graph mail (read), user profile.
- [ ] File/image upload via ASM objects (`https://api.asm.skype.com/v1/objects`).
- [ ] Search across messages, message pinning, channel creation, emoji/reaction picker polish.
- [ ] Activity/Feed tab, "saved messages", read receipts.
- [ ] Multi-account support, spellcheck (hunspell), custom CSS/themes.
- [ ] Telemetry-free crash reporter (opt-in log bundle).

---

## Risks & open questions

| # | Risk | Mitigation |
|---|------|-----------|
| 1 | **Calls protocol is undocumented and the highest-risk item** — signalling may be protobuf/skypemessage-encoded, media may require non-standard SDP. | Phase 2B go/no-go spike **before** committing to the build; MITM the official client as ground truth; keep a browser fallback path alive. |
| 2 | **Unofficial API** — Microsoft can change/break any endpoint without notice (Teams v2 already broke token capture for some tools). | Isolate all endpoints in `teams-api`/`teams-call`; keep fixtures; monitor `teams-for-linux` and related projects; version the protocol layer. |
| 3 | **Terms of Service** — unofficial clients may violate the M365 terms; orgs with Conditional Access/Intune may block them entirely. | Document clearly in README; make no claims of official status; support "cookie/WebView" fallback later if device-code is blocked. |
| 4 | **Auth breaks under MFA/CA/device-compliance** | Spike device-code + FRT in Phase 2A with a real work account *before* committing to the architecture. |
| 5 | **Scale of full A/V feature set** — group meetings with roster, lobby, screen share, simulcast is a multi-month effort beyond chat. | Sub-milestoned Phase 8 (M5a→M5d), each leaves a shippable app; cut line documented (background blur, recording control, live events). |
| 6 | **GUI framework churn** (iced/egui are 0.x) + GPU video compositing requirement. | Pin versions; validate video-texture blit during Phase 2B; keep UI logic thin, state in `teams-store`/core crates so a toolkit swap is survivable. |
| 7 | **WebRTC stack choice** (`str0m` vs `webrtc-rs` vs libwebrtc FFI). | Decide in 2B with a working PoC, not on paper. |
| 8 | Message HTML is a moving target. | Tolerant parsing; never fail to render — degrade to plain text. |
| 9 | PipeWire/portal behaviour differs per desktop (GNOME/KDE/Sway/X11). | Test matrix in CI-less manual checklist; document per-DE caveats; keep X11 fallback. |

---

## Suggested milestone order (summary)

1. **M0** Skeleton + CI + docs scaffold *(Phase 1)* ✅
2. **M1a** Chat/auth spike + docs *(Phase 2A)*
3. **M1b** **Call protocol spike → GO/NO-GO decision** *(Phase 2B)*
4. **M2** Auth works end-to-end from the CLI *(Phase 3)*
5. **M3** Read messages from the terminal *(Phase 4)*
6. **M4** Realtime + send messages *(Phase 5)*
7. **M5** GUI chat client usable daily *(Phase 6)*
8. **M6** Packaged, tray + notifications + incoming-call ringing *(Phase 7)*
9. **M7** Calls: core → 1:1 → meetings → polish *(Phase 8: M5a–M5d)*
10. **M8** Graph, uploads, search, multi-account *(Phase 9)*
