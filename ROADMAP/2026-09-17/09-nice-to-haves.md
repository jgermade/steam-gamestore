# Nice to haves

Work that is wanted but must not gate a milestone. Each one names the task it can
only start after, so it can be picked up whenever there is room — or dropped without
moving anything.

## `qr` — QR login handoff from a phone (3d, after `auth` and `grid`)

Scan a QR on the TV, log in to GOG on a phone, and finish without typing an email
with a gamepad. It also moves GOG's captcha and two-factor to the device that
handles them well.

- [ ] Serve a single-use, short-TTL path on the LAN, and show its QR in the UI.
- [ ] The phone follows the GOG authorization URL and pastes back the address it
      lands on; the code→token exchange stays in the binary.
- [ ] Bind to the LAN interface only, never log the code, and accept that a
      plain-HTTP LAN hop carries a short-lived single-use code.

If the first check in `auth` shows GOG accepts an unregistered `redirect_uri`, this
loses its paste step entirely and becomes clearly worth building. Reasoning and
alternatives in
[`RECORD/2026-09-16.gog-login-flow-decision.completed.md`](../../RECORD/2026-09-16.gog-login-flow-decision.completed.md).

## `art2` — SteamGridDB fallback for missing covers (2d, after `art`)

GOG does not have artwork for everything, and a wall of placeholders is a poor grid.
SteamGridDB needs an API key, which is why it is not a dependency of the first
version: the key handling is the work, not the fetching.

## `upd` — Update detection and reinstall (4d, after `ver` and `state`)

The registry already has an `update available` state. Filling it means comparing the
installed build against the catalog, and reusing the download and install path with
the old install removed only after the new one verifies.

## `multi` — Shared-machine polish (3d, after `vdf`)

Open question 6 decides whether a game installed by one Steam user appears for all
of them. Whatever the answer, the nice-to-have part is making it visible: whose
library a tile belongs to, and a per-user view in the grid.
