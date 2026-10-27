# AGENTS.md

Guidance for AI agents (and humans) working in this repo.

## Project

Unofficial native Microsoft Teams client for Linux in Rust. No Electron/WebView. Talks to
undocumented Teams web-client APIs (chat/IC3, authsvc, CSA, Graph, Trouter/WebRTC). Not
affiliated with Microsoft — never claim official status or use Microsoft branding as if
endorsed.

**Status:** Phase 1 complete (workspace + CI + docs scaffold). Nothing functional yet.
The full plan lives in [`TODO.md`](TODO.md); the current phase's checklist is the source
of truth for what to build next. `docs/` contains the protocol research.

## Commands

```bash
cargo build --workspace
cargo test --workspace                # or: cargo nextest run --workspace --all-features
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo doc --workspace --no-deps       # CI uses RUSTDOCFLAGS: -D warnings
cargo deny check advisories licenses sources bans
cargo run -p teams-cli -- --help
```

CI (`.github/workflows/ci.yml`) runs all of the above with `RUSTFLAGS=-D warnings`.
Keep it green: warnings are denied workspace-wide.

## Workspace layout

```
apps/teams-gui/       iced GUI (toolkit decision pending — no deps yet)
tools/teams-cli/      headless CLI for protocol work & integration tests
crates/
  teams-protocol/     wire types + Teams HTML → RichText AST. NO I/O, no deps on other crates
  teams-auth/         device-code login, FRT cache, authz exchange → skypetoken
  teams-api/          typed HTTP clients: chat (IC3), CSA roster, substrate, Graph, ASM
  teams-realtime/     Trouter websocket (primary) + HTTP long-poll (fallback)
  teams-call/         call signalling, WebRTC media   [gated on Phase 2B GO]
  teams-store/        SQLite history cache + pending-send outbox
  teams-notify/       freedesktop notifications, tray, MPRIS, deep links
docs/                 auth, endpoints, architecture, calls research — keep in sync with code
```

## Layering rules (enforced by review, from `docs/architecture.md`)

1. Dependencies point downward only:
   `gui/cli → call/notify → realtime/api/store → auth → protocol`. No cycles.
2. **No HTTP or I/O in `teams-protocol`** — it must stay unit-testable and serde-only.
3. **The GUI never talks to the network directly** — it consumes state from core crates.
4. **Base URLs are data, not constants**: everything except `login.microsoftonline.com`
   and `authsvc.teams.microsoft.com` comes from the authz `regionGtms` map at runtime.
   Never hardcode a region or service base URL.

## Conventions

- Rust edition **2024**, MSRV **1.89** (`rust-version` in `Cargo.toml`).
- `#![forbid(unsafe_code)]` in every crate, unless a crate explicitly documents why not.
- rustfmt: `max_width = 100`, `use_small_heuristics = "Max"`, Unix newlines. Stable
  options only (`rustfmt.toml`).
- Lints: clippy `all` + `pedantic` are warnings (deny via `-D warnings`); a few
  pedantic lints are allowed project-wide in `[workspace.lints.clippy]`.
- Errors: each crate defines a `thiserror` enum (`AuthError`, `ApiError`, …); use
  `anyhow` context only at the binary boundary (`apps/`, `tools/`).
- Serde types must tolerate unknown fields (`#[serde(default)]`, ignore-unknown) — the
  API changes under us. Never fail to represent server data; degrade gracefully (e.g.
  message renders as plain text).
- Logging: `tracing` with `teams_*` targets; debug logs via `RUST_LOG=teams_*=debug`.
- Dependencies are version-pinned centrally in `[workspace.dependencies]` — add new deps
  there, not in member `Cargo.toml`s. Internal crates need both `version` and `path`
  (keeps cargo-deny's wildcard ban happy).
- Internal crate deps must respect the layering rules above.

## Ground rules (from CONTRIBUTING.md)

- **Never commit secrets**: no tokens, JWTs, cookies, or captures with personal data.
  Traces go in `docs/traces/` (gitignored); sanitize before quoting in `docs/*.md`.
- New endpoints require **both**: a note in `docs/endpoints.md` (method, URL
  construction, auth header, caveats) **and** a sanitized fixture under
  `crates/<crate>/tests/fixtures/` with a deserialization test.
- HTTP calls live in the relevant client crate, never in the GUI.
- Commit style (Conventional-Commits-flavored, matching existing history):
  - Subject: `<scope>: <imperative summary>`, scope = crate or area
    (`auth`, `api`, `realtime`, `gui`, `ci`, `docs`, `chore`, …), e.g.
    `auth: add device-code polling with backoff`.
  - Subject ≤ 50–72 chars, no trailing period, not "added"/"adds".
  - Body: wrap at ~72 chars, explain **what and why** (protocol findings,
    trade-offs, links to `TODO.md`/`docs/` items). Omit if the diff is
    self-explanatory.
  - One logical change per commit/PR; use `refactor:`/`chore:` for
    no-behaviour-change work; break WIP into `…: wip` fixups before review.

## Docs to update as you work

- `TODO.md` — check off phase items as they land; it is the plan.
- `docs/endpoints.md` — every endpoint, as it is implemented.
- `docs/auth.md`, `docs/calls.md` — auth and call-protocol findings.
- `docs/architecture.md` — layering / data-flow changes.
