# Phase 2 — Platform Linux

**Gantt IDs:** `appid`, `vdf`, `compat`, `inno`, `wrap` → milestone `M2`
**Window:** 2026-10-14 → 2026-12-07
**Depends on:** `cat` (for `appid`), `ver` (for `inno`)

## Goal

A downloaded GOG game becomes a Steam tile that runs under the Proton that Steam
already manages. No Proton-GE installation of our own, no separate launcher.

## Tasks

### `appid` — Non-Steam appid (CRC32) (3d)

- [ ] Implement Steam's own non-Steam `appid` derivation (CRC32 over exe + app
      name). Reference implementations exist in projects such as Boilr.
- [ ] This is the first module to build: it is what ties `shortcuts.vdf` to the
      Proton `compatdata` directory, so every later Steam task depends on getting
      it byte-identical to Steam's.
- [ ] Unit tests against known exe/name → appid pairs.

### `vdf` — `shortcuts.vdf` writer (5d)

- [ ] Read, modify and write Steam's binary VDF without corrupting entries Steam
      or other tools added.
- [ ] Add, update and remove our own shortcuts idempotently (re-running the
      installer must not duplicate tiles).
- [ ] Back up the original file before the first write.
- [ ] Detect every Steam user directory under `userdata/`, since the machine is
      shared.

### `compat` — `CompatToolMapping` writer (4d)

- [ ] Write the shortcut's Proton mapping into `config.vdf` /
      `localconfig.vdf`, so the tile runs under Proton without the user touching
      per-game compatibility settings.
- [ ] Pick the Proton version from what Steam has installed; never ship or manage
      our own.
- [ ] Steam rewrites these files on exit: detect a running Steam and warn, or
      write only while it is stopped.

### `inno` — Silent InnoSetup install (7d, critical)

- [ ] Run the downloaded GOG installer through Proton with `/SILENT` or
      `/VERYSILENT`, plus `/DIR=` for the install target.
- [ ] Set up the prefix (`STEAM_COMPAT_DATA_PATH`,
      `STEAM_COMPAT_CLIENT_INSTALL_PATH`) before invoking `proton run`.
- [ ] Detect the installed game executable afterwards, for the shortcut and for
      the appid.
- [ ] Handle installers that ignore the silent flags: surface a clear error
      rather than hanging on an invisible dialog in console mode.

### `wrap` — Launch wrapper (5d)

Generate the bash wrapper that the Steam shortcut actually points at:

1. `ludusavi restore` (see [`04-saves.md`](04-saves.md))
2. export `STEAM_COMPAT_DATA_PATH` and `STEAM_COMPAT_CLIENT_INSTALL_PATH`
3. `proton run game.exe` — blocking
4. `ludusavi backup`, on exit

- [ ] The wrapper is generated per game and stored next to the installation.
- [ ] Non-zero exit from the game must still run the backup step.
- [ ] Log to a per-game file, since there is no terminal to read in Big Picture.

## Milestone M2 — Playable from Big Picture

A GOG game is downloaded, installed silently through Proton, appears as a tile
after a Steam restart, and launches to playable from the controller alone.

## Risks

- **Steam overwrites our files.** `shortcuts.vdf` and `localconfig.vdf` are owned
  by Steam at runtime. Writing while Steam runs loses changes.
- **Appid mismatch.** A wrong CRC32 means the prefix the wrapper sets up is not
  the prefix Steam launches into. Test this end to end early.
