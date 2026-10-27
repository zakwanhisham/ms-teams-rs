# Traffic capture for the Phase 2 protocol spikes

Records sanitized Teams API traffic into `docs/traces/*.jsonl` (**gitignored**) so findings
can be transcribed into `docs/auth.md`, `docs/endpoints.md` and `docs/calls.md` without ever
committing tokens or personal data. The addon scrubs JWTs, `Authorization`/`Cookie`/
`Authentication` headers, and credential-like query/form parameters.

> Never paste raw trace contents into docs. Quote **structure** (field names, message types,
> status codes) only — see `CONTRIBUTING.md`.

## Setup

```bash
python3 -m venv .venv-capture
.venv-capture/bin/pip install mitmproxy

# terminal 1 — proxy with the addon
.venv-capture/bin/mitmdump -s tools/capture/teams_trace.py --set trace_dir=docs/traces -p 8080

# terminal 2 — point the client at it
export https_proxy=http://127.0.0.1:8080 http_proxy=http://127.0.0.1:8080
```

For a GUI client (browser / Teams) you must install the mitmproxy CA certificate first:
`~/.mitmproxy/mitmproxy-ca-cert.pem` — follow <https://docs.mitmproxy.org/stable/concepts-certificates>.
GNOME: `certutil -d sql:$HOME/.pki/nssdb -A -t "C,," -n mitmproxy -i ~/.mitmproxy/mitmproxy-ca-cert.pem`.

**Check whether your org's Conditional Access blocks proxied logins before starting** — if
it does, fall back to browser devtools (Network tab → export HAR) and redact manually.

## Scenarios to capture

### Phase 2A (auth/chat) — also produced implicitly by `teams-cli auth login` through the proxy
- [ ] device-code request + token poll (`login.microsoftonline.com`)
- [ ] `POST authsvc.teams.microsoft.com/v1.0/authz` response (**shape of `tokens`, `region`,
      `regionGtms`** — this is the key unknown)
- [ ] chat service calls: conversations list, messages list

### Phase 2B (calls — the go/no-go gate, see `docs/calls.md`)
- [ ] Trouter websocket handshake + frames (`go.trouter.io`) — use `mitmdump -w` or the
      websocket events; note the auth material used
- [ ] registrar registration (`prod.registrar.skype.com`)
- [ ] 1:1 call: invite → ring → accept → SDP offer/answer → ICE → hangup
- [ ] incoming call
- [ ] scheduled meeting join, incl. lobby wait and admission
- [ ] screen share start/stop
- [ ] meeting leave

## After capturing

1. Locate `docs/traces/teams-<stamp>.jsonl`.
2. Grep for the endpoint of interest, e.g.:
   ```bash
   grep -o '"url": "[^"]*authz[^"]*"' docs/traces/*.jsonl
   ```
3. Transcribe **structure only** into `docs/*.md`, and add a sanitized fixture +
   deserialization test under `crates/<crate>/tests/fixtures/` (AGENTS.md rules).
4. Keep raw traces out of git (`.gitignore` covers `/docs/traces/`).

## Verifying the redaction before sharing anything

```bash
grep -c 'eyJ' docs/traces/*.jsonl        # JWTs should only appear as <redacted-jwt>
grep -Ei 'authorization|cookie' docs/traces/*.jsonl | grep -v redacted   # expect none
```
