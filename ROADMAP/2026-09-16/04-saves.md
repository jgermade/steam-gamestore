# Phase 3 — Saves

**Gantt IDs:** `lud`, `map`, `feed`
**Window:** 2026-12-07 → 2026-12-28
**Depends on:** `wrap`

## Goal

Saves are backed up and restored around every session, without reimplementing
per-game save path mapping. Ludusavi already knows Proton, Steam, GOG, Epic,
Heroic and Lutris layouts, and behaves the same on Linux and Windows.

## Tasks

### `lud` — Ludusavi CLI integration (5d)

- [ ] Invoke Ludusavi as a subprocess in `--api` mode and parse its JSON output.
- [ ] Detect a missing or too-old Ludusavi and say so clearly at install time,
      not at launch time.
- [ ] Map its exit codes to our error type; a failed restore must block the
      launch, a failed backup must be loud but must not lose the session.

### `map` — GOG to Ludusavi title mapping (5d)

- [ ] Resolve the GOG catalog identifier to the title Ludusavi expects in its
      manifest (usually the PCGamingWiki title).
- [ ] Resolve it **once, when the game is added to the catalog**, and persist it —
      not on every launch.
- [ ] Manual override in config for titles that do not match automatically.
- [ ] Report unmapped games instead of silently playing with no save coverage.

### `feed` — Pre and post launch screens (5d)

A floating in-game HUD was ruled out: it would need a custom Wayland client using
`wlr-layer-shell`, a project of its own. Instead:

- [ ] A brief full-screen window **before** the game, while `ludusavi restore`
      runs.
- [ ] A brief full-screen window **after** the game exits, while `ludusavi backup`
      runs.
- [ ] Nothing at all during play.
- [ ] It must render under gamescope with no desktop session, and must never
      block the launch if the UI fails to start.

## Acceptance criteria

- Play a game, delete the local save, relaunch: the save comes back.
- The backup step runs even when the game crashes.

## Risks

- **Title mapping is fuzzy.** A wrong match backs up nothing, or the wrong thing.
  Prefer failing visibly over guessing.
