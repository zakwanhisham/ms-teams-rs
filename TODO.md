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
- [x] **Async runtime:** `tokio` (non-negotiable given reqwest + long-polling + WebRTC) —
      decided, already the workspace's runtime (reqwest, tests, ClI all built on it).
- [~] **Local storage:** `rusqlite` (bundled SQLite) for the message cache — still open
      for Phase 5. Tokens ✅ decided: `CredentialStore` trait with the `0600` file store
      (default) + the OS keyring via Secret Service (`keyring-core` +
      `dbus-secret-service-keyring-store`), selected by `TEAMS_RS_CREDENTIAL_STORE`;
      kernel keyrings rejected on quota (see `docs/auth.md`).
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
      `cargo-nextest` (CI, and installed locally 2026-10-28); `cargo-insta` still not
      installed locally — snapshots are accepted by promoting the `.snap.new` files
      insta writes on failure (see `tests/snapshots.rs`).
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
- [x] Install `cargo-deny` locally — done; `cargo deny check advisories licenses sources bans` passes clean. (`cargo-nextest` also installed locally 2026-10-28, v0.9.150.)
- [ ] Push to a git remote to enable the CI workflow.
- [ ] Decide the §0 GUI toolkit (blocks Phase 6, not Phase 2).

---

## Phase 2 — Research spike (protocol reverse engineering)

*Do this before writing much production code. Output goes into `docs/` + fixture files.
**Two spikes run in parallel — Chat (2A) and Calls (2B). 2B is the go/no-go for the project.** *

### Phase 2A — Chat/auth protocol

**Spike harness built (2026-10-27): `crates/teams-auth` + `teams-cli auth …` + `tools/capture/`.**

