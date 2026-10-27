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
| Credential store backends behind `CredentialStore` (file + OS keyring) | ✅ | `store.rs`; selected by `TEAMS_RS_CREDENTIAL_STORE`, default `file` — see below |
| authz exchange (both enterprise & consumer response shapes) | ✅ | `authz.rs`; live exchange succeeded, `regionGtms` map has 112 services |
| Claim decoding (unverified, display only) | ✅ | `jwt.rs` |
| Per-audience mint (`TokenCache::fresh_or_redeem`) | ✅ | picks a cached FRT (audience's own → spaces → any), redeems it, caches under the new label; short-circuits without network while the cached token is fresh — covered by tests |
| CLI: `teams-cli auth login/status/authz/mint/whoami/logout` | ✅ | never prints token values |

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
- **Second audience minted live (2026-10-27)**: the family refresh token redeems directly
  for `openid offline_access https://chatsvcagg.teams.microsoft.com/.default` →
  `aud=https://chatsvcagg.teams.microsoft.com`, `scp=user_impersonation`. That bearer is what
  `teams_api::RosterClient` sends (`teams-cli auth mint` performs the redemption).

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

### Credential store backends (decided 2026-10-28)

The serialized cache (one JSON document) is written through the object-safe
`CredentialStore` trait (`crates/teams-auth/src/store.rs`): `read`/`write`/`remove`/
`describe`. Selection is `TEAMS_RS_CREDENTIAL_STORE`:

| Value | Backend | Notes |
|---|---|---|
| `file` (default, also unset/empty) | `FileStore` — one `0600` file at `TokenCache::path()` | unchanged behaviour |
| `keyring` | `KeyringStore` — Secret Service item `service=teams-rs`, `username=token-cache` | Linux only (`org.freedesktop.secrets`: GNOME Keyring, KWallet, `oo7-daemon`) |

Both `teams-cli auth …` and the 401 refreshers (`AuthzRefresher` / `AudienceRefresher`)
resolve the store through the same `store_for` / `default_store` functions, so a request
never reads one backend while its recovery writes another. Switching the variable is
**not a migration** — the other backend stays empty until the next `teams-cli auth login`.

**Why not the kernel keyrings (keyutils)?** Measured and rejected:

- `/proc/sys/kernel/keys/maxbytes` = **20 000** bytes, cumulative per user; the real cache
  is **30 320 bytes** (3 audiences + authz session) and `add_key` fails with *Disk quota
  exceeded* (EDQUOT) — even a second 15 KB key overflows the quota.
- Kernel keyrings are cleared on reboot; the `0600` file already survives it.

So the OS keyring backend is the **Secret Service**: no quota, persists, standard desktop
keystore. Stack: `keyring-core` + `dbus-secret-service-keyring-store` (`crypto-rust`
session encryption, `vendored` libdbus → no system build dependencies). KWallet's wrapper
stores UTF-8 strings only and the payload is JSON, so it round-trips. WSL has no default
collection → use the file store there.

Tests skip gracefully when no daemon is reachable, except in CI where
`TEAMS_RS_REQUIRE_KEYRING=1` turns a missing Secret Service into a failure (the test job
installs `gnome-keyring` and unlocks it with the recipe from keyring-rs's CI).

**Verified live 2026-10-28** against `oo7-daemon` as the local Secret Service: the real
30 320-byte cache was written into the production keyring entry through the store API,
`TEAMS_RS_CREDENTIAL_STORE=keyring teams-cli auth status` read the whole document back
(identity, three audiences, `region=apac` + 112 `regionGtms` services), and
`auth logout` deleted it (a second attempt reports nothing to remove); the `0600` file
was untouched and stays the default backend. (Caveat if you script this: `secret-tool`
caps its stdin at 8 KiB — "password is too long" — so it truncates this cache; the store
API does not.) The store tests pass against that daemon with
`TEAMS_RS_REQUIRE_KEYRING=1` (40 KB round-trip + concurrent access) and **fail rather
than skip** when the variable is set but no Secret Service answers — verified both ways.

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
- [x] `https://chatsvcagg.teams.microsoft.com` (CSA roster) — FRT redemption live
      (2026-10-27), `scp=user_impersonation`; consumed by `teams-cli roster`
- [x] `https://substrate.office.com` (search) — FRT redemption live (2026-10-27); the minted
      token carries `SubstrateSearch-Internal.ReadWrite`; consumed by `teams-cli search`
- [ ] `https://graph.microsoft.com` — ⚠️ blocked via device-code for this client (see above)
- [ ] `go.trouter.io` (calls — needed by Phase 2B)
- [ ] Rotation semantics: is the FRT rotated on refresh? what invalidates it?

### authz exchange
- [x] Response shape parsed live (`region`, `regionGtms` with 112 services) — **raw body
      still uncaptured**, so the exact `expiresIn` key and full `regionGtms` key list are
      pending a proxied login
- [ ] Enterprise vs consumer response differences — *enterprise path confirmed live, consumer
      still only coded*
- [~] Error handling: 401/403 meanings, retry expectations — **401 observed live** on the
      chat service as `{"errorCode":911,"message":"Authentication failed."}` (and plain
      401s from CSA/substrate) and now recovered automatically (evict → re-mint → replay
      once, live-verified 2026-10-27); retry expectations codified in `docs/endpoints.md`
      (backoff only on 429/502/503/504). 403 meanings still unobserved.

### Storage & lifecycle
- [x] Cache file: `$XDG_STATE_HOME/teams-rs/token-cache.json`, mode `0600` (tested)
- [x] `CredentialStore` trait with the file backend (default) and an OS keyring backend
      (`TEAMS_RS_CREDENTIAL_STORE=keyring`, Secret Service) — keyutils rejected on
      measurement (20 KB quota vs a 30 KB cache), decision recorded under
      *Credential store backends* above
- [x] Refresh 300 s before expiry; on 401 → evict → re-mint → retry once — **landed and
      live-verified 2026-10-27**: a poisoned skypetoken recovered via
      `teams_api::AuthzRefresher` (evict spaces → FRT redemption → authz exchange) and a
      poisoned-but-locally-fresh CSA bearer via `teams_api::AudienceRefresher`
      (`TokenCache::evict` keeps the FRT), each replaying exactly once
- [x] `teams-cli auth login | logout | status | whoami | authz`

## Open questions

- Does device-code login work for accounts with compliant-device requirements?
- Can the cookie/WebView path be a fallback if device-code is blocked?
- Exact MS-CV / User-Agent expectations of authsvc?
- Which acquisition path yields **Graph** tokens (calendar/mail — Phase 9)?
