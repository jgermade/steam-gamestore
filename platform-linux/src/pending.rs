//! Steam writes that wait for Steam to be stopped.
//!
//! Steam owns `shortcuts.vdf` and `config.vdf` while it runs and writes them back
//! when it exits, so a tile or a Proton mapping written underneath a running
//! client is lost. The CLI can stop Steam for a moment; the UI cannot, because it
//! is itself launched by Steam and would be stopped with it
//! (`RECORD/2026-10-06.installer-based-install.WIP.md`).
//!
//! So an install that finds Steam running queues what it would have written, as
//! one file per game under `<data>/pending/`, and [`apply`] writes them the next
//! time Steam is not running. On a gamescope session that moment is arranged by
//! [`hook`]: a drop-in on `steam-launcher.service` runs `gamestore steam-apply`
//! just before Steam starts, so restarting Steam is all it takes to see a new game.

use std::path::{Path, PathBuf};

use gamestore_core::atomic::{self, Access};
use gamestore_core::shortcuts::Shortcut;
use gamestore_core::{Error, Paths, Result};
use serde::{Deserialize, Serialize};

use crate::shortcuts::{self, SteamUser};
use crate::{proton, tile};

/// The unit a SteamOS-style gamescope session runs Steam under.
pub const STEAM_UNIT: &str = "steam-launcher.service";

/// Everything one game still needs written into Steam.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingTile {
    pub game_id: String,
    pub title: String,
    /// The Steam accounts that get the tile.
    pub users: Vec<u32>,
    pub app_name: String,
    pub exe: String,
    pub start_dir: String,
    pub launch_options: String,
    pub icon: String,
    /// The compatibility tool's internal name.
    pub tool: String,
    /// The appid of the game's previous tile, when it had one under a different
    /// appid: it is removed, with its mapping and artwork, so a game never ends up
    /// with two tiles.
    #[serde(default)]
    pub replaces: Option<u32>,
}

impl PendingTile {
    /// The shortcut this will write.
    pub fn shortcut(&self) -> Shortcut {
        Shortcut {
            app_name: self.app_name.clone(),
            exe: self.exe.clone(),
            start_dir: self.start_dir.clone(),
            launch_options: self.launch_options.clone(),
            icon: self.icon.clone(),
        }
    }

    /// The appid the shortcut gets.
    pub fn appid(&self) -> u32 {
        self.shortcut().appid()
    }
}

fn dir(paths: &Paths) -> PathBuf {
    paths.data_dir.join("pending")
}

/// Queue `tile`, replacing anything queued for the same game.
pub fn queue(paths: &Paths, tile: &PendingTile) -> Result<()> {
    let dir = dir(paths);
    atomic::create_dir(&dir, Access::Default)?;
    let json = serde_json::to_vec_pretty(tile)
        .map_err(|error| Error::Steam(format!("the pending tile could not be saved: {error}")))?;
    atomic::write(
        &dir.join(format!("{}.json", tile.game_id)),
        &json,
        Access::Default,
    )
}

