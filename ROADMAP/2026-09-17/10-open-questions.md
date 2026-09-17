# Open questions — revision 2026-09-17

Decisions this revision deliberately leaves open. Each one should be answered in a
record under `RECORD/`, and, when it changes the plan, in the next roadmap revision.

## Settled since the previous revision

- **How are games added — from a tile, or only from the CLI?** (was question 4)
  Both: a controller-driven UI is now a phase of its own
  ([`04-ui.md`](04-ui.md)), and the CLI stays as the headless path. Recorded in
  [`RECORD/2026-09-16.controller-ui-requirement.completed.md`](../../RECORD/2026-09-16.controller-ui-requirement.completed.md).
- **How does the login work on a machine with no keyboard?** Manual
  authorization-code paste, with the QR handoff as a nice to have, because GOG
  publishes no device grant and the redirect is fixed on their side. Recorded in
  [`RECORD/2026-09-16.gog-login-flow-decision.completed.md`](../../RECORD/2026-09-16.gog-login-flow-decision.completed.md).

## 1. Save sync transport — rclone, Syncthing, or a custom backend?

Owner of the decision: `seval` in [`06-save-sync.md`](06-save-sync.md).
Blocking: `simpl`, and therefore milestone M4.
Default if undecided: rclone, as the option with no daemon and no backend to
operate.

## 2. Where does the `shortcuts.vdf` writer live?

`platform-linux` for now. If the Windows layout turns out to be identical (`wvdf`),
move it to `core` behind a trait rather than duplicating it.

## 3. GOG `client_id` / `client_secret` distribution

Still open, and now blocking in practice: nothing is baked into the binary, so
`gamestore login` does not work until the user supplies credentials in `config.toml`
or in the environment. To settle before the first public release: ship the
well-known GOG Galaxy credentials as other open-source clients do, or keep
requiring the user's own. In a distributed binary the secret is not really secret
either way.

## 4. Ludusavi as a dependency — bundled or required?

Detecting a system Ludusavi is simplest and is what `lud` assumes. Bundling a
known-good version avoids version skew across the two devices but makes the release
heavier.

## 5. Multi-user Steam on a shared machine

The mini PC is shared with the kids. Whether a game installed by one Steam user
should appear for all of them changes what `vdf` writes into each `userdata/`
directory, and what the grid shows.

## 6. DLC, language and build selection

`man` resolves them and `actions` exposes them, but the policy — install everything,
ask every time, or default to the system language — is unspecified.

## 7. The Rust floor and the UI toolkit version

`eframe` 0.36 requires Rust 1.95 while the workspace declares `rust-version =
"1.85"`. Either pin 0.35 for now or raise the floor deliberately; `spike` decides,
and whichever way it goes, CI installs stable and will not warn about it.

## 8. Where cover art comes from

GOG's own artwork is the default (`art`). Whether to add SteamGridDB as a fallback
depends on how many games in a real library end up with a placeholder, which is
only measurable once the grid exists.
