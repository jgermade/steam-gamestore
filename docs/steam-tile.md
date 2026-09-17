# Trying it from Steam

```sh
gamestore steam-install
```

This adds **gamestore itself** to Steam as a non-Steam tile. It is a test harness,
not the product — the controller UI is its own phase — but installing gamestore as
a game is the cheapest way to answer the two questions no test in this repository
can: whether Steam derives the same appid this code does, and whether it accepts a
`shortcuts.vdf` written here.

Restart Steam afterwards; the tile will not appear until you do. Then:

- Launch it once and check that the `compatdata` directory Steam creates is the
  number `steam-install` printed. **A different number means the appid derivation
  is wrong**, and `vdf`, `compat`, `wrap` and `uninst` all inherit it.
- Close Steam, open it again, and check the tile is still there. That is what proves
  the write survived the client rewriting the file on exit.

Steam owns `shortcuts.vdf` while it runs: it reads the file at startup and writes it
back when it quits, so a tile added underneath a running client is discarded the
moment that client exits. `steam-install` says so when it finds Steam running, but
the safe order is to close Steam first.

The tile runs `gamestore login` by default; `--command library` registers a second
one for something else. Because a tile starts with no terminal attached, the
shortcut points at a small generated launcher that opens one, runs gamestore inside
it, and waits for a keypress so the output can be read from the sofa.
`--no-terminal` points the tile straight at the binary instead.

On a machine with more than one Steam account the command refuses to guess and lists
them: pass `--user <id>` or `--all-users`. Whether a game installed by one user
should appear for all of them is question 5 in the roadmap, and a default here would
answer it by accident.

The file as it was first found is kept next to it as
`shortcuts.vdf.gamestore-backup`, taken once and never overwritten.
`gamestore steam-uninstall` removes the tile again.

Running `steam-install` twice is not a second tile: the entry is found by its appid
and only the fields gamestore owns are rewritten, so a tag added in Big Picture or a
hand-edited `LaunchOptions` survives. Updating the binary in place needs no new
`steam-install` at all — see [`install.md`](install.md).
