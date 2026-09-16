# Phase 0 — Foundations

**Gantt IDs:** `ws`, `ci0`
**Window:** 2026-09-16 → 2026-09-25
**Depends on:** nothing

## Goal

A Rust workspace that compiles on both targets from day one, so that no
platform-specific decision is discovered late.

## Tasks

### `ws` — Rust workspace skeleton (4d)

- [ ] Cargo workspace with three members:
  - `core/` — auth, catalog, download, Ludusavi, everything platform-agnostic.
  - `platform-linux/` — Steam/Proton integration, launch wrapper generation.
  - `platform-windows/` — native install and launch.
- [ ] A thin binary crate (`cli/`) that wires the platform crate selected at
      compile time via `#[cfg(target_os)]`.
- [ ] Shared error type and logging setup in `core`.
- [ ] Config file location resolved per platform (XDG on Linux, `%APPDATA%` on
      Windows).
- [ ] `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test` all green on an
      empty workspace.

### `ci0` — CI build matrix skeleton (3d)

- [ ] GitHub Actions workflow building `x86_64-unknown-linux-gnu` and
      `x86_64-pc-windows-msvc`.
- [ ] Format, clippy and test jobs on every push.
- [ ] Cargo registry and target caching, so later phases do not wait on CI.

Release publishing is not part of this task; it belongs to
[`06-distribution.md`](06-distribution.md).

## Acceptance criteria

- `cargo build --workspace` succeeds on Linux and on Windows in CI.
- `platform-windows` code never leaks into a Linux build and vice versa.

## Risks

- Splitting the workspace too early can force awkward abstractions. Keep
  `platform-*` crates thin: they own only what genuinely differs.
