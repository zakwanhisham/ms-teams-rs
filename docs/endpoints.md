# Endpoint reference

> Research document — Phase 2A findings go here. Plan: [`../TODO.md`](../TODO.md).
> Add every endpoint you prove, with an authenticated reproduction command.
> Sanitize anything copied from captures.

## Conventions

- **Chat base URL**: `regionGtms.chatService` from the authz response (e.g.
  `https://<region>.ng.msg.teams.microsoft.com`) — **never hardcode a region**.
- **Chat auth header**: `Authentication: skypetoken=<jwt>` (note: *not* `Authorization`).
- **Bearer services**: `Authorization: Bearer <jwt>`, audience-specific token.
- Conversation ids contain `:` `@` `;` → percent-encode them in paths.
- Always send MS-CV correlation vectors and a Teams-like User-Agent.
- Response bodies in fixtures are sanitized.

## Chat service (IC3 / MSNP24)

| Op | Method + path | Status |
|----|---------------|--------|
| List conversations | `GET /v1/users/ME/conversations?view=msnp24Equivalent&pageSize=N&startTime=1` | ⬜ |
| List messages | `GET /v1/users/ME/conversations/{conv}/messages?pageSize=N&startTime=1` | ⬜ |
| Read one message | `GET /v1/users/ME/conversations/{conv}/messages/{id}` | ⬜ |
| Read thread (root + replies) | `GET /v1/users/ME/conversations/{conv};messageid={root}/messages` | ⬜ |
| Post message | `POST /v1/users/ME/conversations/{conv}/messages` | ⬜ |
| Reply | `POST /v1/users/ME/conversations/{conv};messageid={root}/messages` | ⬜ |
| Edit | `PUT /v1/users/ME/conversations/{conv}/messages/{id}` (+ `skypeeditedid`) | ⬜ |
| Delete | `DELETE /v1/users/ME/conversations/{conv}/messages/{id}` | ⬜ |
| React | `PUT /v1/users/ME/conversations/{conv}/messages/{id}/properties?name=emotions` | ⬜ |
| Mark read | `PUT /v1/users/ME/conversations/{conv}/properties?name=lastRead` | ⬜ |

Notes:
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

## authz

| Op | Method + path | Status |
|----|---------------|--------|
| Exchange | `POST https://authsvc.teams.microsoft.com/v1.0/authz` | ⬜ |
| Visitor (webinar) | `POST https://authsvc.teams.microsoft.com/v1.0/authz/visitor` | ⬜ |

See [auth.md](auth.md) for details.
