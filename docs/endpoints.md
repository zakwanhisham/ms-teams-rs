# Endpoint reference

> Research document — Phase 2A findings go here. Plan: [`../TODO.md`](../TODO.md).
> Add every endpoint you prove, with an authenticated reproduction command.
> Sanitize anything copied from captures.
>
> **Status:** ✅ verified live · 🔶 implemented, awaiting live verification · ⬜ open

## Conventions

- **Chat base URL**: `regionGtms.chatService` from the authz response (e.g.
  `https://<region>.ng.msg.teams.microsoft.com`) — **never hardcode a region**.
- **Chat auth header**: `Authentication: skypetoken=<jwt>` (note: *not* `Authorization`).
- **CSA base URL**: `regionGtms.chatSvcAggAfd` (e.g. `https://teams.microsoft.com/api/csa/<region>`)
  — the region is already part of the base; bearer audience `chatsvcagg.teams.microsoft.com`.
- **Bearer services**: `Authorization: Bearer <jwt>`, audience-specific token.
- Conversation ids contain `:` `@` `;` → the service writes them into paths **raw** (its
  own `messagesLink` values do), so `teams-api` keeps them raw and percent-encodes only
  path-breaking characters (`?` `#` `%` `/`, controls) — see `encode_path_segment`.
- Always send MS-CV correlation vectors and a Teams-like User-Agent — both implemented in
  `teams-api::HttpService` (one `CorrelationVector` per client, extension incremented per
  attempt; the chat service echoes `MS-CV` extended with its own `.0` component, observed
  live 2026-10-27) and used by every read path.
- **401 handling**: never blind-retry it — the credential is dead. The client evicts the
  rejected token and re-mints it (`teams_api::AuthzRefresher` re-runs the authz exchange,
  `AudienceRefresher` redeems the FRT from the on-disk cache; both persist the new
  credential), then replays the request **exactly once**; a second 401 surfaces to the
  caller. Observed live: the chat service answers 401 with
  `{"errorCode":911,"message":"Authentication failed."}`; both recovery paths were
  live-verified 2026-10-27 by poisoning the cached credential. Backoff (429/502/503/504)
  and transport errors remain deliberately un-retried — see `crates/teams-api/src/retry.rs`.
- Response bodies in fixtures are sanitized.

## Chat service (IC3 / MSNP24)

| Op | Method + path | Status |
|----|---------------|--------|
| List conversations | `GET /v1/users/ME/conversations?view=msnp24Equivalent&pageSize=N&startTime=0` | ✅ |
| List messages | `GET /v1/users/ME/conversations/{conv}/messages?pageSize=N&startTime=0` | ✅ |
| Read one message | `GET /v1/users/ME/conversations/{conv}/messages/{id}` | ✅ |
| Read thread (root + replies) | `GET /v1/users/ME/conversations/{conv};messageid={root}/messages` | ✅ |
| Post message | `POST /v1/users/ME/conversations/{conv}/messages` | ✅ 2026-10-29 |
| Reply | `POST /v1/users/ME/conversations/{conv};messageid={root}/messages` | ⬜ |
| Edit | `PUT /v1/users/ME/conversations/{conv}/messages/{id}` (+ `skypeeditedid`) | ✅ 2026-10-29 |
| Delete | `DELETE /v1/users/ME/conversations/{conv}/messages/{id}` | ✅ 2026-10-29 |
| React / unreact | `PUT /v1/users/ME/conversations/{conv}/messages/{id}/properties?name=emotions` | ✅ 2026-10-29 |
| Typing | `POST /v1/users/ME/conversations/{conv}/messages` (`Control/Typing`) | ✅ 2026-10-29 |
| Read receipt (per message) | `PUT /v1/users/ME/conversations/{conv}/messages/{id}/properties?name=isread` | ✅ 2026-10-29 |
| Mark read (conversation cursor) | `PUT /v1/users/ME/conversations/{conv}/properties?name=consumptionhorizon` | ✅ 2026-10-29 |

