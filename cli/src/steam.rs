//! `steam-install` and `steam-uninstall`: gamestore itself as a Steam tile.
//!
//! Installing gamestore as a game is how the first two unverifiable claims in this
//! repository get tested. Steam either derives the same appid this code does, or it
//! does not; either reads back the `shortcuts.vdf` written here, or it does not.
//! Both answers arrive within a minute of restarting the client.

use std::path::Path;

use gamestore_core::shortcuts::Shortcut;
use gamestore_core::{Error, Paths, Result, steam};
use gamestore_platform_linux::shortcuts::{self, SteamUser};
use gamestore_platform_linux::tile;

/// What to register, and what the tile should run.
#[derive(Debug, Clone)]
pub struct Tile {
    /// Display name, which is half of what the appid is derived from.
    pub name: String,
    /// The gamestore arguments the tile runs.
    pub command: String,
    /// Whether to go through the terminal-opening launcher.
    pub terminal: bool,
}

/// The Steam accounts a command should write into.
///
/// With one account on the machine the choice makes itself. With several it does
/// not: open question 5 is exactly whether a game installed by one Steam user
/// should appear for all of them, and guessing here would answer it by accident.
pub fn pick_users(users: Vec<SteamUser>, id: Option<u32>, all: bool) -> Result<Vec<SteamUser>> {
    if users.is_empty() {
        return Err(Error::Steam(
            "no Steam account has a profile on this machine yet; start Steam, log in once, \
and run this again"
                .to_string(),
        ));
    }

    let listed = users
        .iter()
        .map(|user| user.id.to_string())
        .collect::<Vec<_>>()
        .join(", ");

    if let Some(id) = id {
        let found: Vec<_> = users.into_iter().filter(|user| user.id == id).collect();
        if found.is_empty() {
            return Err(Error::Steam(format!(
                "no Steam account {id} on this machine; the ones under userdata/ are: {listed}"
            )));
        }
        return Ok(found);
    }

    if all || users.len() == 1 {
        return Ok(users);
    }

    Err(Error::Steam(format!(
        "this machine has {} Steam accounts ({listed}); pass --user <id> for one of them, \
or --all-users for every one",
        users.len()
    )))
}

/// The shortcut gamestore registers for itself.
pub fn shortcut(tile: &Tile, binary: &Path, launcher: Option<&Path>) -> Shortcut {
    let exe = launcher.unwrap_or(binary);

    Shortcut {
        app_name: tile.name.clone(),
        exe: exe.to_string_lossy().into_owned(),
        start_dir: binary
            .parent()
            .unwrap_or(Path::new("/"))
            .to_string_lossy()
            .into_owned(),
        launch_options: tile.command.clone(),
        icon: String::new(),
    }
}

/// Add the tile to each selected account.
pub fn install(
    paths: &Paths,
    steam_root: &Path,
    binary: &Path,
    tile: &Tile,
    id: Option<u32>,
    all: bool,
) -> Result<()> {
    let users = pick_users(shortcuts::steam_users(steam_root)?, id, all)?;

    let launcher = if tile.terminal {
        Some(tile::write_launcher(paths, binary)?)
    } else {
        None
    };
    let shortcut = shortcut(tile, binary, launcher.as_deref());
    let appid = shortcut.appid();

    warn_if_steam_is_running();

    println!(
        "tile:          {} (runs: gamestore {})",
        tile.name, tile.command
    );
    println!("exe:           {}", shortcut.stored_exe());
    println!("appid:         {appid}");
    println!(
        "shortcut id:   {}",
        steam::shortcut_id(&shortcut.stored_exe(), &shortcut.app_name)
    );
    println!(
        "compatdata:    {}/steamapps/compatdata/{}",
        steam_root.display(),
        steam::compatdata_name(appid)
    );
    if let Some(launcher) = &launcher {
        println!("launcher:      {}", launcher.display());
    }
    println!();

    for user in &users {
        let mut existing = shortcuts::read(user)?;
        let before = existing.len();
        existing.upsert(&shortcut)?;
        shortcuts::save(user, &existing)?;

        let verb = if existing.len() == before {
            "updated"
        } else {
            "added"
        };
        println!(
            "account {}:  {verb} in {}",
            user.id,
            user.shortcuts_file().display()
        );
        println!(
            "               the file as it was first found is kept at {}",
            user.shortcuts_backup().display()
        );
    }

    println!();
    println!("Restart Steam, and the tile appears under the non-Steam games.");
    println!("Then the two things nobody has checked yet:");
    println!(
        "  1. launch it once and confirm Steam created compatdata/{}",
        steam::compatdata_name(appid)
    );
    println!("     — a different number there means the appid derivation is wrong, and");
    println!("       `vdf`, `compat`, `wrap` and `uninst` all inherit it.");
    println!("  2. confirm the tile is still there after Steam has been closed and reopened,");
    println!("     which is what proves the write survived the client rewriting the file.");

    Ok(())
}

