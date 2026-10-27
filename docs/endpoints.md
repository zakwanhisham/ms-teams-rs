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
- **Bearer services**: `Authorization: Bearer <jwt>`, audience-specific token.
- Conversation ids contain `:` `@` `;` → the service writes them into paths **raw** (its
  own `messagesLink` values do), so `teams-api` keeps them raw and percent-encodes only
  path-breaking characters (`?` `#` `%` `/`, controls) — see `encode_path_segment`.
- Always send MS-CV correlation vectors and a Teams-like User-Agent.
- Response bodies in fixtures are sanitized.

## Chat service (IC3 / MSNP24)

| Op | Method + path | Status |
|----|---------------|--------|
| List conversations | `GET /v1/users/ME/conversations?view=msnp24Equivalent&pageSize=N&startTime=0` | ✅ |
| List messages | `GET /v1/users/ME/conversations/{conv}/messages?pageSize=N&startTime=0` | ✅ |
| Read one message | `GET /v1/users/ME/conversations/{conv}/messages/{id}` | ✅ |
| Read thread (root + replies) | `GET /v1/users/ME/conversations/{conv};messageid={root}/messages` | ✅ |
| Post message | `POST /v1/users/ME/conversations/{conv}/messages` | ⬜ |
| Reply | `POST /v1/users/ME/conversations/{conv};messageid={root}/messages` | ⬜ |
| Edit | `PUT /v1/users/ME/conversations/{conv}/messages/{id}` (+ `skypeeditedid`) | ⬜ |
| Delete | `DELETE /v1/users/ME/conversations/{conv}/messages/{id}` | ⬜ |
| React | `PUT /v1/users/ME/conversations/{conv}/messages/{id}/properties?name=emotions` | ⬜ |
| Mark read | `PUT /v1/users/ME/conversations/{conv}/properties?name=lastRead` | ⬜ |

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
    "mentionType":"person","displayName":…}` — `itemid` is the character offset.
  - Reaction entry: `{"key":"like"|"think"|<emoji-shortcode>,
    "users":[{"mri","time"(<number>),"value"(<string>)}]}`; counts are mirrored in
    `annotationsSummary.emotions` (an object, not stringified).
  - Edit marker is `skypeeditedid` (equals `clientmessageid` of the edit); delete
    bookkeeping lives in `properties.adminDelete`/`deletetime`/`hardDelete*`.
  - `contenttype` casing is inconsistent (`Text` and `text` both observed).
- **Read thread — verified live 2026-10-27** (typed as `teams_protocol::MessagePage`,
  fixture `crates/teams-protocol/tests/fixtures/thread.json`).

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
| Team/channel roster | `GET https://teams.microsoft.com/api/csa/{region}/api/v1/teams/users/me/updates` | `chatsvcagg.teams.microsoft.com` | ⬜ |
| People search | `POST https://substrate.office.com/search/api/v1/suggestions?scenario=powerbar` | `substrate.office.com` (needs `Origin`/`Referer`!) | ⬜ |
| Calendar view | `GET https://graph.microsoft.com/v1.0/me/calendarView?startDateTime=&endDateTime=` | `graph.microsoft.com` | ⬜ |
| Mail | `GET https://graph.microsoft.com/v1.0/me/messages` | `graph.microsoft.com` | ⬜ |
| Profile | `GET https://graph.microsoft.com/v1.0/me` | `graph.microsoft.com` | ⬜ |

## Files (ASM)

| Op | Method + path | Status |
|----|---------------|--------|
| Create object | `POST https://api.asm.skype.com/v1/objects` | ⬜ |
| Upload content | `PUT/POST https://api.asm.skype.com/v1/objects/{id}/content/{view}` | ⬜ |
| Fetch view | `GET https://api.asm.skype.com/v1/objects/{id}/views/{view}` | ⬜ |

## Calls (Phase 2B — see [calls.md](calls.md))

Signalling, meeting join, media/TURN allocation: documented in `calls.md`.

## Trouter websocket (call signalling — Phase 2B)

| Op | Method + path | Status |
|----|---------------|--------|
| Websocket endpoint | `wss://go.trouter.io/v2/a` | 🔶 exists: unauthenticated upgrade probe returns **HTTP 401, empty body** (2026-10-27) — auth material TBD from capture |

## authz

| Op | Method + path | Status |
|----|---------------|--------|
| Exchange | `POST https://authsvc.teams.microsoft.com/v1.0/authz` | 🔶 implemented in `teams-auth::AuthzClient`; live response shape pending login |
| Visitor (webinar) | `POST https://authsvc.teams.microsoft.com/v1.0/authz/visitor` | ⬜ |

Identity platform (device-code) endpoints are documented in [auth.md](auth.md) — probe-verified 2026-10-27.