Notes:
- **List conversations — verified live 2026-10-27** (typed model:
  `teams_protocol::ConversationList`, fixture `crates/teams-protocol/tests/fixtures/conversations.json`).

  ```bash
  cargo run -q -p teams-cli -- conversations --page-size 5
  # fixture source (gitignored): --raw-out docs/traces/live-conversations.json
  ```

  Caveats found live:
  - `startTime=0` (not `1`) is what the web client sends for the first page.
  - Root is `{_metadata, conversations}`; `_metadata` carries `syncState` / `backwardLink` /
    `forwardLink` (479-char opaque cursor) / `totalCount`. `forwardLink: ""` = last page.
    Pagination is **cursor-based**: pass `_metadata.syncState` back rather than `startTime`.
  - Each item's `type` is `"Conversation"`; the real discriminator is
    `threadProperties.productThreadType` — observed: `TeamsTeam`, `TeamsStandardChannel`,
    `Meeting`, `StreamOfNotifications`. `threadType` distinguishes `space` (uses
    `spaceThreadTopic`) from `topic` (channels, use `topic`).
  - Conversation ids vary by kind: `19:…@thread.tacv2`, `19:…@thread.skype`,
    `19:meeting_…@thread.v2`, `48:notifications` — sent raw in paths (see Conventions).
  - `lastMessage` carries the same 17-key prefix of the messages-page `Message` type, so
    both are modeled by `teams_protocol::Message`.
  - Stringified booleans (`favorite`, `collapsed`, `isemptyconversation`,
    `gapDetectionEnabled`, …) are strings, not JSON bools — `isReader`/`isCreator` *are*
    bools. Keep the wire type as observed.
- **List messages — verified live 2026-10-27** (typed model:
  `teams_protocol::MessagePage`, fixture `crates/teams-protocol/tests/fixtures/messages.json`).

  ```bash
  cargo run -q -p teams-cli -- history --conv '<id from `conversations`>' --page-size 10
  # fixture source (gitignored): --raw-out docs/traces/live-messages.json
  ```

  Caveats found live:
  - Same paging contract as the conversations page (`_metadata.syncState` +
    `backwardLink`); this page carries `lastCompleteSegmentStartTime` /
    `lastCompleteSegmentEndTime` instead of `forwardLink`/`totalCount`. Page order is
    newest-first but not strictly monotonic — the thread root can appear inside a later
    page than its replies. Never assume sorted order; sort by `sequenceId` if you need it.
  - Root also carries `tenantId`.
  - **`content` is polymorphic**: HTML for `RichText/Html`, plain text for `Text`, and a
    separate XML dialect for `ThreadActivity/*` (e.g.
    `<addmember><target>8:orgid:{guid}</target><eventtime>…</eventtime><rosterVersion>…`
    `<initiator>28:app:{tenant}_{app}</initiator></addmember>`). System messages have no
    `contenttype` and no `imdisplayname`.
  - **`properties` is heterogeneous**: `mentions`/`cards`/`files`/`links` arrive as
    *stringified* JSON (`"[{…}]"`, `"[]"`), `emotions`/`deltaEmotions` as real arrays,
    flags as bools (`adminDelete`, `skipfanouttobots`) or strings. Typed accessors
    (`Message::mentions`, `Message::emotions`) parse leniently and return `[]` rather
    than failing the page; everything else lands in the property catch-all
    (`deletetime`, `draftId`, `eventReason`, `subject`, …).
  - Mention entry: `{"@type":"http://schema.skype.com/Mention","itemid":N,"mri":…,
    "mentionType":"person","displayName":…}` — `itemid` is **not** a character offset but
    the cross-reference index the RichText span carries (`<span … itemid="N">`); resolve
    by equality, see the wire dialect note below.
  - Reaction entry: `{"key":"like"|"think"|<emoji-shortcode>,
    "users":[{"mri","time"(<number>),"value"(<string>)}]}`; counts are mirrored in
    `annotationsSummary.emotions` (an object, not stringified).
  - Edit marker is `skypeeditedid` (equals `clientmessageid` of the edit); delete
    bookkeeping lives in `properties.adminDelete`/`deletetime`/`hardDelete*`.
  - `contenttype` casing is inconsistent (`Text` and `text` both observed).
  - **Image posts (seen live 2026-10-28, 7 messages)**: `RichText/Html` bodies carrying
    `amsreferences` (one AMS object id, `0-jhb-…`) and a `properties.blurHash`
    placeholder; the body embeds `<img src="https://as-api.asm.skype.com/v1/objects/…">`.
    There is no separate image `messagetype` — distinguish via `Message::has_inline_media`.
  - **Call-log events (seen live 2026-10-28, 37 messages)**: `messagetype: Event/Call`,
    `contenttype: application/user+xml`, `properties.skypeguid` (call session UUID) and
    `s2spartnername: concore_gvc`. Body XML dialect (documented verbatim; sanitized
    fixture `event-call.json`):
    - an optional leading state element — `<ended/>` on ended calls, *absent* on
      call-started bodies (18/37 had none — `started` bodies carry no marker);
    - `<partlist alt… count=N>` with one `<part identity="8:orgid:…">` per attendee,
      each holding `<name>` (MRI), `<displayName>`, `<duration>` (seconds); started
      calls carry an empty partlist;
    - a **doubly-nested** `<meetingDetails><meetingDetails>…` block: `<organizerUpn>`,
      `<meetingType>`, `<startTime>`/`<endTime>` (human-formatted), `<exchangeId>`,
      `<iCalUid>`, `<hasEphemeralTranscript>`, `<isRecapEnabled>`, and an inner
      `<instanceDetails>` (occurrence `<type>`, `<subject>`, Times, `<iCalUid>`);
    - outer sibling leaves: `<callEventType>` (`callStarted`/`callEnded`),
      `<conversationStartTime>`, `<conversationUrl>` (flightproxy URL), `<callId>`,
      `<wasGcpBotInvited>`.
    Typed access: `Message::event_message()` (`teams_protocol::EventMessage`).