- [~] **Auth path**
      - [x] Device-code flow against the Teams **FOCI public client**
            `1fec8e78-bce4-4aaf-ab1b-5451cc387264` — **probe-verified live** (2026-10-27):
            `spaces.skype/.default` (with/without `openid offline_access`) and the O365
            suite scope all issue device codes; Graph scopes are rejected
            (`AADSTS70002`). See `docs/auth.md` for the probe table.
      - [~] Redeem FRT per-audience: `https://api.spaces.skype.com` ✅ **live**
            (FRT → spaces token → authz succeeded) and `chatsvcagg.teams.microsoft.com` ✅
            **live** (FRT → `aud=chatsvcagg`, `scp=user_impersonation`, feeds the CSA roster),
            `substrate.office.com` ✅ **live** (FRT → `aud=substrate` via
            `TokenCache::fresh_or_redeem` + `SCOPE_SUBSTRATE`; the live people search used it),
            `graph.microsoft.com` (⚠️ device-code path blocked for this client — open question
            deferred to Phase 9), **`go.trouter.io`** (needed for calls — not yet redeemed).
      - [~] **authz exchange:** ✅ **live** (`region=apac`, `regionGtms` with 112 services,
            `chatService = https://apac.ng.msg.teams.microsoft.com`); code handles both known
            response shapes (`tokens.skypeToken` and consumer `skypeToken.skypetoken`).
            **Raw body still uncaptured** — run `teams-cli auth login` under the proxy
            (`tools/capture/README.md`) to record `expiresIn` + the `regionGtms` key list.
      - [ ] Verify token refresh semantics: expiry claims, rotation of the FRT, what a 401 means.
      - [ ] MFA / Conditional Access / personal-vs-work account behaviour — document failures
            *(needs the account owner's login session)*.
- [~] **Chat service (IC3 / MSNP24)** — base `{chatService}` from `regionGtms`, header
      **`Authentication: skypetoken=<jwt>`** (not `Authorization`):
      - [x] `GET /v1/users/ME/conversations?view=msnp24Equivalent&pageSize=N&startTime=0` —
            **verified live** 2026-10-27; typed `ConversationList` model + sanitized fixture
            + 6 tests; `teams-cli conversations` prints the page.
            (Web client sends `startTime=0`, not `1`; paging is via `_metadata.syncState`.)
      - [x] `GET /v1/users/ME/conversations/{id}/messages?pageSize=N&startTime=0` —
            **verified live** 2026-10-27; typed `MessagePage`/`Message` model + sanitized
            fixture + 9 tests; `teams-cli history --conv <id>` prints the page.
      - [ ] `POST /v1/users/ME/conversations/{id}/messages` (+ `;messageid={root}` for replies)
      - [ ] `PUT …/messages/{id}` (edit, `skypeeditedid`), `DELETE`, reactions via
            `PUT …/messages/{id}/properties?name=emotions`
      - [x] Conversation/channel id handling in paths (`:` `@` `;` are sent **raw** — the
            service's own `messagesLink` does); only path-breaking chars are encoded
            (`teams-api::encode_path_segment`, unit-tested).
      - [~] Message HTML wire format: parsed into the `RichText` AST ✅ (Phase 4 — every
            captured message parses; mentions/bold/links/lists/tables/emoticons/inline
            images seen live; code/quotes/@everyone implemented but *not yet seen live*).
            Remaining here: wire evidence only (attachments e.g. `AMSImageInfo` posting
            flow) — rendering itself is Phase 6.
- [ ] **Realtime events** — pick one and prove it with a packet capture:
      - [ ] *(A)* **HTTP long-poll**: register endpoint → `POST /v1/users/ME/endpoints/{id}/active`
            (keepalive ~45 s, `timeout: 12`) → subscribe `interestedResources`
            (`/v1/threads/ALL`, `/v1/users/ME/conversations/ALL/messages`, …,
            `channelType: httpLongPoll`) → `POST …/subscriptions/{n}/poll` (blocks ~30 s).
      - [ ] *(B)* **Trouter** (`wss://go.trouter.io/v2/a`) — 🔶 endpoint confirmed live
            (unauthenticated upgrade → 401); required for call signalling anyway; likely
            becomes the primary transport once Phase 5 lands.
      - [ ] Recommendation: **(B) is mandatory for calls**, so implement it first and keep the
            long-poll as a fallback transport behind a trait.
- [x] **Team/channel roster**: `GET {csa}/api/v1/teams/users/me/updates?isPrefetch=false&enableMembershipSummary=true`,
      `{csa}` from `regionGtms.chatSvcAggAfd`, bearer `aud=chatsvcagg` minted from the FRT —
      **verified live** 2026-10-27; typed `RosterUpdate` model + sanitized fixture
      (`roster.json`) + 10 tests; `teams-cli roster` prints the summary. Teams arrive with
      **only their General channel**, the rest in the top-level `channels` array; `chats`
      doubles as the meeting list (join URLs).
- [x] **People/search**: `POST https://substrate.office.com/search/api/v1/suggestions`
      (needs `Origin`/`Referer: https://teams.microsoft.com` else 400) — **verified live
      2026-10-27**: `EntityRequests`/`People` body with `scenario=powerbar` returns real
      directory hits; typed `SearchSuggestions` model + sanitized fixture +
      `teams-cli search <q>`. Full request/response notes in `docs/endpoints.md`.
- [~] Record **fixtures**: sanitized JSON samples for every endpoint into
      `crates/*/tests/fixtures/` — ✅ `conversations.json`, `messages.json`, `thread.json`,
      `message.json`, `roster.json`, `search.json` landed with deserialization tests;
      every further endpoint needs the same treatment
      (`tools/sanitize_fixture.py` generates them).
- [x] **Capture tooling**: `tools/capture/teams_trace.py` (mitmproxy addon with JWT +
      credential redaction) + `tools/capture/README.md` scenario checklist.

### Phase 2B — Call protocol spike *(GO/NO-GO CHECKPOINT)*

- [x] **Capture tooling ready** — `tools/capture/` (mitmproxy addon with redaction +
      scenario checklist). The scenarios below still need the account owner to perform them.
- [x] **Execute the close-out plan — stage 0 ✅** — pipeline readiness reached: the addon
      records websocket connections (`websocket_start`/`websocket_message`/`websocket_end`,
      mitmproxy ≥12 event names, verified live end-to-end on mitmproxy 12.3.1): one
      `kind: websocket` JSONL record per connection with the scrubbed handshake,
      protocol/key, and buffered frames — text frames keep content (JWT-scrubbed,
      truncated to 64 KiB), `ping`/`binary` frames record direction and size only.
      Remaining stages' live probes (unauthenticated Trouter 401, registrar reachability)
      and the owner capture session (stages 1–5) still need the account owner.

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

**Built early as part of the Phase 2A spike harness — `crates/teams-auth` is live and a real
work-account login ran through it (2026-10-27).**

- [x] `DeviceCodeFlow`: prints the code + verification URL, polls with backoff (+5 s on
      `slow_down`, decline/expiry handled) — `device_code.rs`; verified by a live login.
- [x] `TokenCache`: FRT + per-audience access tokens + derived skype session
      (`skypetoken`, `region`, `regionGtms`, expiry), stored behind the `CredentialStore`
      trait: **`0600`** file under `$XDG_STATE_HOME/teams-rs/token-cache.json` by default,
      OS keyring (Secret Service) under `TEAMS_RS_CREDENTIAL_STORE=keyring` ✅ — keyutils
      rejected on measurement (20 KB quota vs a 30 KB cache), decision + evidence in
      `docs/auth.md`.
- [x] `AuthzClient`: POST authz, parse tokens + region map → `AuthzSession`; `AuthError`
      covers transport / identity / authz / parse failures (concrete variants are `Http`,
      `Identity`, `DeviceCode`, `Authz`, `Parse` rather than the sketch above); both known
      response shapes handled; verified live (`region=apac`, 112 `regionGtms` services).
- [x] Auto-refresh: refresh skew implemented (`is_expired(300)` + expired-token filtering
      on load) **and** the 401 hook is wired and **live-verified 2026-10-27**: a rejected
      credential (observed as chat 401 `errorCode:911`, and a poisoned-but-locally-fresh
      CSA bearer) → `TokenCache::evict` (keeps the FRT) → re-mint
      (`teams_api::AuthzRefresher` / `AudienceRefresher`) → replay exactly once; wired
      into all four `teams-cli` read commands, mocked-HTTP tests in
      `crates/teams-api/tests/refresh.rs`.
- [x] `teams-cli auth login | logout | status | whoami | authz | mint` (`mint` redeems a
      cached FRT for another audience and prints its claims — never the token) (JWT claims decoded for
      display only, no signature verification; token values are never printed).
- [x] Unit tests with mocked HTTP (`wiremock`) — `tests/device_code_http.rs` (code issue
      with form assertions, AADSTS identity errors, `authorization_pending` →
      `slow_down` → success with the +5 s backoff measured, decline/expiry/deadline
      endings, refresh grant) and `tests/authz_http.rs` (enterprise + consumer shapes,
      non-2xx, missing skypetoken); endpoints injectable via `with_login_base` /
      `with_endpoint`. The poll floor (5 s) makes that suite take ~20 s wall time.

**Exit criteria:** ✅ `teams-cli auth login && teams-cli auth status` works (live,
2026-10-27); tokens survive a process restart (later CLI runs reload them from the cache).

---

## Phase 4 — `teams-protocol` + `teams-api` (read path)

- [x] Strongly-typed models: `ConversationList`/`Conversation`/`MessagePage`/`Message`/
      `Mention`/`Emotion`(+`EmotionUser`) ✅ (`teams-protocol/src/{conversations,messages}.rs`)
      — one `Message` type serves both `lastMessage` and the messages page;
      `RosterUpdate`/`Team`/`Channel`/`ChatSummary`/`MeetingInformation`/`PreviewMessage` ✅
      (`teams-protocol/src/roster.rs`);
      `MessageKind` ✅ (`PlainText`/`RichText`/`CallLog`/`System`/`Unknown` — grounded in
      the live corpus; **`code` and `image` messagetypes never appeared**: image posts are
      `RichText/Html` + `amsreferences`+`blurHash` (7 messages live), so they stay
      `RichText` and `Message::has_inline_media()` distinguishes them);
      `EventMessage` ✅ (`teams-protocol/src/events.rs`, call-log XML → participants +
      occurrence, lenient hand scan, fixture `event-call.json`); `User` ✅
      (`users.rs` — MRI + display/given/family names + tenant assembled from the
      observed sender fields, no dedicated wire object exists).
- [x] Serde deserializers tolerant of unknown fields (`#[serde(default)]`, ignore-unknown) —
      enforced by tests for the conversation **and** message types; keep doing it per type.
- [ ] `ChatClient` with methods:
      - [x] `list_conversations(page_size)` — typed `ConversationList`, chats + channels in
            one page (verified live; `teams_api::ChatClient::from_session` builds it from the
            cached authz session)
      - [x] `list_messages(conv, page_size)` — typed `MessagePage`, lenient
            `mentions()`/`emotions()` accessors (stringified-JSON properties), system/edit
            classification helpers (verified live)
      - [x] `get_message(conv, id)` / `get_thread(conv, root_id)` (via `;messageid=`) —
            **verified live** 2026-10-27. Thread reuses `MessagePage`; a single message is a
            bare `Message` (adds `fromTenantId`). Fixtures: `thread.json`, `message.json`.
      - [x] `roster()` (CSA) — typed `RosterUpdate`, **verified live** 2026-10-27;
            fixture `roster.json`. Bearer minted per audience via
            `TokenCache::fresh_or_redeem` (`teams-cli auth mint`).
      - [x] `search_people(q)` (substrate) — **verified live** 2026-10-27;
            `SubstrateClient::search_people` + `teams-cli search <q>`, fixture `search.json`.
            Substrate has no `regionGtms` entry, so its base URL is a documented constant
            exception (`SUBSTRATE_BASE`).
- [x] Central `ApiClient` handling: base URL from `regionGtms` ✅ (via `AuthzSession::host`),
      correct auth header per service (`Authentication` vs `Authorization`) ✅, `ApiError`
      ✅; MS-CV correlation vectors ✅ (`CorrelationVector` — one per client, extension
      incremented per attempt, live-verified: the chat service echoes it extended with
      `.0`), jittered exponential backoff with 429/`Retry-After` ✅ (`RetryPolicy` in
      `teams-api::service`, statuses 429/502/503/504, honouring `Retry-After` verbatim,
      ±25 % jitter, overridable per client; mocked-HTTP tests in
      `crates/teams-api/tests/retry.rs`). Transport errors are deliberately *not* retried
      so the future write path cannot double-send.
- [x] HTML content model: parse Teams wire HTML → internal `RichText` AST (so the GUI can
      render it natively rather than embedding a webview) — `teams-protocol/src/richtext.rs`,
      html5ever 0.39 (+ `markup5ever_rcdom` 0.39, versions must pair) fragment parser +
      walker; dialect documented in `docs/endpoints.md` ("RichText wire dialect").
      **Verified against all 121 RichText/Html messages in the live captures
      (2026-10-28): 0 parse failures, 0 content-less parses; 28/28 mentions resolved
      via `properties.mentions`, all 24 emoticon names recovered from the wire.**
      - [x] mentions (`span`+`itemid` cross-reference), bold (`strong`), links, lists
            (incl. the misnesting Teams sends), inline images/emoticons, `br`,
            `margin-left` paragraph runs, tables
      - [x] italic/underline/strike, inline `code`, `pre` blocks, `blockquote`,
            headings — implemented for the standard dialect but *not yet seen live*
            (labelled in code)
      - [~] `@everyone` — never seen live so far (only `person` mentions); if it appears
            outside the mention-span dialect it degrade to text like any unmodelled tag
- [x] Snapshot tests from Phase 2 fixtures ✅ — `crates/teams-protocol/tests/snapshots.rs`
      (`insta`, json snapshots) pins the parsed models of all six sanitized fixtures plus
      the call-event fixture and a RichText projection; regenerate with `cargo insta`.
      The sanitizer gained an `Event/Call` XML mode (tag structure kept, names/emails/
      call times/mailbox ids/URLs redacted).

**Exit criteria:** ✅ met — `teams-cli conversations` and `teams-cli history --conv <id>`
both print real data (verified live 2026-10-27). Beyond the exit criteria: `get_message`/
`get_thread` ✅, `roster()` ✅, `search_people(q)` ✅, retry/MS-CV ✅ (2026-10-27), the
`RichText` HTML AST ✅ (every capture parses, 2026-10-28), remaining typed models ✅ and
snapshot tests ✅ (2026-10-28). **Phase 4 item checklist is fully closed.**

---

## Phase 5 — `teams-realtime` + write path

- [ ] **Trouter transport (primary)**: websocket client, auth handshake, message envelope
      codec, auto-reconnect with backoff, MS-CV headers. Shared with `teams-call` signalling.
- [ ] **HTTP long-poll transport (fallback)**: endpoint registration + `/active` keepalive
      (45 s cadence, jitter), subscription manager, resume-from-cursor (persist watermark).
- [ ] Transport abstraction: `RealtimeTransport` trait; pick Trouter when available, fall back
      to long-poll; connection state surfaced to the UI.
- [~] Event decoding → typed `TeamEvent` (`MessageCreated`, `MessageEdited`, `MessageDeleted`,
      `ReactionChanged`, `ConversationUpdated`, `Typing` decode now — shapes from the
      committed traces; `Presence`, `EndpointExpired`, **`IncomingCall`** have no observed
      wire shape yet and stay unlinked until Trouter auth works).
- [ ] **Write operations:** (live-verified 2026-10-29 + landed in `teams-api::ChatClient`
      — send/edit/delete/react/isread/consumptionhorizon/typing)
      - [x] send text message (capture server id from the `Location` response header, falling
            back to `OriginalArrivalTime`) — never trust the echoed `clientmessageid`
      - [ ] reply-to-thread (`{conv};messageid={root}`) — path shape implemented for reads
            (`get_thread`), the write goes through the same POST
      - [x] edit (remember: `properties` is *replaced*, not merged — re-send `mentions` on
            edit), delete, react, mark-as-read (`isread` per message +
            `consumptionhorizon` conversation cursor)
      - [ ] typing indicator done; presence (`presenceDocs/messagingService`) still ⬜
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
2. **M1a** Chat/auth spike + docs *(Phase 2A)* — ✅ live chat read path proven
3. **M1b** **Call protocol spike → GO/NO-GO decision** *(Phase 2B)* — ⏳ needs the account owner's captures
4. **M2** Auth works end-to-end from the CLI *(Phase 3)* ✅ — exit criteria met; 401-retry hook ✅ live-verified, keyring backend ✅ (`CredentialStore`)
5. **M3** Read messages from the terminal *(Phase 4)* ✅ — exit criteria met and the
   full item checklist closed: roster/search/MS-CV/retries ✅, RichText AST ✅ (all 121
   live captures parse), typed models + snapshot tests ✅
6. **M4** Realtime + send messages *(Phase 5)*
7. **M5** GUI chat client usable daily *(Phase 6)*
8. **M6** Packaged, tray + notifications + incoming-call ringing *(Phase 7)*
9. **M7** Calls: core → 1:1 → meetings → polish *(Phase 8: M5a–M5d)*
10. **M8** Graph, uploads, search, multi-account *(Phase 9)*
