# Installing a game

```sh
gamestore install "Xeno Crisis" --restart-steam
```

The game can be named by its GOG id, its whole title, or a part of the title no
other game in the library has; an ambiguous name lists the candidates with their
ids. `gamestore library --search <text>` shows what is there.

What it does, in order:

1. **Downloads the Windows offline installer** from GOG into
   `~/.cache/gamestore/downloads/<id>/`, checking every 10 MiB chunk against GOG's
   checksum as it arrives. An interrupted download resumes from its last good
   chunk the next time; a finished one is not downloaded again.
2. **Runs the installer silently through Proton**, the one Steam uses by default
   (Steam Settings → Compatibility), into `~/.local/share/gamestore/games/<title>/`.
   The installer's log goes to `~/.local/share/gamestore/logs/install-<id>.log`. An
   installer that has not finished after an hour is stopped: it is almost
   certainly waiting on a window nobody can see.
3. **Adds the Steam tile**, pointing at the game's own executable, with Steam set
   to run it under that Proton. Steam creates the game's Proton prefix at the first
   launch, under `steamapps/compatdata/<appid>`.
4. **Fetches artwork** — portrait, wide capsule, hero and logo — from SteamGridDB
   when `[steamgriddb] api_key` is set in `config.toml`, and from GOG's store
   otherwise or for whatever SteamGridDB lacks.

Every step can be repeated, so when something fails, run the same command again.

## Steam has to be closed for the last step

Steam owns its files while it runs and writes them back when it exits, so a tile
added underneath a running Steam disappears when it quits. `install` therefore
refuses to start while Steam is running, unless `--restart-steam` is given: then
Steam is stopped only for the few seconds the Steam files take, after the download
and the install, and started again. On a gamescope session that is done through
the `steam-launcher.service` user unit, since the session would otherwise restart
Steam the moment it exits; elsewhere Steam is asked to shut down and has to be
started again by hand.

## Options

| | |
|---|---|
| `--language <code>` | Installer language when GOG has several (`en` by default; English is the fallback) |
| `--user <id>` / `--all-users` | Which Steam accounts get the tile, as for `steam-install` |
| `--restart-steam` | Stop Steam for the last step and start it again |
| `--no-art` | Skip the artwork |

## Not yet

Saves (Ludusavi) and uninstalling are not part of this yet. The tile runs the game
directly; the launch wrapper that restores and backs up saves will go into the
tile's launch options as `<wrapper> %command%`, which keeps Steam's Proton, overlay
and gamescope integration.
