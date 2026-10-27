#!/usr/bin/env python3
"""Sanitize a live Teams API capture into a committable test fixture.

Ground rules (see AGENTS.md / CONTRIBUTING.md): fixtures may never contain tokens,
JWTs, emails, tenant ids, names, conversation ids, URLs or message text. This script
applies the standing policy:

  * allowlisted *enum-like* keys keep their value verbatim
    (``type``, ``messagetype``, ``mentionType``, … — extend as you prove more endpoints);
    for an array-valued key (``PropertyHits``) the *items* inherit the key, so the
    enumerated values survive while a plain string array would be redacted;
  * digit-only strings are kept (server ids, versions, sequence numbers);
  * numbers with |value| >= 1e12 are zeroed (ms-epoch timestamps);
  * arrays are truncated to ``--array-limit`` items (0 = keep all);
  * ``mentions``/``cards``/``files``/``links`` arrive *stringified* on the wire, so the
    JSON inside them is sanitized and re-stringified — the wire type survives;
  * ``ThreadActivity/*`` message bodies are XML built from ids and timestamps: the tag
    structure is kept (it is protocol surface) while every UUID and long number inside
    is zeroed;
  * everything else that is a string becomes ``<redacted>``.

Usage::

    python3 tools/sanitize_fixture.py raw.json crates/x/tests/fixtures/out.json
    python3 tools/sanitize_fixture.py raw.json -o            # to stdout

The output is idempotent: sanitizing an already-sanitized fixture changes nothing.
Review the diff before committing — this is a guard rail, not a guarantee.
"""

from __future__ import annotations

import argparse
import json
import re
import sys

# Enum-ish keys whose values carry protocol meaning and no personal data.
DEFAULT_ALLOW = {
    "@type",
    "chatType",
    "collapsed",
    "contenttype",
    "favorite",
    "formatVariant",
    "gapDetectionEnabled",
    "isemptyconversation",
    "isCreator",
    "isReader",
    "key",
    "memberRoleString",
    "mentionType",
    "messagetype",
    "messageType",
    "ongoingCallChatEnforcement",
    "postType",
    "productThreadType",
    "PropertyHits",
    "role",
    "scenario",
    "spaceType",
    "streamType",
    "threadSubType",
    "threadType",
    "type",
    "Type",
}

# Keys whose values are JSON *encoded as a string* rather than nested JSON.
STRINGIFIED_KEYS = {"cards", "files", "links", "mentions"}

# Top-level item lists: the *caller* curates how many entries to keep (a thread fixture
# needs its root message, which is not the first element), so the script never truncates
# these itself. Everything nested still obeys --array-limit.
CURATED_LIST_KEYS = {"conversations", "messages"}

UUID_RE = re.compile(r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}")
LONG_NUMBER_RE = re.compile(r"\d{12,}")
DIGITS_RE = re.compile(r"[0-9]+")
ZERO_UUID = "00000000-0000-0000-0000-000000000000"
BIG_NUMBER = 10**12

# One pass over both shapes: sequential passes break idempotence, because the zeroed UUID
# contains a 12-digit run that the number rule would then eat (`…-000000000000` → `…-0`).
XML_BODY_RE = re.compile(
    UUID_RE.pattern + "|" + LONG_NUMBER_RE.pattern,
    flags=re.IGNORECASE,
)


def sanitize_xml_body(raw: str) -> str:
    """Keep the tag structure of a `ThreadActivity/*` body, zero its identities."""
    return XML_BODY_RE.sub(lambda m: ZERO_UUID if "-" in m.group(0) else "0", raw)


# Leaves of an `Event/Call` body that carry personal data (names, e-mails, call times
# and ids a mailbox or conversation could be identified from): tags are protocol
# surface, values are not. Enum-like leaves (`callEventType`, `meetingType`, `type`)
# keep their values — they are dialect, not context.
XML_REDACT_LEAVES = ("displayName", "organizerUpn", "startTime", "endTime", "iCalUid",
                     "exchangeId", "subject", "conversationUrl", "conversationStartTime")


def sanitize_call_xml_body(raw: str) -> str:
    """Keep the tag structure of an `Event/Call` body, zero identities, redact leaves."""
    raw = sanitize_xml_body(raw)
    for leaf in XML_REDACT_LEAVES:
        raw = re.sub(
            rf"(<{leaf}>)[^<]*(</{leaf}>)", r"\1<redacted>\2", raw, flags=re.IGNORECASE
        )
    return raw


def sanitize(obj, key=None, *, allow, array_limit, in_xml_body=False, in_call_body=False):
    if isinstance(obj, dict):
        out = {}
        for k, v in obj.items():
            message_type = str(obj.get("messagetype", ""))
            is_system_body = (
                k == "content" and message_type.startswith("ThreadActivity/")
            )
            is_call_body = k == "content" and message_type == "Event/Call"
            body_mode = is_system_body or is_call_body
            nested_limit = 0 if k in CURATED_LIST_KEYS else array_limit
            out[k] = sanitize(
                v, k, allow=allow, array_limit=nested_limit,
                in_xml_body=body_mode, in_call_body=is_call_body,
            )
        return out

    if isinstance(obj, list):
        items = obj if array_limit <= 0 else obj[:array_limit]
        return [sanitize(v, key, allow=allow, array_limit=array_limit,
                         in_xml_body=in_xml_body, in_call_body=in_call_body)
                for v in items]

    if isinstance(obj, bool):
        return obj

    if isinstance(obj, str):
        if in_call_body:
            return sanitize_call_xml_body(obj)
        if in_xml_body:
            return sanitize_xml_body(obj)
        if key in allow or DIGITS_RE.fullmatch(obj):
            return obj
        if key in STRINGIFIED_KEYS:
            try:
                inner = sanitize(json.loads(obj), key, allow=allow, array_limit=array_limit)
                return json.dumps(inner, ensure_ascii=False)
            except (ValueError, TypeError):
                pass
        return "<redacted>"

    if isinstance(obj, (int, float)):
        return 0 if abs(obj) >= BIG_NUMBER else obj

    return obj


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("input", help="raw JSON capture (never commit this file)")
    parser.add_argument("output", nargs="?", help="fixture path; omit or pass - for stdout")
    parser.add_argument(
        "--array-limit", type=int, default=2, metavar="N",
        help="truncate every array to N items (0 keeps all; default 2)",
    )
    parser.add_argument(
        "--allow", action="append", default=[], metavar="KEY",
        help="additional enum-like key to keep verbatim (repeatable)",
    )
    args = parser.parse_args()

    allow = DEFAULT_ALLOW | set(args.allow)
    with open(args.input, encoding="utf-8") as fh:
        data = json.load(fh)

    clean = sanitize(data, allow=allow, array_limit=args.array_limit)

    rendered = json.dumps(clean, indent=2, ensure_ascii=False) + "\n"
    if args.output in (None, "-"):
        sys.stdout.write(rendered)
    else:
        with open(args.output, "w", encoding="utf-8") as fh:
            fh.write(rendered)
        print(f"wrote {args.output}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