- **Read thread — verified live 2026-10-27** (typed as `teams_protocol::MessagePage`,
  fixture `crates/teams-protocol/tests/fixtures/thread.json`).

## RichText wire dialect (observed 2026-10-28)

Bodies with `messagetype = "RichText/Html"` are compact HTML. Captured inventory
(121 messages across the gitignored `docs/traces/rt-*.json` traces): `p`(260),
`span`(70), `td`(51), `li`(43), `a`(43), `br`(35), `img`(31), `strong`(28),
`tr`(17), `ul`(8), `table/tbody`(3), `ol`(2). Parser: `teams-protocol/src/richtext.rs`
(html5ever 0.39 fragment parse → AST walk); verified to parse all 121 with zero
failures. Grammar details, all live-observed:

- **Paragraph runs**: top-level `<p>`; `style="margin-left:Npx"` (measured value: `0px`,
  22×) travels in the AST as `indent_px` — semantics of the npx runs beyond `0` are
  **unknown, do not invent** (no live message carried `Npx` ≠ `0` yet).
- **Mentions**: `<span itemscope itemtype="http://schema.skype.com/" itemid="N">`
  whose *text* is the rendered name; `N` indexes `properties.mentions[N]`
  (28/28 matched in the captures — `itemid` is a cross-reference, **not a character
  offset**). Non-numeric `itemid`s on spans are a different construct
  (`app-preview-card-…` app cards) and must not be treated as mentions.
- **Emoticons**: `<span class="animated-emoticon-20-…" type="(smile)" itemscope
  title="…"><img … itemid="smile"></span>` — `type` carries the canonical name in
  parentheses (inventory seen live: `smile`, `cake`, `cakeslice`, `1f389_partypopper`);
  the child `img`'s `itemid` and the class tail agree with it.
- **Links**: `<a href target=_blank rel="noreferrer noopener">`; SharePoint file cards
  additionally carry `itemid="…guid"`/`itemtype="http://schema.skype.com/…"`.
