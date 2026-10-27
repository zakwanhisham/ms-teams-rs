# Authentication

> Research document — Phase 2A findings go here. Plan: [`../TODO.md`](../TODO.md).
> **Never paste real tokens, tenant ids or account names into this file.**

## Goal

Obtain and continuously refresh a **skypetoken** plus the service region map, which gates
every Teams API call.

## Flow (hypothesis — confirm in the spike)

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

## Checklist

### Device-code login (FOCI client `1fec8e78-bce4-4aaf-ab1b-5451cc387264`)
- [ ] `POST /{tenant|common}/oauth2/v2.0/devicecode` → `device_code`, `user_code`, `verification_uri`, interval
- [ ] `POST /{tenant|common}/oauth2/v2.0/token` polling with `grant_type=urn:ietf:params:oauth:grant-type:device_code`
- [ ] Confirm FRT (`foci`) is returned for work **and** personal accounts
- [ ] Behaviour under MFA / Conditional Access / device compliance — record exact error codes

### Per-audience token redemption
- [ ] `https://api.spaces.skype.com` (chat)
- [ ] `https://chatsvcagg.teams.microsoft.com` (CSA roster)
- [ ] `https://substrate.office.com` (search)
- [ ] `https://graph.microsoft.com` (calendar/mail; scopes?)
- [ ] `go.trouter.io` (calls — needed by Phase 2B)
- [ ] Rotation semantics: is the FRT rotated on refresh? what invalidates it?

### authz exchange
- [ ] `POST https://authsvc.teams.microsoft.com/v1.0/authz`, empty JSON body, `Authorization: Bearer`
- [ ] Response shape: `tokens.skypeToken`, `expiresIn`, `region`, `regionGtms` (record full key list)
- [ ] Enterprise vs consumer response differences
- [ ] Error handling: 401/403 meanings, retry expectations

### Storage & lifecycle
- [ ] Cache file: `$XDG_STATE_HOME/teams-rs/token-cache.json`, mode `0600`
- [ ] Refresh ~5 min before expiry; on chat 401 → evict session → re-mint → retry once
- [ ] `teams-cli auth login | logout | status | whoami`

## Findings

_(fill in during Phase 2A)_

## Open questions

- Does device-code login work for accounts with compliant-device requirements?
- Can the cookie/WebView path be a fallback if device-code is blocked?
- Exact MS-CV / User-Agent expectations of authsvc?
