# GOG Installer for Steam (gamescope/Big Picture)

## Context and goal

Mini PC running CachyOS/Bazzite, booting in "console mode" via **gamescope + Steam Big Picture**
(no traditional desktop), shared by the user and their kids, used only for gaming.

Goal: have a "game" in the Steam library that is actually a custom **GOG** installer, so that
installed games show up as regular tiles in Big Picture, indistinguishable from native Steam
games.

Motivation for building this from scratch instead of just using Heroic: `gogdl` (Heroic's
backend) is not up to date, and full control over the pipeline is preferred.

Epic support is out of scope for now — GOG only.

## Decisions made

- **Language**: Rust (plus some shell for launch wrappers).
- **Game runtime**: use the **Proton that Steam already manages**, not a separate Proton-GE
  setup. All compatibility handling is delegated to Steam.
- **Saves**: use **Ludusavi** (a multi-store save backup/restore tool, written in Rust) instead
  of reimplementing per-game save path mapping.
- **Distribution**: GitHub repo, binaries attached to releases, one-line install à la
  `nvm`/`rustup` (`curl | bash` on Linux, `irm | iex` in PowerShell on Windows).
- **Cross-platform**: also planning to bring the installer (and save sync) to Windows, so that
  syncing savegames across devices actually makes sense.

## Proposed architecture

Rust workspace split into:

- **`core`**: platform-shared logic.
  - OAuth2 auth against GOG (requires `client_id` + `client_secret`; the code→token exchange
    can NOT be done from a front-end-only web app because it would expose the secret — that
    would need a small backend/worker if built as a web app).
  - Catalog: list the user's library via GOG's (unofficial but well-documented) API.
  - Download: the hardest part to port is GOG's chunk/manifest-based download protocol
    (similar to Steam's). `gogdl` reimplements this in Python and serves as a reference.
  - Ludusavi integration (invoked as a subprocess via its CLI, with `--api` mode for JSON
    output).

- **`platform-linux`**:
  - Non-Steam `appid` calculation (CRC32 over exe+name, the same algorithm Steam uses
    internally — reference implementations exist in projects like Boilr).
  - Writing `shortcuts.vdf` (Steam library) and `config.vdf`/`localconfig.vdf` (for
    `CompatToolMapping`, which forces that shortcut to use Proton).
  - Launch wrapper (bash) that:
    1. `ludusavi restore` (before playing)
    2. exports `STEAM_COMPAT_DATA_PATH` / `STEAM_COMPAT_CLIENT_INSTALL_PATH`
    3. runs `proton run game.exe` (blocking)
    4. `ludusavi backup` (on close, since `proton run` blocks until the game exits)
  - Silent installation of GOG installers (InnoSetup) via Wine/Proton using `/SILENT` or
    `/VERYSILENT` flags.

- **`platform-windows`**:
  - Same core auth/catalog/download logic, unchanged.
  - No Proton needed: the `.exe` installer runs natively.
  - Equivalent launch wrapper (without the Proton layer).
  - Non-Steam shortcuts are also natively supported by Steam on Windows.

## Visual feedback during restore/backup

A floating HUD during gameplay was ruled out (it would require a custom Wayland client using
`wlr-layer-shell`, a non-trivial project on its own). Instead:

- A brief screen/window **before** launching the game, during `ludusavi restore`.
- A brief screen/window **after** closing the game, during `ludusavi backup`.
- Nothing during actual gameplay (restore/backup takes seconds, no overlay needed).

## Savegame sync

- Ludusavi only backs up/restores locally to a chosen folder; transporting that elsewhere (or
  to another device) is a separate concern. Options considered:
  - **rclone**: sync to Google Drive/OneDrive/Dropbox/WebDAV/S3, etc.
  - **Syncthing**: direct P2P sync between the user's own devices, no third-party provider.
  - A custom backend (Worker + storage, similar to the pattern used in the contact-form
    Cloudflare Workers project).
- Conflict strategy: for this use case (the same game isn't played in parallel on multiple
  devices), "last write wins" is enough — the default behavior of rclone/Syncthing.
- Full wrapper flow with sync: `pull remote → ludusavi restore → play (proton run) →
  ludusavi backup → push remote`.
- Ludusavi already natively supports Proton, Steam, GOG, Epic, Heroic, and Lutris, and works
  the same way on Windows and Linux, which makes syncing saves between a Linux mini PC and a
  Windows laptop feasible without extra conversion logic.

## Distribution (one-line install)

- GitHub Actions CI with a target matrix (`x86_64-unknown-linux-gnu`,
  `x86_64-pc-windows-msvc`), attaching binaries to each release (e.g. via
  `softprops/action-gh-release`).
- Predictable asset naming convention so install scripts can locate the right binary without
  hardcoding versions.
- `install.sh` (Linux) and `install.ps1` (Windows) that:
  1. Detect architecture/platform.
  2. Query `GET /repos/{owner}/{repo}/releases/latest` on the GitHub API.
  3. Download the matching asset and place it on the user's PATH.
- Possible `self-update` subcommand on the binary itself, so users don't need to re-run the
  install script for every update.

## Still to define / next steps

- Concrete structure of the Rust workspace (`core` + `platform-linux` + `platform-windows`).
- Mapping between each game's internal identifier (GOG catalog) and the name Ludusavi expects
  in its manifest (usually the title as it appears on PCGamingWiki) — best resolved once, when
  adding the game to the catalog, rather than on every launch.
- Implementation of the non-Steam `appid` calculation (CRC32) as the first module of
  `platform-linux`, since it's what connects `shortcuts.vdf` to the Proton `compatdata`.
- GitHub Actions CI for the build matrix and release publishing.
- Decide whether save sync will be rclone, Syncthing, or a custom backend.
