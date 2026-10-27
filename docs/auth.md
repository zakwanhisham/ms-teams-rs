# Authentication

> Research document — Phase 2A. Plan: [`../TODO.md`](../TODO.md).
> **Never paste real tokens, tenant ids or account names into this file.**

## Goal

Obtain and continuously refresh a **skypetoken** plus the service region map, which gates
every Teams API call.

## Flow (partially verified — see Findings)

```
device-code login ──► FRT (family refresh token)
                        │  redeem per audience
                        ▼
        access token (aud: https://api.spaces.skype.com)
                        │  POST https://authsvc.teams.microsoft.com/v1.0/authz
                        ▼
        { tokens.skypeToken, region, regionGtms{…} }
                        │
                        ▼
        chat service:  Authentication: skypetoken=<jwt>
        bearer services: Authorization: Bearer <jwt>
```

## Status legend

✅ verified against live services · 🔶 implemented, awaiting live login · ⬜ open

## Findings

### Identity platform probes (2026-10-27, unauthenticated `POST …/oauth2/v2.0/devicecode`)

| Scope (client `1fec8e78-bce4-4aaf-ab1b-5451cc387264`, tenant `common`) | Result |
|---|---|
| `openid offline_access https://api.spaces.skype.com/.default` | ✅ **HTTP 200** — device code issued |
| `https://api.spaces.skype.com/.default` | ✅ HTTP 200 |
| `openid profile offline_access 00000003-0000-0ff1-ce00-000000000000/.default` (O365 suite) | ✅ HTTP 200 |
| `openid offline_access https://graph.microsoft.com/.default` | ❌ HTTP 401 `invalid_client` / `AADSTS70002` "not enabled for consumers" |
| `openid profile offline_access User.Read` (delegated) | ❌ HTTP 401, same `AADSTS70002` |

Conclusions:
- The **FOCI Teams client accepts device-code grants** — the primary login path is viable.
  Default scope for the spike: `openid offline_access https://api.spaces.skype.com/.default`.
- **Graph cannot be reached through this client at the device-code stage** (neither
  `.default` nor delegated `User.Read`). Graph tokens likely require a different acquisition
  path (another client id, or in-session MSAL like teams-for-linux does). → open question,
  tracked in Phase 9.

### Implementation status (in `crates/teams-auth`)

| Step | Status | Notes |
|---|---|---|
| Device-code request + poll (incl. `slow_down`, decline, expiry) | ✅ | `device_code.rs`; live login succeeded (2026-10-27) |
| FRT → per-audience refresh | ✅ | `DeviceCodeClient::refresh`; FRT (`foci`) confirmed returned by live login |
| Token cache, `$XDG_STATE_HOME/teams-rs/token-cache.json`, mode `0600` | ✅ | `token_cache.rs`; permissions covered by test |
| authz exchange (both enterprise & consumer response shapes) | ✅ | `authz.rs`; live exchange succeeded, `regionGtms` map has 112 services |
| Claim decoding (unverified, display only) | ✅ | `jwt.rs` |
| CLI: `teams-cli auth login/status/authz/whoami/logout` | ✅ | never prints token values |

### Live findings (login 2026-10-27)

- `region = "apac"` for this tenant — but it is **data, not config**: always read the
  `regionGtms` map from the authz session, never hardcode a region.
- `regionGtms.chatService = https://apac.ng.msg.teams.microsoft.com` — host pattern
  `https://{region}.ng.msg.teams.microsoft.com`. This is the base URL for every
  `ChatClient` (`teams_api::ChatClient::from_session`).
- FRT is issued and stored with a `spaces.skype` refresh; the skype token expires in ~2 h.
- `expiresIn` is still **not** recorded: login ran outside the proxy, so the raw authz body
  was never captured. Re-run `teams-cli auth login` under `tools/capture/` to fill this in
  (status shows "unknown lifetime" meanwhile).
- Graph remains unreachable through this FOCI client at device-code time (probe table).

### Endpoint notes

- `POST https://login.microsoftonline.com/{tenant}/oauth2/v2.0/devicecode` — form:
  `client_id`, `scope`. Response: `device_code`, `user_code`, `verification_uri`,
  (`verification_uri_complete` when present), `expires_in`, `interval`, `message`.
- `POST …/oauth2/v2.0/token` — grants `device_code` / `refresh_token`; pending state is
  HTTP 400 `{"error":"authorization_pending"}`, back off +5 s on `slow_down`.
- `POST https://authsvc.teams.microsoft.com/v1.0/authz` — `Authorization: Bearer <spaces
  jwt>`, empty JSON body → `{tokens.skypeToken, region, regionGtms, expiresIn}` (enterprise
  shape; consumer shape differs — both handled). **Live response still to be captured**:
  run `teams-cli auth login` inside the proxy (`tools/capture/README.md`) and record the
  `regionGtms` key list here.

## Checklist

### Device-code login (FOCI client `1fec8e78-bce4-4aaf-ab1b-5451cc387264`)
- [x] `POST /{tenant|common}/oauth2/v2.0/devicecode` → verified live (probe table above)
- [x] `POST …/token` polling with `grant_type=urn:ietf:params:oauth:grant-type:device_code`
      (code handles pending/slow_down/declined/expired; integration-testable only via login)
- [x] Confirm FRT (`foci`) is returned for work **and** personal accounts — work account
      confirmed live (2026-10-27); personal still open
- [ ] Behaviour under MFA / Conditional Access / device compliance — record exact error codes
      — *needs your login*

### Per-audience token redemption
- [x] `https://api.spaces.skype.com` (chat) — FRT redemption + authz exchange live
- [ ] `https://chatsvcagg.teams.microsoft.com` (CSA roster)
- [ ] `https://substrate.office.com` (search)
- [ ] `https://graph.microsoft.com` — ⚠️ blocked via device-code for this client (see above)
- [ ] `go.trouter.io` (calls — needed by Phase 2B)
- [ ] Rotation semantics: is the FRT rotated on refresh? what invalidates it?

### authz exchange
- [x] Response shape parsed live (`region`, `regionGtms` with 112 services) — **raw body
      still uncaptured**, so the exact `expiresIn` key and full `regionGtms` key list are
      pending a proxied login
- [ ] Enterprise vs consumer response differences — *enterprise path confirmed live, consumer
      still only coded*
- [ ] Error handling: 401/403 meanings, retry expectations

### Storage & lifecycle
- [x] Cache file: `$XDG_STATE_HOME/teams-rs/token-cache.json`, mode `0600` (tested)
- [x] Refresh 300 s before expiry; on chat 401 → evict → re-mint → retry once (401-retry hook
      lands with `teams-api` in Phase 4)
- [x] `teams-cli auth login | logout | status | whoami | authz`

## Open questions

- Does device-code login work for accounts with compliant-device requirements?
- Can the cookie/WebView path be a fallback if device-code is blocked?
- Exact MS-CV / User-Agent expectations of authsvc?
- Which acquisition path yields **Graph** tokens (calendar/mail — Phase 9)?
