"""mitmproxy addon: sanitized capture of Microsoft Teams API traffic.

Purpose: record request/response metadata for the Phase 2 protocol spikes (TODO.md
Phase 2A/2B) into JSONL files under ``docs/traces/`` (gitignored), with JWTs and
credential-like headers/parameters scrubbed so nothing secret can leak into docs.

Usage:
    mitmdump -s tools/capture/teams_trace.py --set trace_dir=docs/traces -p 8080

Then point a browser / the Teams client at the proxy and perform the scenario
(see tools/capture/README.md). Response bodies are truncated to 64 KiB.
"""

from __future__ import annotations

import json
import os
import re
import time
from urllib.parse import parse_qsl, urlencode, urlsplit, urlunsplit

from mitmproxy import http

# Hosts worth recording: Teams web/app API surface, identity, authz, chat (IC3),
# Trouter/registrar (calls), ASM (files), substrate (search), Graph.
HOST_MARKERS = (
    "teams.microsoft.com",
    "teams.live.com",
    "msg.teams.microsoft.com",
    "authsvc.teams.microsoft.com",
    "authsvc-server.teams.microsoft.com",
    "login.microsoftonline.com",
    "login.live.com",
    "go.trouter.io",
    "trouter.io",
    "prod.registrar.skype.com",
    "api.spaces.skype.com",
    "api.asm.skype.com",
    "substrate.office.com",
    "graph.microsoft.com",
    "presence.teams.microsoft.com",
    "config.teams.microsoft.com",
    "vc.trouter.io",
)

# Headers whose values are credentials or session material.
SECRET_HEADERS = {
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
    "authentication",
    "x-ms-token",
    "x-ms-token-aad-access-token",
    "x-ms-token-aad-refresh-token",
    "x-skypetoken",
}

# Query/form parameter names that carry secrets or user codes.
SECRET_PARAMS = {
    "access_token",
    "refresh_token",
    "id_token",
    "code",
    "device_code",
    "user_code",
    "assertion",
    "skypetoken",
    "token",
    "sso_nonce",
}

# JWTs are self-contained (claims inside) — never safe to paste anywhere.
JWT_RE = re.compile(r"eyJ[A-Za-z0-9_-]{4,}\.[A-Za-z0-9_-]{4,}\.[A-Za-z0-9_-]{4,}")

MAX_BODY = 64 * 1024


def scrub_text(text: str) -> str:
    """Replace JWT-looking substrings with a placeholder."""
    return JWT_RE.sub("<redacted-jwt>", text)


def scrub_headers(headers: http.Headers) -> dict[str, str]:
    out: dict[str, str] = {}
    for name, value in headers.items(multi=True):
        key = name.lower()
        if key in SECRET_HEADERS:
            out[name] = f"<redacted ({len(value)} chars)>"
        else:
            out[name] = scrub_text(value)
    return out


def scrub_url(url: str) -> str:
    parts = urlsplit(url)
    query = []
    for key, value in parse_qsl(parts.query, keep_blank_values=True):
        if key.lower() in SECRET_PARAMS:
            query.append((key, "<redacted>"))
        else:
            query.append((key, scrub_text(value)))
    return urlunsplit((parts.scheme, parts.netloc, parts.path, urlencode(query), parts.fragment))


def body_text(content: bytes | None) -> str:
    if not content:
        return ""
    if len(content) > MAX_BODY:
        content = content[:MAX_BODY]
        suffix = f"\n… <truncated, {len(content)}+ bytes shown of more>"
    else:
        suffix = ""
    try:
        text = content.decode("utf-8")
    except UnicodeDecodeError:
        return f"<binary body, {len(content)} bytes>{suffix}"
    try:
        parsed = json.loads(text)
        return scrub_text(json.dumps(parsed, indent=2, ensure_ascii=False)) + suffix
    except (json.JSONDecodeError, ValueError):
        return scrub_text(text) + suffix