/// Remove the tile from each selected account.
pub fn uninstall(
    paths: &Paths,
    steam_root: &Path,
    binary: &Path,
    tile: &Tile,
    id: Option<u32>,
    all: bool,
) -> Result<()> {
    let users = pick_users(shortcuts::steam_users(steam_root)?, id, all)?;

    // The launcher is not written here, only named: removing a tile should not
    // create the file it is about to stop pointing at.
    let launcher = tile.terminal.then(|| tile::launcher_path(paths));
    let appid = shortcut(tile, binary, launcher.as_deref()).appid();

    warn_if_steam_is_running();

    for user in &users {
        let mut existing = shortcuts::read(user)?;
        if existing.remove(appid) {
            shortcuts::save(user, &existing)?;
            println!(
                "account {}:  removed {appid} from {}",
                user.id,
                user.shortcuts_file().display()
            );
        } else {
            println!(
                "account {}:  no shortcut with appid {appid} was there",
                user.id
            );
        }
    }

    Ok(())
}

/// Say so when Steam is running, because the write will not survive it.
fn warn_if_steam_is_running() {
    if !tile::steam_is_running() {
        return;
    }

    println!("Steam is running. It reads shortcuts.vdf at startup and writes it back when it");
    println!("quits, so this change is discarded the moment Steam exits. Close Steam, run this");
    println!("again, then start Steam.");
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn user(id: u32) -> SteamUser {
        SteamUser {
            id,
            dir: PathBuf::from(format!("/steam/userdata/{id}")),
        }
    }

    fn tile() -> Tile {
        Tile {
            name: "gamestore".to_string(),
            command: "login".to_string(),
            terminal: true,
        }
    }

    #[test]
    fn the_only_account_on_the_machine_needs_no_choosing() {
        let picked = pick_users(vec![user(7)], None, false).unwrap();

        assert_eq!(picked, vec![user(7)]);
    }

    #[test]
    fn several_accounts_and_no_choice_is_an_error_that_lists_them() {
        let error = pick_users(vec![user(7), user(9)], None, false).unwrap_err();

        let message = error.to_string();
        assert!(message.contains('7') && message.contains('9'), "{message}");
        assert!(
            message.contains("--user") && message.contains("--all-users"),
            "{message}"
        );
    }

    #[test]
    fn asking_for_every_account_gets_every_account() {
        let picked = pick_users(vec![user(7), user(9)], None, true).unwrap();

        assert_eq!(picked.len(), 2);
    }

    #[test]
    fn naming_an_account_picks_only_that_one() {
        let picked = pick_users(vec![user(7), user(9)], Some(9), false).unwrap();

        assert_eq!(picked, vec![user(9)]);
    }

    #[test]
    fn naming_an_account_that_is_not_there_lists_the_ones_that_are() {
        let error = pick_users(vec![user(7), user(9)], Some(3), false).unwrap_err();

        let message = error.to_string();
        assert!(message.contains("no Steam account 3"), "{message}");
        assert!(message.contains('7') && message.contains('9'), "{message}");
    }

    #[test]
    fn no_steam_account_at_all_says_what_to_do_about_it() {
        let error = pick_users(Vec::new(), None, false).unwrap_err();

        assert!(error.to_string().contains("start Steam"), "{error}");
    }

    #[test]
    fn the_tile_points_at_the_launcher_and_carries_the_command() {
        let launcher = PathBuf::from("/data/steam/gamestore-tile.sh");
        let shortcut = shortcut(&tile(), Path::new("/bin/gamestore"), Some(&launcher));

        assert_eq!(shortcut.exe, "/data/steam/gamestore-tile.sh");
        assert_eq!(shortcut.start_dir, "/bin");
        assert_eq!(shortcut.launch_options, "login");
    }

    #[test]
    fn without_a_terminal_the_tile_points_straight_at_the_binary() {
        let shortcut = shortcut(&tile(), Path::new("/bin/gamestore"), None);

        assert_eq!(shortcut.exe, "/bin/gamestore");
    }

    #[test]
    fn the_appid_follows_the_exe_so_the_two_variants_are_different_tiles() {
        let launcher = PathBuf::from("/data/steam/gamestore-tile.sh");
        let wrapped = shortcut(&tile(), Path::new("/bin/gamestore"), Some(&launcher));
        let bare = shortcut(&tile(), Path::new("/bin/gamestore"), None);

        assert_ne!(wrapped.appid(), bare.appid());
    }
}
