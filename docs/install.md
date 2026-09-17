# Installing and updating

## The one line

```sh
curl -fsSL https://raw.githubusercontent.com/jgermade/steam-gamestore/main/install.sh | sh
```

That puts the binary in `~/.local/bin/gamestore`. Nothing outside that prefix is
touched, so it needs no root and works unchanged on an immutable SteamOS.

It takes the released binary when there is one for this machine, and builds from
source when there is not — which is what happens until the first release is cut, and
what happens on any system the published binary will not start on. The build needs
`cargo`, and the script says how to get one when there is none (`pacman -S rustup` on
Arch and CachyOS).

## Updating

The same line updates: run it again. Before it downloads anything it looks at what
is already installed, works out the latest release from the redirect
`releases/latest` answers with, and asks a question that fits what it found.

| What it finds | With a terminal | Piped with no terminal (CI, a script) |
|---------------|-----------------|---------------------------------------|
| Nothing installed | Installs | Installs |
| A newer release is out | `Update it to vX.Y.Z? [Y/n]` | Updates |
| The installed one is the latest | `Reinstall it? [y/N]` | Does nothing, and says so |
| The installed one is newer (a pinned `--version`) | `Downgrade it to vX.Y.Z? [y/N]` | Installs the named version |
| The latest release cannot be looked up | `Reinstall it? [Y/n]` | Installs |

The question is asked on `/dev/tty`, not on standard input — piped into `sh`, standard
input is the script itself — so it works in the one-liner and stays out of the way
where there is no terminal to answer it. `--force` skips the question and installs
regardless, which is also the way to reinstall an up-to-date copy non-interactively.

Building from source never asks, whether from a checkout or with `--from-source`:
those are an explicit "install what is in front of me", and a local tree carries no
version to compare against.

## Options

Options go after `--`, and the script is worth reading before it is run:

```sh
curl -fsSL .../install.sh | sh -s -- --prefix /opt/gamestore
curl -fsSL .../install.sh | sh -s -- --from-source
curl -fsSL .../install.sh | sh -s -- --version v0.1.0
curl -fsSL .../install.sh | sh -s -- --force
```

From a checkout it always builds what is in front of it:

```sh
./install.sh            # or --debug, for a faster edit-run loop
```

## What it touches

One file: `${prefix}/bin/gamestore`. It is written with `install -Dm755`, which
unlinks the old one before writing, so an update lands even while the binary is
running — which it is, when gamestore was started from a Steam tile.

Nothing else is written. The `PATH` line at the end is printed, not appended to any
profile; the temporary directory is removed on the way out; and configuration,
the installed-games registry and the stored GOG session are left alone, so an update
keeps you logged in.

The Steam tile is not re-registered, and does not need to be: it points at the same
path the new binary was written to. The exception is changing `--prefix` between
runs — the appid Steam derives comes from the executable path, so the old tile keeps
pointing at the old binary and a fresh `gamestore steam-install` makes a second tile
rather than moving the first. Run `gamestore steam-uninstall` before moving the
prefix.

## Verification

Released archives are published with a `SHA256SUMS` file, and the installer refuses
an archive that does not match it. A release without one is installed unverified,
and it says so.

The Linux binary is linked against glibc, so a distribution older than the runner's
will refuse it. The installer runs the downloaded binary once before installing it
and falls back to building from source when it does not start, which turns an
unreadable loader error into a slower install.