/// What is queued, oldest name first. Unreadable files are skipped, not fatal:
/// one bad file must not keep every other game out of Steam.
pub fn list(paths: &Paths) -> Vec<(PathBuf, PendingTile)> {
    let Ok(entries) = std::fs::read_dir(dir(paths)) else {
        return Vec::new();
    };
    let mut found: Vec<_> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .filter_map(|path| {
            let tile = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
            Some((path, tile))
        })
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

/// Write one tile into Steam now. Steam must not be running.
pub fn write(steam_root: &Path, tile: &PendingTile) -> Result<()> {
    let shortcut = tile.shortcut();
    let users = shortcuts::steam_users(steam_root)?;
    if let Some(old) = tile.replaces.filter(|old| *old != shortcut.appid()) {
        // From every account, not only the ones getting the new tile: the old one
        // may have been given to more.
        for user in &users {
            let mut existing = shortcuts::read(user)?;
            if existing.remove(old) {
                shortcuts::save(user, &existing)?;
            }
            for kind in gamestore_core::artwork::Kind::ALL {
                gamestore_core::artwork::remove_kind(&user.grid_dir(), old, kind);
            }
        }
        proton::unmap(steam_root, old)?;
    }
    for id in &tile.users {
        let Some(user) = users.iter().find(|user| user.id == *id) else {
            continue;
        };
        write_shortcut(user, &shortcut)?;
    }
    proton::map(steam_root, shortcut.appid(), &tile.tool)
}

fn write_shortcut(user: &SteamUser, shortcut: &Shortcut) -> Result<()> {
    let mut existing = shortcuts::read(user)?;
    existing.upsert(shortcut)?;
    shortcuts::save(user, &existing)
}

/// Write everything queued, if Steam is not running from `steam_root`, and
/// return the titles written. Each game is removed from the queue only once it
/// is written.
pub fn apply(paths: &Paths, steam_root: &Path) -> Result<Vec<String>> {
    let queued = list(paths);
    if queued.is_empty() {
        return Ok(Vec::new());
    }
    if tile::steam_is_running_from(steam_root) {
        return Err(Error::Steam(format!(
            "{} game{} waiting to be added to Steam, which is running; they are added the next \
             time it starts, or by running this again with Steam closed",
            queued.len(),
            if queued.len() == 1 { " is" } else { "s are" }
        )));
    }
    let mut written = Vec::new();
    for (path, tile) in queued {
        write(steam_root, &tile)?;
        let _ = std::fs::remove_file(path);
        written.push(tile.title);
    }
    Ok(written)
}

/// The drop-in that runs `gamestore steam-apply` before Steam starts.
pub fn hook_path(config_home: &Path) -> PathBuf {
    config_home
        .join("systemd/user")
        .join(format!("{STEAM_UNIT}.d"))
        .join("gamestore.conf")
}

/// Its contents. The `-` makes a failure harmless: whatever happens to
/// gamestore, Steam still starts.
pub fn hook_contents(binary: &Path) -> String {
    format!(
        "# Written by `gamestore steam-hook`; remove with `gamestore steam-hook --remove`.\n\
         # Adds games installed while Steam was running, before Steam starts again.\n\
         [Service]\n\
         ExecStartPre=-{} steam-apply\n",
        binary.display()
    )
}

/// Whether this session runs Steam under [`STEAM_UNIT`].
pub fn has_steam_unit() -> bool {
    std::process::Command::new("systemctl")
        .args(["--user", "cat", STEAM_UNIT])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Install or remove the drop-in, then have systemd re-read it.
pub fn hook(config_home: &Path, binary: &Path, remove: bool) -> Result<PathBuf> {
    let path = hook_path(config_home);
    if remove {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(Error::io(format!("removing {}", path.display()), error)),
        }
    } else {
        atomic::create_dir(path.parent().expect("has a parent"), Access::Default)?;
        atomic::write(&path, hook_contents(binary).as_bytes(), Access::Default)?;
    }
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status();
    Ok(path)
}

/// Whether the drop-in is in place.
pub fn hook_installed(config_home: &Path) -> bool {
    hook_path(config_home).is_file()
}

/// Restart Steam through its unit, which runs the drop-in on the way up. Called
/// from inside Steam this ends the caller too, which is the point: it is the last
/// thing the UI does.
pub fn restart_steam() -> Result<()> {
    let status = std::process::Command::new("systemctl")
        .args(["--user", "--no-block", "restart", STEAM_UNIT])
        .status()
        .map_err(|error| Error::io("restarting Steam", error))?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::Steam(format!(
            "systemctl could not restart {STEAM_UNIT}"
        )))
    }
}

