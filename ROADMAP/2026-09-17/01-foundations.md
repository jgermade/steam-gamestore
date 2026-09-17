# Phase 0 — Foundations (done)

**Gantt IDs:** `ws`, `ci0`
**Window:** 2026-09-16 → 2026-09-18
**Status:** done

## What was built

- Cargo workspace with `core`, `platform-linux`, `platform-windows` and `cli`
  (binary `gamestore`), Rust edition 2024 with `rust-version = "1.85"`.
- `core`: shared error type, `tracing` logging, configuration and per-platform
  directories, and the `Platform` trait the platform crates implement.
- The platform crate is selected by a cfg-scoped dependency, so neither can leak
  into the other's binary; `UnsupportedPlatform` keeps the workspace building
  elsewhere.
- CI: `cargo fmt --all --check`, then clippy with `-D warnings`, `cargo test` and
  `cargo build` for `x86_64-unknown-linux-gnu` on `ubuntu-latest` and
  `x86_64-pc-windows-msvc` on `windows-latest`, each on its own runner, with cargo
  and target caching.

Details, decisions and verification are in
[`RECORD/2026-09-16.workspace-and-ci-skeleton.completed.md`](../../RECORD/2026-09-16.workspace-and-ci-skeleton.completed.md).

## Carried forward

- Release publishing is still out of this phase; it belongs to `rel` in
  [`07-distribution.md`](07-distribution.md), which also adds the write permission
  CI does not have today.
- The UI phase adds a fifth workspace member (`ui`), which extends the CI matrix
  with whatever system libraries the toolkit needs on the Linux runner.
