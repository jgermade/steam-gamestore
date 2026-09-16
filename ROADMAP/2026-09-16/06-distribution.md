# Phase 5 — Distribution

**Gantt IDs:** `rel`, `ish`, `selfup` → milestone `M4`
**Window:** 2026-11-19 → 2026-12-08 (release train), M4 gated on `M3`
**Depends on:** `ver`, `ci0`

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
      release binaries for `x86_64-unknown-linux-gnu` and
      `x86_64-pc-windows-msvc`.
- [ ] Attach binaries to each GitHub release (for example with
      `softprops/action-gh-release`).
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
[`07-platform-windows.md`](07-platform-windows.md).

### `selfup` — `self-update` subcommand (5d)

- [ ] The binary updates itself from the latest release, so no one re-runs the
      install script for every version.
- [ ] Verify the checksum before replacing the running binary; keep the previous
      one for rollback.
- [ ] Never auto-update mid-session without being asked.

## Milestone M4 — `v0.1` on Linux

Fresh mini PC, one line, log in to GOG, install a game, play it from Big Picture
with saves restored and synced.

## Risks

- **Unauthenticated GitHub API rate limits** on a shared IP can break the install
  script. Fall back to the release redirect URL.