/// Restart Steam through its unit and wait until it has started again, which
/// includes the drop-in having run. For the CLI, which is not inside Steam.
pub fn restart_steam_and_wait() -> Result<()> {
    let status = std::process::Command::new("systemctl")
        .args(["--user", "restart", STEAM_UNIT])
        .status()
        .map_err(|error| Error::io("restarting Steam", error))?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::Steam(format!(
            "systemctl could not restart {STEAM_UNIT}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths_in(dir: &Path) -> Paths {
        Paths {
            config_dir: dir.join("config"),
            data_dir: dir.join("data"),
            cache_dir: dir.join("cache"),
        }
    }

    fn tile(id: &str) -> PendingTile {
        PendingTile {
            game_id: id.to_string(),
            title: format!("Game {id}"),
            users: vec![42],
            app_name: format!("Game {id}"),
            exe: format!("/games/{id}/game.exe"),
            start_dir: format!("/games/{id}"),
            launch_options: String::new(),
            icon: String::new(),
            tool: "proton_11".to_string(),
            replaces: None,
        }
    }

    fn steam_root(dir: &Path) -> PathBuf {
        let root = dir.join("steam");
        std::fs::create_dir_all(root.join("userdata/42/config")).unwrap();
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(
            root.join("config/config.vdf"),
            "\"InstallConfigStore\"\n{\n}\n",
        )
        .unwrap();
        root
    }

    #[test]
    fn queued_tiles_are_written_and_then_forgotten() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths_in(temp.path());
        let root = steam_root(temp.path());
        queue(&paths, &tile("1")).unwrap();
        queue(&paths, &tile("2")).unwrap();
        // Queuing a game again replaces it rather than adding a second entry.
        queue(&paths, &tile("2")).unwrap();
        assert_eq!(list(&paths).len(), 2);

        let written = apply(&paths, &root).unwrap();

        assert_eq!(written, ["Game 1", "Game 2"]);
        assert!(list(&paths).is_empty());
        let user = SteamUser {
            id: 42,
            dir: root.join("userdata/42"),
        };
        assert_eq!(shortcuts::read(&user).unwrap().len(), 2);
        let config = std::fs::read_to_string(root.join("config/config.vdf")).unwrap();
        assert!(config.contains(&tile("1").appid().to_string()), "{config}");
    }

    #[test]
    fn a_tile_that_replaces_another_removes_it_and_its_mapping() {
        let temp = tempfile::tempdir().unwrap();
        let root = steam_root(temp.path());
        let old = tile("1");
        write(&root, &old).unwrap();
        let grid = root.join("userdata/42/config/grid");
        std::fs::create_dir_all(&grid).unwrap();
        std::fs::write(grid.join(format!("{}p.png", old.appid())), "art").unwrap();

        let mut new = tile("1");
        new.exe = "/games/1/real.exe".to_string();
        new.replaces = Some(old.appid());
        write(&root, &new).unwrap();

        let user = SteamUser {
            id: 42,
            dir: root.join("userdata/42"),
        };
        let shortcuts = shortcuts::read(&user).unwrap();
        assert_eq!(shortcuts.appids(), [new.appid()]);
        let config = std::fs::read_to_string(root.join("config/config.vdf")).unwrap();
        assert!(!config.contains(&old.appid().to_string()), "{config}");
        assert!(config.contains(&new.appid().to_string()), "{config}");
        assert!(!grid.join(format!("{}p.png", old.appid())).exists());
    }

    #[test]
    fn an_unreadable_file_does_not_block_the_rest() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths_in(temp.path());
        queue(&paths, &tile("1")).unwrap();
        std::fs::write(dir(&paths).join("broken.json"), "{").unwrap();

        assert_eq!(list(&paths).len(), 1);
    }

    #[test]
    fn the_hook_never_stops_steam_from_starting() {
        let contents = hook_contents(Path::new("/home/me/.local/bin/gamestore"));
        assert!(contents.contains("ExecStartPre=-/home/me/.local/bin/gamestore steam-apply"));
        assert_eq!(
            hook_path(Path::new("/home/me/.config")),
            Path::new("/home/me/.config/systemd/user/steam-launcher.service.d/gamestore.conf")
        );
    }
}
