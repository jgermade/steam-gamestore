# Phase 6 — Distribution

**Gantt IDs:** `rel`, `ish`, `selfup`, `e2e` → milestone `M5`
**Window:** 2026-11-10 → 2027-01-11
**Depends on:** `ver`, `ci0`; M5 also gated on `M4`

## Goal

Install with one line, the way `nvm` and `rustup` do:

```sh
curl -fsSL https://.../install.sh | bash
```

```powershell
irm https://.../install.ps1 | iex
```

## Tasks

### `rel` — Release workflow and assets (5d)

- [ ] Extend the CI matrix from [`01-foundations.md`](01-foundations.md) to build
      release binaries for `x86_64-unknown-linux-gnu` and `x86_64-pc-windows-msvc`.
- [ ] Attach binaries to each GitHub release (for example with
      `softprops/action-gh-release`). This is the job that needs `contents: write`,
      which CI deliberately does not have today.
- [ ] A predictable asset naming convention, so install scripts can find the right
      binary without hardcoding a version.
- [ ] Publish checksums alongside the binaries.

### `ish` — `install.sh` for Linux (3d)

1. Detect platform and architecture.
2. Query `GET /repos/{owner}/{repo}/releases/latest`.
3. Download the matching asset, verify its checksum, place it on `PATH`.

- [ ] Works on CachyOS and Bazzite, including an immutable `/usr`: install into
      `~/.local/bin` by default.
- [ ] Re-running the script upgrades in place.

`install.ps1` is the Windows twin and ships with
[`08-platform-windows.md`](08-platform-windows.md).

### `selfup` — `self-update` subcommand (5d)

- [ ] The binary updates itself from the latest release, so no one re-runs the
      install script for every version.
- [ ] Verify the checksum before replacing the running binary; keep the previous one
      for rollback.
- [ ] Never auto-update mid-session without being asked, and never while an install
      is in the queue.

### `e2e` — End to end pass on a fresh machine (3d)

New in this revision, because M5 claims something no single task proves:

- [ ] From a clean install: one line, log in to GOG, browse the grid, install a
      game, play it from Big Picture, uninstall it.
- [ ] Saves restored before play, backed up after, and reachable from the second
      device.
- [ ] Do it on the real machine, in console mode, with a controller and nothing
      else. Anything that needs a keyboard at that point is a bug against M3.

## Milestone M5 — `v0.1` on Linux

Fresh mini PC, one line, log in to GOG, install a game, play it from Big Picture
with saves restored and synced — verified by `e2e`, not assumed.

## Risks

- **Unauthenticated GitHub API rate limits** on a shared IP can break the install
  script. Fall back to the release redirect URL.
