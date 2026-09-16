# Phase 1 — Core

**Gantt IDs:** `auth`, `tok`, `cat`, `man`, `dl`, `ver` → milestone `M1`
**Window:** 2026-09-22 → 2026-11-19
**Depends on:** `ws`

## Goal

Authenticate against GOG, list the user's library, and download a game's files
correctly — all from the CLI, with no Steam or Proton involved yet.

## Tasks

### `auth` — GOG OAuth2 authentication (7d)

- [ ] OAuth2 authorization-code flow against GOG, with `client_id` and
      `client_secret`.
- [ ] Local redirect capture: a loopback listener, or the manual "paste the code"
      fallback for a machine running in console mode.
- [ ] The code→token exchange stays in the binary. It cannot move into a
      front-end-only web app without exposing the secret; if a web UI is ever
      wanted, it needs a backend, which is out of scope here.

### `tok` — Token storage and refresh (4d)

- [ ] Store tokens in the platform keyring where available, with an encrypted
      file fallback.
- [ ] Transparent refresh-token rotation, so long-lived sessions do not force the
      kids to re-authenticate mid-session.
- [ ] Never write tokens to logs.

### `cat` — Library catalog (5d)

- [ ] List owned products through GOG's unofficial-but-documented API.
- [ ] Per-game detail: title, slug, available builds, platforms, languages.
- [ ] Local cache of the catalog, refreshed on demand, so Big Picture browsing
      does not depend on the network being up.

### `man` — Manifest parsing (7d)

- [ ] Fetch and parse build manifests (V1 and V2) for a selected game.
- [ ] Resolve depots, file lists and chunk lists, including language and DLC
      selection.

### `dl` — Chunked downloader (14d, critical)

- [ ] Concurrent chunk download with a bounded worker pool.
- [ ] Decompression and reassembly into the final file layout.
- [ ] Progress reporting through a channel, so the CLI and any UI can subscribe.
- [ ] `gogdl` is the reference implementation to read; the protocol is not
      officially specified.

### `ver` — Integrity check and resume (5d)

- [ ] Per-chunk and per-file checksum verification.
- [ ] Resume an interrupted download without re-fetching verified chunks.
- [ ] Re-verify an existing installation and repair only the bad files.

## Milestone M1 — Download works

A game is authenticated, listed, downloaded and verified from the CLI, on both
Linux and Windows, with resume across a killed process.

## Risks

- **Protocol drift.** GOG can change the manifest format without notice. Keep
  manifest handling isolated behind a trait so a V3 does not ripple outward.
- **Rate limiting.** Concurrency must be configurable and default to something
  polite.
