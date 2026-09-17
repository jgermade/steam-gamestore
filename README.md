# steam-gamestore

A GOG installer that lives inside the Steam library. Games bought on GOG are
downloaded, installed silently, and added as ordinary Steam tiles, so they launch
from Big Picture through the Proton that Steam already manages — with saves
restored before play and backed up afterwards by Ludusavi.

Design notes: [`docs/gog-installer.md`](docs/gog-installer.md).
Plan: [`ROADMAP/2026-09-17/00-overview.md`](ROADMAP/2026-09-17/00-overview.md).

## Status

Phase 0 of the roadmap is done: the workspace compiles for Linux and Windows and
the CLI reports what it resolved on the machine. The GOG OAuth2 flow (`auth`) is
implemented and the session now survives between runs (`tok`), so `gamestore
login` is followed by `gamestore logout` and an `info` that says who is logged in.
Both still wait on a run against GOG itself from the mini PC. Catalog, downloads
and the controller UI are not started.

## Build and run

```sh
cargo build --workspace
cargo run -p gamestore-cli -- info
cargo run -p gamestore-cli -- config
cargo run -p gamestore-cli -- login
cargo run -p gamestore-cli -- logout
```

Checks, the same three CI runs:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Logging in to GOG

`gamestore login` prints a GOG authorization URL, you log in in any browser, and
you paste back the address you land on (it carries `code=…`); the code→token
exchange happens locally, so the client secret never leaves the machine. GOG
publishes no device-code flow and the redirect is fixed on their side, which is
why the paste step exists — see
[`RECORD/2026-09-16.gog-login-flow-decision.completed.md`](RECORD/2026-09-16.gog-login-flow-decision.completed.md).

The OAuth2 client credentials are supplied by you, either in `config.toml`:

```toml
[auth]
client_id = "…"
client_secret = "…"
```

or in the environment, which wins over the file:

```sh
export GOG_CLIENT_ID=… GOG_CLIENT_SECRET=…
```

Whether releases should ship the well-known GOG Galaxy credentials instead is
still open (question 3 in the roadmap).

## Where the session is kept

`gamestore login` stores the tokens and refreshes them by itself; GOG rotates the
refresh token, and what comes back replaces what went in. `gamestore logout`
forgets them, and `gamestore info` prints where they are.

The platform keyring is used when there is one — Secret Service on Linux, the
credential manager on Windows. When there is not, gamestore falls back to a file
under the data directory, `~/.local/share/gamestore/sessions/gog.json`, created
`0600`:

```
gog tokens:   file (~/.local/share/gamestore/sessions/gog.json, unencrypted, 0600)
              the platform keyring is not usable: No default store has been set …
```

**That file is not encrypted, and anyone who can read it can use the session.** It
is written that way on purpose. Encrypting it would need a key the same machine
can read unattended, which is obfuscation rather than secrecy; `0600` gives the
same real protection without claiming more. On a machine booted straight into
gamescope there is no D-Bus session and therefore no keyring, so this is the
ordinary path there, not a corner case — which is why `info` and `login` say so
rather than falling back quietly.

`GAMESTORE_ACCOUNT` names the session, so two Steam users sharing a machine keep
separate GOG logins (`GAMESTORE_DATA_DIR` separates their files, and the account
name separates their keyring entries, which are per-machine).

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
guess, `GOG_CLIENT_ID` and `GOG_CLIENT_SECRET` carry the OAuth2 credentials, and
`GAMESTORE_ACCOUNT` names the stored GOG session, and `GAMESTORE_LOG` sets the log
filter (for example `GAMESTORE_LOG=debug`).

## Contributing

Read [`AGENTS.md`](AGENTS.md) first: it defines how the roadmap and the
append-only change records under [`RECORD/`](RECORD) are kept.
