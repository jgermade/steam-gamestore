# steam-gamestore

A GOG installer that lives inside the Steam library. Games bought on GOG are
downloaded, installed silently, and added as ordinary Steam tiles, so they launch
from Big Picture through the Proton that Steam already manages — with saves
restored before play and backed up afterwards by Ludusavi.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/jgermade/steam-gamestore/main/install.sh | sh
```

The binary lands in `~/.local/bin/gamestore`; nothing outside that prefix is
touched, so it needs no root and works unchanged on an immutable SteamOS. It takes
the released binary when there is one for this machine and builds from source when
there is not.

Running the same line again is how it is updated: it looks at what is installed
first and asks — to update when a newer release is out, to reinstall when there is
not. Options, the update rules, and what the installer does and does not touch:
[`docs/install.md`](docs/install.md).

## First run

```sh
gamestore login           # log in to GOG — needs client credentials
gamestore steam-install   # add gamestore to Steam as a tile, then restart Steam
```

`login` needs GOG OAuth2 client credentials of your own:
[`docs/gog-login.md`](docs/gog-login.md). `steam-install` adds **gamestore itself**
as a non-Steam tile — a test harness rather than the product, and the cheapest way
to find out whether Steam agrees with the appid this code derives:
[`docs/steam-tile.md`](docs/steam-tile.md).

## Status

Phase 0 is done, and six tasks are written on top of it:

| Task | What it does | State |
|------|--------------|-------|
| `auth` | GOG OAuth2 login | written, **unverified against GOG** |
| `tok` | Session storage and refresh | done |
| `cat` | Library catalog and cache | written, **unverified against GOG** |
| `state` | Installed games registry | done |
| `appid` | Steam's non-Steam appid | written, **unverified against Steam** |
| `vdf` | `shortcuts.vdf` reader and writer | written, **unverified against Steam** |

"Unverified" is meant literally: this has all been built in an environment that can
reach neither GOG nor Steam, so those four are complete and unit-tested against
fixtures nobody has compared to the real thing. What it would take to close them is
[`RECORD/2026-09-17.mini-pc-verification-plan.WIP.md`](RECORD/2026-09-17.mini-pc-verification-plan.WIP.md).

Downloads (`man`, `dl`, `ver`), the install path (`inno`, `wrap`, `uninst`) and the
controller UI are not started.

## Documentation

| | |
|---|---|
| [`docs/install.md`](docs/install.md) | Installing, updating, and what the installer touches |
| [`docs/gog-login.md`](docs/gog-login.md) | The GOG login flow, credentials, where the session is kept |
| [`docs/steam-tile.md`](docs/steam-tile.md) | Running gamestore from a Steam tile |
| [`docs/development.md`](docs/development.md) | Build, checks, workspace layout, environment, releases |
| [`docs/gog-installer.md`](docs/gog-installer.md) | Design notes: the whole thing, end to end |
| [`ROADMAP/2026-09-17/00-overview.md`](ROADMAP/2026-09-17/00-overview.md) | The plan, with a Gantt chart |

## Contributing

Read [`AGENTS.md`](AGENTS.md) first: it defines how the roadmap and the
append-only change records under [`RECORD/`](RECORD) are kept.
