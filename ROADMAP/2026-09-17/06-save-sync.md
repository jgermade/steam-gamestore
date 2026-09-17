# Phase 5 — Save sync

**Gantt IDs:** `seval`, `simpl` → milestone `M4`
**Window:** 2026-12-10 → 2027-01-05
**Depends on:** `map`

## Goal

Ludusavi only backs up and restores into a local folder. Getting that folder to a
second device is a separate concern, and it is the whole reason the Windows port
exists. The full wrapper flow becomes:

```
pull remote → ludusavi restore → play (proton run) → ludusavi backup → push remote
```

## Tasks

### `seval` — Evaluate the transport (4d)

Decide between the three options already on the table, and write the decision down
as a short ADR in `docs/`:

| Option | Pros | Cons |
|--------|------|------|
| **rclone** | One binary, many backends (Drive, OneDrive, Dropbox, WebDAV, S3). No always-on peer. | Needs a third-party account; remote config per device. |
| **Syncthing** | Direct P2P between the user's own devices, no third party. | Both devices must be reachable; a daemon to keep running. |
| **Custom backend** | Full control, same pattern as the existing Cloudflare Workers contact-form project. | We own storage, auth and uptime for a two-device problem. |

- [ ] Evaluate against: works headless under gamescope, works on Windows, no manual
      step for the kids, survives a laptop that is offline for a week.
- [ ] Pick one. The custom backend is the fallback, not the default.

### `simpl` — Implement the chosen transport (10d)

- [ ] `pull` before restore and `push` after backup, wired into the wrapper from
      [`03-platform-linux.md`](03-platform-linux.md).
- [ ] Conflict strategy: **last write wins**. The same game is not played on two
      devices in parallel here, and that is the default behaviour of both rclone and
      Syncthing.
- [ ] A stale or unreachable remote must not block play: warn, play on the local
      save, and push when the remote returns. The `feed` screen is where that
      warning is visible.
- [ ] Timeout on the pull step — nobody waits 60 seconds at a tile press.

## Milestone M4 — Saves survive and travel

Finish a session on the mini PC, open the same game on the Windows laptop, and
continue from the same save.

## Risks

- **Silent divergence.** Last-write-wins loses data if the rule is ever broken. Keep
  Ludusavi's own backup history as the undo path, and never prune it aggressively.