- **Lists**: `<ul>"/"<ol><li>` outdoors, *but often misnested* — `p` containing `ul`,
  `li` directly inside `li`, `li` containing `p` runs. html5ever's tree construction
  normalizes this (a real parser was required; string handling can't reconstruct it).
- **Tables**: `table > tbody > tr > td` (no `th` seen live; parser marks `th` as header
  defensively).
- `contenttype` says `"Text"` even for HTML bodies in most captures — the
  discriminator is `messagetype == "RichText/Html"`.
- **Implemented but never seen live** (standard dialect, labelled in code): `em`/`i`,
  `u`, `s`/`del`, inline `code`/`pre`/`blockquote`, headings `h1`–`h6`, `hr`,
  and `@everyone` (only `person` mentions observed, 28×). Anything unmodelled degrades
  transparently to its text content.

## Services, payloads — alphabetical

  ```bash
  cargo run -q -p teams-cli -- history --conv '<id>' --root '<root message id>' --page-size 20
  ```

  Caveats:
  - `;messageid={root}` is glued to the **conversation id segment**
    (`…/conversations/19:…@thread.tacv2;messageid=1700…/messages`), not a separate path
    element. `teams-api::thread_path` builds it and unit-tests the exact string.
  - The envelope is the plain messages page: `{_metadata, messages, tenantId}` — but
    `_metadata` here carries only `syncState` + the two segment bounds (no
    `backwardLink`), so a thread cannot be paged backwards the way a conversation can.
  - Ordering is **not** root-first: the root lands wherever its `sequenceId` puts it
    (observed mid-list), so identify it by `id == rootMessageId` rather than position.

- **Read one message — verified live 2026-10-27** (typed as `teams_protocol::Message`,
  fixture `crates/teams-protocol/tests/fixtures/message.json`).

  ```bash
  cargo run -q -p teams-cli -- history --conv '<id>' --message '<message id>'
  ```

  Caveats:
  - The response is a **bare message object**, not a page — no `_metadata`, no `messages`
    wrapper.
  - It carries one field the pages do not: `fromTenantId` (36-char GUID).

- Server message id comes from the `Location` response header on POST (last path segment),
  falling back to `OriginalArrivalTime`. Never use the echoed `clientmessageid`.
- `properties` is **replaced**, not merged, on edit — re-send `properties.mentions`.

### Write shapes — verified live 2026-10-29 (own `48:notifications` feed; every created
### message deleted afterwards, delete → `200`/`null` re-verified each round)

- **Post**: `POST …/messages`, body
  `{clientmessageid:"<ms epoch>", content:"<div>…</div>", contenttype:"text",
  messagetype:"RichText/Html"}` → `201 Created`, body `{"OriginalArrivalTime":<ms>}`,
  id **only** in the `Location` header (13-digit tail). Also works without
  `clientmessageid`. Implementation note: `ChatClient::send_message` never trusts the
  echoed client id.
- **Edit**: `PUT …/messages/{id}` `{content, contenttype, messagetype,
  properties:{skypeeditedid:"<id>"}}` → `200`, **empty** body. Without the
  `skypeeditedid` property the service rejects the change (the property proves the
  client has the newest version).
- **Delete**: `DELETE …/messages/{id}` → `200`, body `null`. Even without
  `clientmessageid`, a message created then deleted in the same second deletes fine.
- **React**: `PUT …/messages/{id}/properties?name=emotions`, body
  `{"emotions":{"key":"like","value":<ms epoch>}}` → `200`; the reaction persists and a
  follow-up `GET …/messages/{id}` shows `"annotationsSummary":{"emotions":{"like":1}}`.
  The `{key,value}` object shape was mined from the official web client bundle
  (`setMessageProperty` helper, `emotions:{key,value}` literal in
  `statics.teams.cdn.office.net/…/services-chat-…js`). Rejected shapes —
  `{"emotions":{"like":<ms>}}` (int or string), `{"emotions":{"key":<mri?>}}` without
  the wrapper: `400 errorCode:201 "Invalid format of emotions property"`.
  Unreact (bundle-derived, **not yet seen live**): `PUT` the same endpoint with
  `{"emotions":{"key":"like"}}` (no `value`).
- **Typing**: `POST …/messages` `{content:"", contenttype:"text",
  messagetype:"Control/Typing"}` → `201` like a normal send (a `Location`/arrival-time
  response, nothing rendered as a message).
- **Read receipt (per message)**: `PUT …/messages/{id}/properties?name=isread`,
  body `{"isread":"<ms epoch as string>"}` → `200`. Shape mined from the same bundle
  (`isread:{isread:n.toString()}`), then confirmed live.
- **Mark read (conversation cursor)**: `PUT …/conversations/{conv}/properties?name=
  consumptionhorizon`, body `{"consumptionhorizon":"<a>;<b>;<bookmark>"}` — the same
  `;`-delimited triple the read side (`view=msnp24Equivalent` conversations) returns,
  e.g. `<firstUnreadOrigArrival>;<newer time>;<large syncBookmark>`. Accepted with a
  synthetic bookmark (`200`); camelCase `name=consumptionHorizon` is rejected
  (`400 errorCode:201 "… property is not allowed"`). The `lastRead` name is also
  rejected with the same error — only lowercase `consumptionhorizon` is allowed on
  `regionGtms.chatService` IC3 bases. What `a`/`b`/bookmark each mean authoritatively
  is not settled; treat the triple as opaque for now except that field 1 is a message
  original-arrival time and the bookmark is a server-issued id (keep whatever the read
  side gave you when advancing the first two fields).

## Realtime

| Op | Method + path | Status |
|----|---------------|--------|
| Register endpoint | `POST /v1/users/ME/endpoints` (or `/v4/users/ME/endpoints`, TROUTER transport) | ⬜ |
| Keepalive | `POST /v1/users/ME/endpoints/{id}/active` `{timeout:12}` ~every 45 s | ⬜ |
| Subscribe | `POST /v1/users/ME/endpoints/{id}/subscriptions` (`interestedResources`, `channelType: httpLongPoll`) | ⬜ |
| Poll | `POST /v1/users/ME/endpoints/{id}/subscriptions/{n}/poll` (blocks ~30 s) | ⬜ |
| Trouter websocket | `wss://go.trouter.io/v2/a` | ⬜ |
| Registrar | `https://prod.registrar.skype.com/v2/registrations` | ⬜ |

Subscribed resources (web client): `/v1/threads/ALL`, `/v1/users/ME/contacts/ALL`,
`/v1/users/ME/conversations/ALL/messages`, `/v1/users/ME/conversations/ALL/properties`.

## Bearer services

| Op | Method + path | Audience | Status |
|----|---------------|----------|--------|
| Team/channel roster | `GET {csa}/api/v1/teams/users/me/updates?isPrefetch=false&enableMembershipSummary=true` | `chatsvcagg.teams.microsoft.com` | ✅ |
| People search | `POST https://substrate.office.com/search/api/v1/suggestions?scenario=powerbar` | `substrate.office.com` (needs `Origin`/`Referer`!) | ✅ |
| Calendar view | `GET https://graph.microsoft.com/v1.0/me/calendarView?startDateTime=&endDateTime=` | `graph.microsoft.com` | ⬜ |
| Mail | `GET https://graph.microsoft.com/v1.0/me/messages` | `graph.microsoft.com` | ⬜ |
| Profile | `GET https://graph.microsoft.com/v1.0/me` | `graph.microsoft.com` | ⬜ |

### Team/channel roster (CSA) — verified live 2026-10-27

Typed as `teams_protocol::RosterUpdate` (fixture `crates/teams-protocol/tests/fixtures/roster.json`),
client `teams_api::RosterClient`.

```bash
teams-cli auth mint    # FRT → chatsvcagg bearer, cached under `chatsvcagg`
teams-cli roster --raw-out docs/traces/live-roster.json
```

- **Base URL is data**: `regionGtms.chatSvcAggAfd` (observed
  `https://teams.microsoft.com/api/csa/apac`) — the region is the last path segment of the
  base, so the request is `{base}/api/v1/teams/users/me/updates?…`. `RosterClient` forces a
  trailing `/` on the base before `Url::join`, otherwise the join would *replace* the region
  segment instead of appending after it.
- **Auth**: plain `Authorization: Bearer <jwt>` with `aud:
  https://chatsvcagg.teams.microsoft.com`, redeemed from the family refresh token
  (`teams-cli auth mint`, scope
  `openid offline_access https://chatsvcagg.teams.microsoft.com/.default`). No skypetoken, no
  `Origin`/`Referer`, no `x-skypetoken` were needed — bearer alone worked.
- **Teams list only their General channel** (`channels[0].isGeneral == true`, 1 channel each
  in all five observed teams). Channels reported outside any team arrive in the top-level
  `channels` array with `parentTeamId` set. Merge both: `RosterUpdate::all_channels()`
  deduplicates by thread id. Anything else has to come from the chat-service conversation
  list (`GET /v1/users/ME/conversations`).
- **`chats` doubles as the meeting list**: 64 entries observed (31 `chatType: "meeting"`),
  meetings carrying `meetingInformation` with `meetingJoinUrl`, `startTime`/`endTime`,
  `isCancelled`, `organizerId`. Chat ids use the chat service's conversation-id form, so a
  chat id feeds `list_messages` unchanged.
- **Last-message previews are camelCase** (`originalArrivalTime`, `imDisplayName`,
  `containerId`, `clientMessageId`) — the opposite casing from the chat service's `Message`,
  hence the separate `teams_protocol::PreviewMessage`.
- `chatSubType` is an **integer** (0/1 across all 64 chats) despite the name; typed as
  `Value`.
- `metadata`: `syncToken` (incremental-update cursor), `forwardSyncToken`, `hasMoreChats`,
  `isPartialData` (both booleans `false` in the verified response = complete data).
- `privateFeeds` (notification/draft/mention/call-log streams) and `users` (empty in the
  verified response) are carried as raw `Value` — no consumer yet.
- `enableMembershipSummary=true` buys `membershipSummary` (total/user/admin/bot/muted
  counts) per team; without it the counts are absent.

### People search (substrate) — verified live 2026-10-27

Typed as `teams_protocol::SearchSuggestions`
(fixture `crates/teams-protocol/tests/fixtures/search.json`), client
`teams_api::SubstrateClient`.

```bash
teams-cli search ken --limit 5
# mint on demand, cached under `substrate`; fixture source (gitignored):
#   --raw-out docs/traces/live-search-people1.json
```

- **Base URL is a documented constant exception**: the authz `regionGtms` map has **no**
  `substrate` entry (all 112 live service keys checked 2026-10-27; only
  `substrateSyncS2S`/`search`/`peopleProfileService` exist), so the global
  `https://substrate.office.com` front door is used — `teams_api::SUBSTRATE_BASE`. This is
  the one base URL in `teams-api` that is *not* runtime data.
- **Auth**: `Authorization: Bearer <jwt>` with `aud: https://substrate.office.com`, redeemed
  from the family refresh token (scope
  `openid offline_access https://substrate.office.com/.default`, cached as `substrate`).
- **`Origin: https://teams.microsoft.com` and `Referer: https://teams.microsoft.com/` are
  required**; without them the endpoint answers HTTP 400 regardless of body.
- **Request body** (probed live — every rejected variant is listed below):

  ```json
  {"cvid": "<uuid4>", "logicalId": "<uuid4>",
   "EntityRequests": [{"Query": {"QueryString": "ken", "DisplayQueryString": "ken"},
                       "EntityType": "People", "Size": 5,
                       "Fields": ["Id", "MRI", "DisplayName", "EmailAddresses",
                                  "JobTitle", "Department"]}]}
  ```

  - `?scenario=powerbar` is required (GET without it → 400).
  - Missing `EntityRequests` → HTTP 500 `System.ArgumentNullException: 'EntityRequests'`.
  - Unknown name in `Fields` → HTTP 400 `Invalid field name specified.` — the field list
    above is the live-verified set; anything else must be proven before adding it.
  - Rejected alternatives: top-level `{Scenario, Query}` → 400 "POST message is missing or
    not valid", `{"message": …}` (string or object) → 500, form-encoded → 415,
    `GET ?query=` → 400.
  - `cvid`/`logicalId` are fresh UUIDv4 per call (correlation only; no state observed).
- **Response**: `{Groups: [{Suggestions: […], Type: "People"}], LayoutHints:
  {BestMatchSuggestions: [{ReferenceId, Group}]}, Instrumentation: {TraceId}}`. Ordering for
  the UI lives in `LayoutHints`, which *references* suggestions by `ReferenceId` instead of
  inlining them — `SearchSuggestions::best_match_order()` maps it back.
- **Suggestion fields observed**: `Id` (`<guid>@<tenant-guid>`), `MRI` (`8:orgid:<guid>`),
  `DisplayName` and `Text` (same string), `QueryText` (the matched email), `EmailAddresses[]`,
  `JobTitle`, `Department` (both optional), `PropertyHits[]` — enum-like matched properties
  (`DisplayName`, `GivenName`, `proxy_email` observed) — and `ReferenceId`.
- `Size` is an upper bound: a 10-sized request returned 3 hits in the verified tenant (it
  never pads the group).

## Files (ASM)

| Op | Method + path | Status |
|----|---------------|--------|
| Create object | `POST https://api.asm.skype.com/v1/objects` | ⬜ |
| Upload content | `PUT/POST https://api.asm.skype.com/v1/objects/{id}/content/{view}` | ⬜ |
| Fetch view | `GET https://api.asm.skype.com/v1/objects/{id}/views/{view}` | ⬜ |

## Calls (Phase 2B — see [calls.md](calls.md))

Signalling, meeting join, media/TURN allocation: documented in `calls.md`.

## Trouter websocket (call signalling — Phase 2B)

Live-verified 2026-10-29 from the official web client bundle
(`statics.teams.cdn.office.net/teams-modular-packages/hashed-assets/
cmd-trouter-2873cb3d61573f21.js`, i.e. `@skype/trouter-client` 4.1.36010 + socket.io
0.9.6 custom build) **and** our own probes with the cached skypetoken. Shared
infrastructure URLs (same class as go.trouter.io already quoted); no user data.

| Op | Method + path | Status |
|----|---------------|--------|
| Allocate | `POST https://go.trouter.io/v4/a` (or `https://go.trouter.teams.microsoft.com/v4/a`) | ✅ live 200 — `X-Skypetoken: <skypetoken>` + `X-MS-Migration: True`, `Content-Type: text/plain`, **empty body**; query `cor_id=<uuid>&con_num=<clientID>_<attempt>&tc=<url-enc JSON {"ua","v"}>&timeout=30` |
| Register | `POST {alloc.registrarUrl}` (live value: `https://edge.skype.com/registrar/prod/V3/registrations`) | ✅ live **202 Accepted, empty body** — `X-Skypetoken` + `X-MS-Migration: True`; auth via **`Authentication` header is wrong (401)** |
| Connect | `wss://{alloc.socketio host}/v4/c` (NO `/websocket/<sid>` suffix — 404; `wss://go.trouter.io/v4/c` also accepts upgrades) | ✅ upgrade + socket.io `1::` connect frame |
| Connect auth | emit `user.authenticate` | ❌ not yet accepted live (see below) — official-client frame format reconstructed, server response matrix recorded; the dashboard needs the owner capture |

**Allocation response (shape):**
`{ccid, id: "<clientID?>_<ccid>", socketio: "https://pub-csm-<region>-<n>-t.trouter.skype.com:443/",
surl: "https://…-f.trouter.skype.com:3443/v4/f/<session-token>/",
url: "https://…:8443/v4/f/…/", ttl: "<seconds as string, ~83k>",
healthUrl, curlb, registrarUrl, connectparams: {sr, issuer(""), sp: "<node>", se, st, scae: "1",
sig}}` — `scae=1` selects the v4c protocol; `se` is epoch-ms expiry of `sig`.

**Failed shapes (worth remembering):**
- `POST /v3/a` → 404; `POST /v3/c` → 400; allocating **without `X-Skypetoken` → 401**, and
  the `Authentication: skypetoken=…` header form → 401 — the bare `X-Skypetoken` header is
  what the allocator validates.
- Registration against `prod.registrar.skype.com`/`registrar.skype.com` → 401/404 — the V3
  registrar URL **from the allocation response** (`edge.skype.com`) is the live one.
- `clientDescription.templateKey` **required** (error `40041 'client description field
  "templateKey" is required for set registrations'`); accepted value found by guess:
  **`"MicrosoftTeams"`** → 202. Other candidates were not exercised against this URL.
- `clientDescription.productContext` must be a **JSON string** (an object → server error
  `$.clientDescription.productContext: Unexpected token StartObject`); accepted value:
  `"{\"appType\": \"Microsoft Teams Web\", \"productNum\": \"13\"}"`.
- WS endpoint suffix `/v4/c/websocket/<sid>` (socket.io's default transport path) → **404**.
  `/v4/f/<session-token>` path → **401**, not the connect route.
- Connect query (from bundle `buildV4cUrlParams` + live tests): `tc=<enc JSON>&timeout=40..70
  &ccid=<alloc.ccid>&epid=<registrationId>&cor_id=&con_num=` — **no `auth=true`** (dropped it;
  earlier `401` included it in a wrong-guess probe).
- `/v4/c` upgrade accepted on all hosts tried (`go.trouter.io`, `go.trouter.teams.microsoft.com`,
  the `socketio` pub-csm node), and the server drives socket.io 0.9 handshake: `1::` then a
  ~10 s idle `0:::{"reason":"timeout"}` if nothing is sent.

**v4c authentication (socket.io 0.9 emit, from bundle `sendV4cAuthenticationEvent` + encoder):**

```
5:::{"name":"user.authenticate","args":[{"headers":{
      "X-Skypetoken": "<skypetoken>",       // tokenType "skype":
      "X-MS-Migration": "True" },           //   usingLegacyTokenApi=false → migration header
    // — or for tokenType "aad"/"cae": {"Authorization": "Bearer <token>"},
    "connectparams": {…the allocation's connectparams…
    }  // `sr`,`issuer`,`sp`,`se`,`st`,`scae`,`sig`
    // official code copies alloc.connectparams, stripping serviceUrl/reconnectUrl (fe()).
}]}
```

Socket.io 0.9 wired frame encode: `<type>:<id>(+ack)?:<endpoint(array optional):...`: event
type `5`, JSON payload = `{"name": eventName, "args": [...]}` (NOT the bare array form;
bundle `encodePacket`: `case"event": …R=s.stringify({name, args})`). Disconnect frames arrive
as `0:::{"reason":"…"}`.

**Live server reactions to `user.authenticate` (2026-10-29, all hosts and both header forms):**

| Token form in emit | Server disconnect reason |
|--------------------|--------------------------|
| our cached `authz.skype_token` (via X-Skypetoken, ±migration header) | `badrequest` |
| garbage / AAD JWT / empty string in X-Skypetoken | `unauthorized` |
| `Bearer <token>` (spaces / chatsvcagg / teams.microsoft.com audiences) | `unauthorized` |
| no packet sent | `timeout` after ~10 s (so the upgrade path itself is fine) |
| X-Skypetoken as ws-upgrade headers | ignored (auth must be the emit) |

Wrong-audience AAD bearers are rejected *properly* (`unauthorized` = audience/claim checks
seen to run), so the emit **format** passes; our **skypetoken flavor** isn't the one the
official flow presents, and the right AAD audience for trouter hasn't been found — trouter
shape resources (`*trouter…) return AADSTS500011 (no such resource). What is needed: the
owner's **Phase 2B stages 1–5 capture** (mitmproxy websocket frames of the real client) to
settle the exact authenticate argument and token flavor, and follow-ups on the message
envelope (`trouter.connected` args, `user.activity`, subscription registration semantics +
`X-Microsoft-Skype-Chain-ID`/correlation-vector handling — all seen in the bundle only).

**Other verified timings (bundle):** ping/pong 40 s + 5 s, request timeout 5 s, backoff cap
30 s; `con_num=<clientID>_<connectionAttempt>`; reconnect cache `{serviceUrl, reconnectUrl,
…connectparams}` with `se` epoch-ms expiry dropped 1 h before; `trouter.message_loss` / bad
`purpose` handled by `Trouter-Responder: "ClientLib"` downstream-reply headers; legacy token
api = no `X-MS-Migration` header. `/v4/a`↔`/v4/c` rewriting by `scae` (v4c fail → v4a
websocket fallback, i.e. HTTP long-poll on the same session).


## authz

| Op | Method + path | Status |
|----|---------------|--------|
| Exchange | `POST https://authsvc.teams.microsoft.com/v1.0/authz` | 🔶 implemented in `teams-auth::AuthzClient`; live response shape pending login |
| Visitor (webinar) | `POST https://authsvc.teams.microsoft.com/v1.0/authz/visitor` | ⬜ |

Identity platform (device-code) endpoints are documented in [auth.md](auth.md) — probe-verified 2026-10-27.
