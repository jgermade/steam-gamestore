# Phase 6 — Platform Windows

**Gantt IDs:** `winst`, `wwrap`, `wvdf`, `ps1` → milestone `M5`
**Window:** 2027-01-08 → 2027-02-03
**Depends on:** `M4`

## Goal

The same tool on the Windows laptop, so that syncing saves actually has a second
device to sync with. `core` is reused unchanged: auth, catalog, download and
manifest handling are platform-agnostic by construction.

## Tasks

### `winst` — Native installer runner (7d)

- [ ] Run the GOG `.exe` installer natively, with the same silent flags and the
      same post-install executable detection as the Linux path.
- [ ] No Proton, no prefix, no `STEAM_COMPAT_*` variables.
- [ ] Windows install paths, and UAC behaviour when the target needs elevation.

### `wwrap` — Windows launch wrapper (5d)

The same flow minus the Proton layer:

1. pull remote
2. `ludusavi restore`
3. run the game executable, blocking
4. `ludusavi backup`
5. push remote

- [ ] Generated as a script or a small launcher binary, whichever Steam handles
      more cleanly as a shortcut target.
- [ ] Same pre/post launch screens as [`04-saves.md`](04-saves.md).

### `wvdf` — Windows Steam shortcuts (3d)

- [ ] Non-Steam shortcuts are natively supported by Steam on Windows; reuse the
      `shortcuts.vdf` writer from `platform-linux` by moving it into `core` if it
      turns out to be identical.
- [ ] Only the `CompatToolMapping` step is Linux-only and must be skipped.

### `ps1` — `install.ps1` (3d)

- [ ] PowerShell twin of `install.sh`: detect architecture, query the latest
      release, verify the checksum, place the binary on `PATH`.
- [ ] `irm https://.../install.ps1 | iex` works on a clean Windows install.

## Milestone M5 — `v0.2` cross-platform

The same game, the same save, on both machines, installed by one line on each.

## Risks

- **VDF divergence.** If the Windows `shortcuts.vdf` layout differs at all, the
  shared writer must stay behind a trait rather than being forced into `core`.
