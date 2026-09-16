# Open questions — revision 2026-09-16

Decisions this revision deliberately leaves open. Each one should be answered in
a record under `RECORD/`, and, when it changes the plan, in the next roadmap
revision.

## 1. Save sync transport — rclone, Syncthing, or a custom backend?

Owner of the decision: `seval` in [`05-save-sync.md`](05-save-sync.md).
Blocking: `simpl`, and therefore milestone M3.
Default if undecided: rclone, as the option with no daemon and no backend to
operate.

## 2. Where does the `shortcuts.vdf` writer live?

`platform-linux` for now. If the Windows layout turns out to be identical
(`wvdf`), move it to `core` behind a trait rather than duplicating it.

## 3. GOG `client_id` / `client_secret` distribution

The exchange cannot live in a front-end-only web app without exposing the secret.
In a distributed binary the secret is likewise not really secret. To settle before
the first public release: ship the well-known GOG client credentials as other
open-source clients do, or require the user to supply their own.

## 4. How are games added — from a tile, or only from the CLI?

The installer is meant to appear in the Steam library as a "game". Whether the
first version ships a controller-driven browse-and-install UI, or only a CLI with
the tile added afterwards, is not decided. The Gantt assumes CLI first.

## 5. Ludusavi as a dependency — bundled or required?

Detecting a system Ludusavi is simplest and is what `lud` assumes. Bundling a
known-good version avoids version skew across the two devices but makes the
release heavier.

## 6. Multi-user Steam on a shared machine

The mini PC is shared with the kids. Whether a game installed by one Steam user
should appear for all of them changes what `vdf` writes into each `userdata/`
directory.

## 7. DLC, language and build selection

`man` resolves them, but the policy — install everything, ask, or default to the
system language — is unspecified.
