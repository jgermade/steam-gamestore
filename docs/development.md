# Development

## Build and run

```sh
cargo build --workspace
cargo run -p gamestore-cli -- info
cargo run -p gamestore-cli -- config
cargo run -p gamestore-cli -- login
cargo run -p gamestore-cli -- logout
cargo run -p gamestore-cli -- library
cargo run -p gamestore-cli -- appid --exe /games/Bastion/Bastion.exe --name Bastion
cargo run -p gamestore-cli -- steam-install
cargo run -p gamestore-cli -- steam-uninstall
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
`~/.local/share/gamestore` on Linux, `%APPDATA%\gamestore` on Windows.

## Environment

| Variable | What it does |
|----------|--------------|
| `GAMESTORE_CONFIG_DIR`, `GAMESTORE_DATA_DIR`, `GAMESTORE_CACHE_DIR` | Override the platform directories |
| `GAMESTORE_ACCOUNT` | Names the stored GOG session |
| `GAMESTORE_LOG` | Log filter, for example `GAMESTORE_LOG=debug` |
| `GOG_CLIENT_ID`, `GOG_CLIENT_SECRET` | OAuth2 credentials, over what `config.toml` holds |
| `STEAM_ROOT` | Points at a Steam installation the platform crates cannot guess |

## Releases

Two workflows, and neither is run by pushing a tag by hand:

- [`build.yml`](../.github/workflows/build.yml) runs on every push to every branch:
  `cargo fmt --check`, `clippy -D warnings`, `cargo test` and `cargo build`, on Linux
  and on Windows. It is also callable, which is how the release uses it.
- [`release.yml`](../.github/workflows/release.yml) is run by hand from the Actions tab,
  with one input: whether to raise the `patch`, `minor` or `major` version.

A release run raises the version in the workspace `Cargo.toml`, refreshes
`Cargo.lock`, commits, tags it `vX.Y.Z`, then calls `build.yml` **against that tag**
and publishes what comes out. So the binaries attached to a release are built from
exactly the commit the release names, and a failing check leaves the tag in place
and publishes nothing — a tag can be deleted, a published release cannot be taken
back. Tick `draft` to look it over before it goes out.

Attached to each release: `gamestore-x86_64-unknown-linux-gnu.tar.gz`,
`gamestore-x86_64-pc-windows-msvc.zip`, and `SHA256SUMS`. What the installer does
with them is in [`install.md`](install.md).
