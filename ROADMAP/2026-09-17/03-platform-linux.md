# Phase 2 — Platform Linux

**Gantt IDs:** `appid`, `vdf`, `compat`, `inno`, `wrap`, `uninst` → milestone `M2`
**Window:** 2026-10-05 → 2026-12-02
**Depends on:** `cat` (for `appid`), `ver` (for `inno`), `state` (for `uninst`)

## Goal

A downloaded GOG game becomes a Steam tile that runs under the Proton that Steam
already manages — and can be removed again without leaving anything behind except
the saves.

## Tasks

### `appid` — Non-Steam appid (CRC32) (3d)

- [ ] Implement Steam's own non-Steam `appid` derivation (CRC32 over exe + app
      name). Reference implementations exist in projects such as Boilr.
- [ ] This is the first module to build: it ties `shortcuts.vdf` to the Proton
      `compatdata` directory, so every later Steam task depends on it being
      byte-identical to Steam's.
- [ ] Unit tests against known exe/name → appid pairs.

### `vdf` — `shortcuts.vdf` writer (5d)

- [ ] Read, modify and write Steam's binary VDF without corrupting entries Steam
      or other tools added.
- [ ] Add, update and remove our own shortcuts idempotently; removal is the same
      code path `uninst` uses.
- [ ] Back up the original file before the first write.
- [ ] Detect every Steam user directory under `userdata/`, since the machine is
      shared — and see open question 6 about which of them to write into.

### `compat` — `CompatToolMapping` writer (4d)

- [ ] Write the shortcut's Proton mapping into `config.vdf` / `localconfig.vdf`, so
      the tile runs under Proton without per-game settings.
- [ ] Pick the Proton version from what Steam has installed; never ship or manage
      our own.
- [ ] Steam rewrites these files on exit: detect a running Steam and warn, or write
      only while it is stopped. The UI has to surface that, since a tile press is
      the only interaction available.

### `inno` — Silent InnoSetup install (7d, critical)

- [ ] Run the downloaded GOG installer through Proton with `/SILENT` or
      `/VERYSILENT`, plus `/DIR=` for the install target.
- [ ] Set up the prefix (`STEAM_COMPAT_DATA_PATH`,
      `STEAM_COMPAT_CLIENT_INSTALL_PATH`) before invoking `proton run`.
- [ ] Detect the installed game executable afterwards, for the shortcut and the
      appid.
- [ ] Handle installers that ignore the silent flags: fail with a clear error rather
      than hanging on an invisible dialog, record `failed` in the registry, and keep
      the per-game log path so the UI can point at it.
- [ ] Report phase transitions to the same progress channel `dl` uses, even without
      a percentage — the UI shows an indeterminate bar for this step.

### `wrap` — Launch wrapper (5d)

Generate the script the Steam shortcut points at:

1. pull remote (once [`06-save-sync.md`](06-save-sync.md) lands)
2. `ludusavi restore`
3. export `STEAM_COMPAT_DATA_PATH` and `STEAM_COMPAT_CLIENT_INSTALL_PATH`
4. `proton run game.exe` — blocking
5. `ludusavi backup`, on exit
6. push remote

- [ ] Generated per game, stored next to the installation, path recorded in the
      registry.
- [ ] Non-zero exit from the game must still run the backup step.
- [ ] Log to a per-game file; there is no terminal to read in Big Picture.
- [ ] Record the launch in the registry, so the UI can sort by "last played".

### `uninst` — Uninstall (4d)

- [ ] Remove, in order: the Steam shortcut, the Proton prefix under
      `compatdata/<appid>`, the generated wrapper, the installed files, and the
      registry entry.
- [ ] **Never delete saves.** Ludusavi's backups are the undo path for everything
      else here, and a controller-driven UI on a machine shared with the kids is
      exactly where a mis-click must not be destructive. Deleting backups is not an
      option the flow offers.
- [ ] Idempotent: a half-finished install must be removable, and re-running must
      not fail on what is already gone.
- [ ] Report what could not be removed rather than failing silently — a leftover
      prefix is a disk-space problem months later.

## Milestone M2 — Playable from Big Picture

A GOG game is downloaded, installed silently through Proton, appears as a tile
after a Steam restart, and launches to playable from the controller alone.

## Risks

- **Steam overwrites our files.** `shortcuts.vdf` and `localconfig.vdf` are owned
  by Steam at runtime; writing while Steam runs loses changes.
- **Appid mismatch.** A wrong CRC32 means the prefix the wrapper sets up is not the
  prefix Steam launches into. Test end to end early.