class TeamsTrace:
    """Append sanitized HTTP records to a JSONL file."""

    def __init__(self) -> None:
        self.trace_dir = "docs/traces"
        self.path: str | None = None
        # in-flight websocket traces, keyed by flow id
        self._ws_traces: dict[str, dict] = {}

    def load(self, loader) -> None:  # noqa: ANN001 - mitmproxy Loader type
        loader.add_option(
            "trace_dir",
            str,
            "docs/traces",
            "Directory for sanitized Teams traffic traces (JSONL)",
        )

    def configure(self, updates) -> None:  # noqa: ANN001 - mitmproxy updates type
        from mitmproxy import ctx

        self.trace_dir = ctx.options.trace_dir
        if self.path is None:
            os.makedirs(self.trace_dir, exist_ok=True)
            stamp = time.strftime("%Y%m%d-%H%M%S")
            self.path = os.path.join(self.trace_dir, f"teams-{stamp}.jsonl")

    def _interesting(self, host: str) -> bool:
        return any(marker in host for marker in HOST_MARKERS)

    # --- websocket recording (Phase 2B stage 0) -------------------------------

    def websocket_start(self, flow: http.HTTPFlow) -> None:
        """Called when a websocket connection commences; record the handshake."""

        if not self._interesting(flow.request.pretty_host) or flow.response is None:
            return
        scrubbed = scrub_url(flow.request.pretty_url)
        if scrubbed.startswith(("http://", "https://")):
            scrubbed = "ws" + scrubbed[4:]  # ws:// or wss://
        self._ws_traces[flow.id] = {
            "ts": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            # The handshake is an HTTP GET; recorded as ws(s):// for readability.
            "url": scrubbed,
            "status": flow.response.status_code,
            "req_headers": scrub_headers(flow.request.headers),
            "resp_headers": scrub_headers(flow.response.headers),
            "ws_key_accept": flow.response.headers.get("sec-websocket-accept", ""),
            "subprotocol": flow.response.headers.get("sec-websocket-protocol", ""),
            "frames": [],
        }

    def websocket_message(self, flow: http.HTTPFlow) -> None:
        """Buffer one frame; flushed by websocket_end."""
        trace = self._ws_traces.get(flow.id)
        if trace is None or flow.websocket is None:
            return
        message = flow.websocket.messages[-1]
        # mitmproxy reassembles fragments; `type` is a wsproto Opcode enum.
        frame_type = message.type.name.lower()
        frame: dict[str, object] = {
            "ts": time.strftime("%H:%M:%SZ", time.gmtime()),
            "dir": "c→s" if message.from_client else "s→c",
            "type": frame_type,
        }
        if frame_type == "text":
            text = message.content.decode("utf-8", "replace")
            frame["text"] = scrub_text(text[:MAX_BODY]) + (
                f" <truncated, {len(text)} chars>" if len(text) > MAX_BODY else ""
            )
        else:
            # Binary frames are media/SRTP-ish or opaque framing: record size only.
            frame["bytes"] = len(message.content)
        trace["frames"].append(frame)

    def websocket_end(self, flow: http.HTTPFlow) -> None:
        """Flush the connection's handshake + frames as one JSONL record."""
        trace = self._ws_traces.pop(flow.id, None)
        if trace is None:
            return
        # Summary describes shape: frame counts, not media content.
        counts: dict[str, int] = {}
        for frame in trace["frames"]:
            counts[frame["type"]] = counts.get(frame["type"], 0) + 1  # type: ignore[index]
        record = {"kind": "websocket", "summary": counts, **trace}
        try:
            with open(self.path, "a", encoding="utf-8") as handle:
                handle.write(json.dumps(record, ensure_ascii=False) + "\n")
        except OSError as err:  # never break proxying because of tracing
            print(f"[teams_trace] write failed: {err}")

    def response(self, flow: http.HTTPFlow) -> None:
        if not self._interesting(flow.request.pretty_host):
            return
        record = {
            "ts": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            "method": flow.request.method,
            "url": scrub_url(flow.request.pretty_url),
            "status": flow.response.status_code if flow.response else None,
            "req_headers": scrub_headers(flow.request.headers),
            "req_body": body_text(flow.request.content),
            "resp_headers": scrub_headers(flow.response.headers) if flow.response else {},
            "resp_body": body_text(flow.response.content) if flow.response else "",
        }
        try:
            with open(self.path, "a", encoding="utf-8") as handle:
                handle.write(json.dumps(record, ensure_ascii=False) + "\n")
        except OSError as err:  # never break proxying because of tracing
            print(f"[teams_trace] write failed: {err}")


addons = [TeamsTrace()]
