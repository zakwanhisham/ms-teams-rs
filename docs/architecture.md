# Architecture

> Living document — updated as phases land. Plan: [`../TODO.md`](../TODO.md).

## Overview

```
┌──────────────────────────────────────────────────────────────────────┐
│ apps/teams-gui (iced)                    tools/teams-cli (headless) │
└───────────────┬──────────────────────────────────┬───────────────────┘
                │                                  │
   ┌────────────▼────────────┐        ┌────────────▼────────────┐
   │ teams-call              │        │ teams-notify            │
   │  signalling + WebRTC    │        │  notifications, tray,   │
   │  [gated: Phase 2B GO]   │        │  MPRIS, deep links      │
   └────────────┬────────────┘        └─────────────────────────┘
                │
   ┌────────────▼────────────────────────────────────────────┐
   │ teams-realtime   Trouter (primary) / long-poll (backup) │
   └────────────┬────────────────────────────────────────────┘
                │
   ┌────────────▼─────────────┐     ┌────────────────────────┐
   │ teams-api                │◄────┤ teams-auth             │
   │  chat / CSA / substrate  │     │  device-code, FRT,     │
   │  Graph / ASM             │     │  authz → skypetoken    │
   └────────────┬─────────────┘     └────────────────────────┘
                │
   ┌────────────▼─────────────┐     ┌────────────────────────┐
   │ teams-protocol (types,   │     │ teams-store (SQLite:   │
   │  RichText AST, no I/O)   │     │  cache, outbox, cursor)│
   └──────────────────────────┘     └────────────────────────┘
```

## Layering rules

1. **Dependencies point downward only**: `gui/cli → call/notify → realtime/api/store →
   auth → protocol`. No cycles; `teams-protocol` depends on nothing internal.
2. **No HTTP or I/O in `teams-protocol`.** It must stay unit-testable and serde-only.
3. **The GUI never talks to the network directly** — it consumes state from the core crates,
   which keeps a toolkit swap survivable.
4. **Base URLs are data, not constants**: everything except the fixed Microsoft entry points
   (`login.microsoftonline.com`, `authsvc.teams.microsoft.com`) comes from the authz
   `regionGtms` map at runtime.

## Concurrency model

- Single `tokio` multi-thread runtime in both binaries.
- One task per: realtime transport, outbox flusher, auth refresher, per-call media pipeline.
- UI state changes flow over `iced::Subscription` / channels; heavy work never happens on
  the UI thread (message rendering, HTML parsing, image decode included).
- `teams-store` is accessed through a single writer task (`tokio::sync::mpsc`) to avoid
  SQLite lock contention.

## Data flow: incoming message

```
Trouter frame → teams-realtime decode → TeamEvent::MessageCreated
   → teams-store upsert (transactional, sets sync_cursor)
   → event bus → gui (append to message list + notification if window unfocused/unread)
```

## Data flow: outgoing message

```
composer submit → teams-store insert as pending (outbox) → optimistic UI
   → outbox task: teams-api::send → on success: mark acked with server id
   → on failure (401): teams-auth refresh + retry once; other errors: retry with backoff
```

## Calls (Phase 8, gated)

`teams-call` reuses the Trouter connection from `teams-realtime` for signalling
(ring/invite/SDP/ICE/hangup) and owns the WebRTC peer connection + media pipelines.
Media state is mirrored into the UI as a small `CallState`; video frames reach the GUI as
GPU textures (validate this against the chosen toolkit in Phase 2B).

## Error handling strategy

- Each crate defines a `thiserror` enum (`AuthError`, `ApiError`, `RealtimeError`,
  `CallError`); contexts are added with `anyhow` only at the binary boundary.
- A 401 anywhere triggers exactly one auth refresh + retry, then surfaces a re-login prompt.
- Nothing panics on malformed server data: decode errors are logged, fixtures updated,
  and the affected item degrades (e.g. message renders as plain text).
