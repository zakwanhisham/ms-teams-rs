# ms-teams-rs

**Unofficial native Microsoft Teams client for Linux, written in Rust.** No Electron, no
WebView — chat, calls and meetings through the same (undocumented) APIs the Teams web
client uses, with real Linux desktop integration (notifications, tray, deep links).

> [!WARNING]
> This project is **not affiliated with, endorsed by, or supported by Microsoft**. It talks
> to undocumented APIs that can change or break at any time, and its use may conflict with
> your organization's terms or Conditional Access policies. Use at your own risk, on accounts
> you are allowed to use this way.

## Status

Early scaffolding — **Phase 1 of [TODO.md](TODO.md)** complete: workspace, CI, docs scaffold.
Nothing is functional yet. The plan (with go/no-go checkpoints) lives in [`TODO.md`](TODO.md).

| Phase | Milestone | Status |
|------:|-----------|--------|
| 1 | Workspace + CI + docs scaffold | ✅ |
| 2 | Protocol spike (auth/chat **and** calls → GO/NO-GO) | ⬜ |
| 3 | `teams-auth` — device-code login, token cache, authz | ⬜ |
| 4 | `teams-protocol` + `teams-api` — read path | ⬜ |
| 5 | `teams-realtime` + write path | ⬜ |
| 6 | GUI chat client | ⬜ |
| 7 | Desktop integration (notifications, tray, packaging) | ⬜ |
| 8 | Calls & meetings (1:1 → group → meetings) | ⬜ |
| 9 | Graph extras (calendar, mail, uploads, search) | ⬜ |

## Workspace layout

```
apps/teams-gui/       native desktop client
tools/teams-cli/      headless CLI for protocol work & integration tests
crates/
  teams-protocol/     wire types + Teams HTML → RichText AST (no I/O)
  teams-auth/         device-code login, FRT cache, authsvc authz exchange
  teams-api/          typed HTTP clients: chat (IC3), CSA roster, substrate, Graph, ASM
  teams-realtime/     Trouter websocket (primary) + HTTP long-poll (fallback)
  teams-call/         call signalling, WebRTC media, device capture   [gated on Phase 2B]
  teams-store/        SQLite history cache + pending-send outbox
  teams-notify/       freedesktop notifications, tray, MPRIS, deep links
docs/                 auth, endpoints, architecture and calls research (the real treasure)
```

## Building

Requires a current stable Rust (see `rust-version` in `Cargo.toml`).

```bash
cargo build --workspace          # build everything
cargo test --workspace           # run tests
cargo run -p teams-cli -- --help # headless CLI
cargo run -p teams-gui           # GUI (placeholder for now)
```

Tooling used by CI (optional locally):

```bash
cargo install cargo-nextest cargo-deny
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check
```

## Documentation

- [`TODO.md`](TODO.md) — the full plan, milestones and risk register
- [`docs/architecture.md`](docs/architecture.md) — component design
- [`docs/auth.md`](docs/auth.md) — authentication findings
- [`docs/endpoints.md`](docs/endpoints.md) — endpoint reference
- [`docs/calls.md`](docs/calls.md) — call/meeting protocol research (Phase 2B gate)

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
