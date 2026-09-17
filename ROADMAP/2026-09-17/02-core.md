# Phase 1 — Core

**Gantt IDs:** `auth`, `tok`, `cat`, `art`, `state`, `man`, `dl`, `ver` → milestone `M1`
**Window:** 2026-09-17 → 2026-11-10
**Depends on:** `ws`

## Goal

Authenticate against GOG, list the library, know what is installed, and download a
game's files correctly — all headless, with no Steam, Proton or UI involved.

## Tasks

### `auth` — GOG OAuth2 authentication (3d, in progress)

Implemented and unit-tested; see
[`RECORD/2026-09-17.gog-oauth-flow.WIP.md`](../../RECORD/2026-09-17.gog-oauth-flow.WIP.md).
What is left is the part that needs the real service:

- [ ] Run `gamestore login` against GOG on the mini PC, end to end.
- [ ] Confirm the token response carries `access_token`, `refresh_token`,
      `user_id`, `session_id` and `expires_in`.
- [ ] Check whether an unregistered `redirect_uri` (`http://<lan-ip>:<port>/…`) is
      accepted. If it is, the QR handoff in
      [`09-nice-to-haves.md`](09-nice-to-haves.md) needs no paste step at all.
- [ ] Check whether any undocumented device-code endpoint exists.

The manual paste is the flow, not a fallback: GOG publishes no device grant and the
redirect is fixed on their side. The `HttpClient` trait added here is what keeps
the rest of this phase testable without a network.

### `tok` — Token storage and refresh (4d)

- [ ] Store tokens in the platform keyring where available, with an encrypted file
      fallback.
- [ ] Transparent refresh-token rotation, so long-lived sessions do not force the
      kids to re-authenticate mid-session. `auth::refresh` already exists; this is
      the session around it.
- [ ] Never write tokens to logs. `TokenSet` already redacts its `Debug`; keep that
      property when it gains a serialized form.
- [ ] One stored session per Steam user on a shared machine, given the directory
      overrides phase 0 added.

### `cat` — Library catalog (5d)

- [ ] List owned products through GOG's unofficial-but-documented API.
- [ ] Per-game detail: title, slug, available builds, platforms, languages.
- [ ] Local cache of the catalog under `Paths::cache_dir`, refreshed on demand, so
      browsing does not depend on the network being up.
- [ ] Stable per-game identifier that `state`, `art` and the Ludusavi mapping all
      key on.

### `art` — Cover art fetch and cache (3d)

- [ ] Fetch the artwork GOG serves with the catalog and cache it next to the
      catalog, keyed by product id.
- [ ] Fetch on catalog refresh, never while drawing a frame: a grid that blocks on
      the network is worse than a grid with placeholders.
- [ ] Vertical cover per game, with a placeholder for what GOG has no art for.
- [ ] Keep the source behind a small trait; SteamGridDB as a fallback is a nice to
      have, and it needs an API key.

### `state` — Installed games registry (3d)

The badge in the grid, uninstall and the launch wrapper all need to know what is
installed. The catalog cannot answer that — it only says what is owned.

- [ ] A registry under `Paths::data_dir`, keyed by product id, recording: state,
      installed version or build, install path, generated wrapper path, Steam
      appid, and the timestamp of the last successful launch.
- [ ] States: not installed, queued, downloading, installing, installed, update
      available, failed.
- [ ] Atomic writes (write a temporary file and rename), because the machine is a
      console that gets switched off at the wall.
- [ ] Survives a crash mid-install: a `downloading` or `installing` entry that no
      process owns is resumable or clearable, not a permanently stuck tile.

### `man` — Manifest parsing (7d)

- [ ] Fetch and parse build manifests (V1 and V2) for a selected game.
- [ ] Resolve depots, file lists and chunk lists, including language and DLC
      selection.
- [ ] Keep manifest handling behind a trait so a V3 does not ripple outward.

### `dl` — Chunked downloader (14d, critical)

- [ ] Concurrent chunk download with a bounded worker pool. The HTTP client is
      blocking by design, so this is threads, not an async runtime.
- [ ] Decompression and reassembly into the final file layout.
- [ ] Progress reporting through a channel, so the CLI and the UI subscribe to the
      same source. The grid's progress bar is a consumer of this.
- [ ] Concurrency configurable, defaulting to something polite; GOG rate-limits.
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

- **Protocol drift.** GOG can change the manifest format without notice.
- **Rate limiting.** Politeness by default, configurable when it is not enough.
- **Registry and reality diverging.** If `state` says installed and the files are
  gone, every later phase inherits the lie. Verification (`ver`) is what
  reconciles them, and the UI must be able to trigger it per game.
