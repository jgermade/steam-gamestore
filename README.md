# steam-gamestore

A GOG installer that lives inside the Steam library. Games bought on GOG are
downloaded, installed silently, and added as ordinary Steam tiles, so they launch
from Big Picture through the Proton that Steam already manages — with saves
restored before play and backed up afterwards by Ludusavi.

Design notes: [`docs/gog-installer.md`](docs/gog-installer.md).
Plan: [`ROADMAP/2026-09-16/00-overview.md`](ROADMAP/2026-09-16/00-overview.md).

## Status

Phase 0 of the roadmap: the workspace compiles for Linux and Windows and the CLI
reports what it resolved on the machine. Authentication, catalog and downloads
are not implemented yet.

## Build and run

```sh
cargo build --workspace
cargo run -p gamestore-cli -- info
cargo run -p gamestore-cli -- config
```

Checks, the same three CI runs:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Workspace

| Crate | Contents |
|-------|----------|
| `core/` | Configuration, logging, the shared error type, the `Platform` trait. Everything platform-agnostic. |
| `platform-linux/` | Steam and Proton integration: shortcuts, compat tool mapping, launch wrappers. |
| `platform-windows/` | Native install and launch, no Proton layer. |
| `cli/` | The `gamestore` binary; picks its platform crate at compile time. |

Directories follow the platform conventions — `~/.config/gamestore` and
`~/.local/share/gamestore` on Linux, `%APPDATA%\gamestore` on Windows — and
`GAMESTORE_CONFIG_DIR`, `GAMESTORE_DATA_DIR` and `GAMESTORE_CACHE_DIR` override
them. `STEAM_ROOT` points at a Steam installation the platform crates cannot
guess, and `GAMESTORE_LOG` sets the log filter (for example `GAMESTORE_LOG=debug`).

## Contributing

Read [`AGENTS.md`](AGENTS.md) first: it defines how the roadmap and the
append-only change records under [`RECORD/`](RECORD) are kept.
