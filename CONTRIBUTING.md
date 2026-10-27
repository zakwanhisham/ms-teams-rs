# Contributing to ms-teams-rs

Thanks for helping! This project reverse-engineers undocumented APIs, so **research notes
are as valuable as code**.

## Ground rules

1. **Never commit secrets.** No tokens, JWTs, cookies, or captures containing personal data.
   Traces belong in `docs/traces/`, which is gitignored — sanitize before quoting them in
   `docs/*.md` (drop tokens, emails, tenant ids, real names).
2. **No claims of official status.** Do not use Microsoft/Teams branding as if endorsed.
3. Everything must stay `#![forbid(unsafe_code)]` unless a crate explicitly documents why
   not (e.g. a future FFI media binding).

## Development workflow

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo nextest run --workspace --all-features   # or: cargo test --workspace
cargo doc --workspace --no-deps
```

CI runs all of the above plus `cargo deny check` (advisories, licenses, sources, bans).
Please keep it green.

- Commit style: imperative mood, scoped, e.g. `auth: add device-code polling with backoff`.
- One logical change per PR; include the reasoning for protocol findings in the PR body.
- New endpoints require: a note in `docs/endpoints.md` **and** a sanitized fixture under
  `crates/<crate>/tests/fixtures/`.

## Adding an endpoint (checklist)

- [ ] Documented in `docs/endpoints.md` (method, URL construction, auth header, caveats)
- [ ] Types live in `teams-protocol` with `#[serde(default)]` + unknown-field tolerance
- [ ] HTTP call lives in the relevant client crate, never in the GUI
- [ ] Fixture-based deserialization test
- [ ] Regions/base URLs come from the authz `regionGtms` map — never hardcoded

## Reporting problems

Attach: `teams-cli --version`, your distro/desktop (GNOME/KDE/Sway, Wayland/X11), whether
the failure is auth/chat/calls, and sanitized logs (`RUST_LOG=teams_*=debug`). Do **not**
paste tokens.
