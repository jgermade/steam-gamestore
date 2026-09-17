# Phase 4 — Saves

**Gantt IDs:** `lud`, `map`
**Window:** 2026-11-26 → 2026-12-10
**Depends on:** `wrap`

## Goal

Saves are backed up and restored around every session without reimplementing
per-game save path mapping. Ludusavi already knows Proton, Steam, GOG, Epic, Heroic
and Lutris layouts, and behaves the same on Linux and Windows.

The pre/post launch screens that used to live in this phase are now `feed` in
[`04-ui.md`](04-ui.md): same UI binary, different mode.

## Tasks

### `lud` — Ludusavi CLI integration (5d)

- [ ] Invoke Ludusavi as a subprocess in `--api` mode and parse its JSON output.
- [ ] Detect a missing or too-old Ludusavi and say so clearly at install time, not
      at launch time. Whether to bundle it is open question 5.
- [ ] Map its exit codes to our error type: a failed restore must block the launch,
      a failed backup must be loud but must not lose the session.
- [ ] Record the outcome of each backup and restore in the registry, so the UI can
      show when a game was last protected.

### `map` — GOG to Ludusavi title mapping (5d)

- [ ] Resolve the GOG catalog identifier to the title Ludusavi expects in its
      manifest (usually the PCGamingWiki title).
- [ ] Resolve it **once, when the game is added to the catalog**, and persist it in
      the registry — not on every launch.
- [ ] Manual override in config for titles that do not match automatically.
- [ ] Report unmapped games instead of silently playing with no save coverage: a
      badge state in the grid, not a log line.

## Acceptance criteria

- Play a game, delete the local save, relaunch: the save comes back.
- The backup step runs even when the game crashes.
- A game with no Ludusavi mapping is visibly unprotected before it is played, not
  after.

## Risks

- **Title mapping is fuzzy.** A wrong match backs up nothing, or the wrong thing.
  Prefer failing visibly over guessing.
