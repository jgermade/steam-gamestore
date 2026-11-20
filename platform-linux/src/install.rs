//! Installing a GOG game into Steam: the orchestration both the CLI and the UI use.
//!
//! The installer-based path from
//! `RECORD/2026-10-06.manual-install-prototype.completed.md`: download the offline
//! installer verified chunk by chunk, run it silently through Steam's Proton, then
//! add the shortcut, its Proton mapping and its artwork.
//!
//! Every step can be run again. The download resumes and keeps what verified; a
//! game already in its install directory is not installed twice; the shortcut and
//! the mapping replace themselves. So when something fails, the answer is always
//! "run it again".
//!
//! Progress is reported as [`Event`]s, so the CLI prints them and the UI draws them
//! on the game's cover from the same source.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

use gamestore_core::catalog::{Game, Os};
use gamestore_core::http::{DownloadClient, UreqClient};
use gamestore_core::installer::{self, Checksum};
use gamestore_core::registry::{Registry, State};
use gamestore_core::shortcuts::Shortcut;
use gamestore_core::{Config, Error, Paths, Result, artwork};

use crate::pending::{self, PendingTile};
use crate::proton::{self, ProtonTool};
use crate::shortcuts::SteamUser;
use crate::{inno, native, tile};

/// How long an installer may run before it is taken to be stuck.
pub const INSTALL_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// What to do about Steam when it is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteamPolicy {
    /// Refuse before downloading anything: the CLI's default.
    Refuse,
    /// Stop Steam for the Steam writes and start it again: `--restart-steam`.
    Restart,
    /// Queue the Steam writes for the next time Steam is stopped: the UI, which
    /// runs inside Steam and cannot stop it.
    Defer,
}

/// Where an install has got to.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// One installer file is downloading; `index` counts from 1.
    Downloading {
        file: String,
        index: usize,
        count: usize,
        done: u64,
        total: u64,
    },
    /// The installer is running. It reports no progress of its own.
    Installing { target: PathBuf },
    /// The game was already installed by an earlier run.
    AlreadyInstalled { target: PathBuf },
    /// Fetching the library artwork.
    Artwork,
    /// Stopping Steam for the Steam writes.
    StoppingSteam,
    /// Writing the tile and the Proton mapping.
    WritingSteam,
}

/// How an install ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub appid: u32,
    pub exe: PathBuf,
    pub target: PathBuf,
    /// The installer's version, as GOG names it.
    pub version: String,
    /// Which build was installed.
    pub os: Os,
    /// The Steam writes are queued, waiting for Steam to restart.
    pub pending: bool,
    /// Steam was stopped and started again.
    pub restarted: bool,
    pub art: usize,
    /// Things that went wrong without stopping the install.
    pub warnings: Vec<String>,
}

/// What the install needs to know about this machine.
#[derive(Debug, Clone, Copy)]
pub struct Context<'a> {
    pub config: &'a Config,
    pub paths: &'a Paths,
    pub steam_root: &'a Path,
    pub access_token: &'a str,
}

/// What to install, for whom, and how.
#[derive(Debug, Clone)]
pub struct Request<'a> {
    pub game: &'a Game,
    /// Which build: the native Linux one, or the Windows one under Proton.
    pub os: Os,
    pub users: &'a [SteamUser],
    pub language: &'a str,
    pub artwork: bool,
    pub steam: SteamPolicy,
}

/// The directory a game is installed into: its title, without what a path or
/// Wine would choke on.
pub fn install_dir_name(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .filter(|c| {
            !matches!(
                c,
                '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '™' | '®' | '©'
            )
        })
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let cleaned = cleaned.trim_matches('.').to_string();
    if cleaned.is_empty() {
        "game".to_string()
    } else {
        cleaned
    }
}

/// Check what can be checked before anything is downloaded.
pub fn preflight(context: &Context, request: &Request) -> Result<Option<ProtonTool>> {
    if !builds(request.game).contains(&request.os) {
        return Err(Error::Install(format!(
            "{} has no {} build on GOG",
            request.game.title,
            os_label(request.os)
        )));
    }
    if request.steam == SteamPolicy::Refuse && tile::steam_is_running_from(context.steam_root) {
        return Err(Error::Steam(
            "Steam is running, and it would discard the new tile when it exits. Close it \
             first, or pass --restart-steam to have it stopped for the few seconds the \
             Steam files take and started again"
                .to_string(),
        ));
    }
    match request.os {
        Os::Windows => proton::choose(context.steam_root, None).map(Some),
        _ => Ok(None),
    }
}

/// The builds of `game` this machine can install, best first: the native Linux
/// build when GOG has one, then the Windows build under Proton.
pub fn builds(game: &Game) -> Vec<Os> {
    [Os::Linux, Os::Windows]
        .into_iter()
        .filter(|os| game.runs_on.contains(os))
        .collect()
}

/// The build to install when nobody says which.
pub fn preferred(game: &Game) -> Option<Os> {
    builds(game).first().copied()
}

/// A build's name in messages.
pub fn os_label(os: Os) -> &'static str {
    match os {
        Os::Linux => "Linux",
        Os::Windows => "Windows",
        Os::Mac => "Mac",
    }
}

/// Where `os`'s build of `game` is installed. The two builds of one game live
/// side by side, so switching does not overwrite what the other left behind.
pub fn target_dir(context: &Context, game: &Game, os: Os) -> PathBuf {
    let name = install_dir_name(&game.title);
    install_root(context).join(match os {
        Os::Linux => format!("{name} (Linux)"),
        _ => name,
    })
}

/// Install a game, reporting progress through `events`. The registry follows it
/// through downloading and installing to installed, or failed with the reason.
pub fn install(
    context: &Context,
    request: &Request,
    events: &mut dyn FnMut(Event),
) -> Result<Outcome> {
    let tool = preflight(context, request)?;
    let id = &request.game.id;

    let mut registry = Registry::load(context.paths)?;
    let previous = registry.get(id).and_then(|entry| entry.steam_appid);
    registry.claim(
        id,
        State::Downloading,
        std::process::id(),
        SystemTime::now(),
    );
    registry.save(context.paths)?;

    let result = run(
        context,
        request,
        tool.as_ref(),
        previous,
        &mut registry,
        events,
    );
    // Reload before recording the end: the UI may have been writing other games'
    // rows meanwhile, and this run only owns its own.
    let mut registry = Registry::load(context.paths)?;
    match &result {
        Ok(outcome) => {
            registry.mark_installed(
                id,
                outcome.version.clone(),
                outcome.target.clone(),
                outcome.appid,
            );
            registry.update(id, |entry| entry.platform = Some(outcome.os));
        }
        Err(error) => registry.mark_failed(id, error.to_string()),
    }
    registry.save(context.paths)?;
    result
}

fn install_root(context: &Context) -> PathBuf {
    context.config.install_root(context.paths)
}

fn run(
    context: &Context,
    request: &Request,
    tool: Option<&ProtonTool>,
    previous: Option<u32>,
    registry: &mut Registry,
    events: &mut dyn FnMut(Event),
) -> Result<Outcome> {
    let (paths, game) = (context.paths, request.game);
    let http = UreqClient::new();
    let cdn = DownloadClient::new();
    let mut warnings = Vec::new();

    // 1. Download.
    let installer = installer::installer(
        &http,
        context.access_token,
        &game.id,
        request.os,
        request.language,
    )?;
    let download_dir = installer::download_dir(paths, &game.id);
    let count = installer.files.len();
    let mut setup = None;
    for (position, file) in installer.files.iter().enumerate() {
        let link = installer::resolve(&http, context.access_token, file)?;
        let checksum_url = link.checksum.as_deref().ok_or_else(|| {
            Error::Download(format!(
                "GOG gives no checksum for {}, so it cannot be verified",
                file.id
            ))
        })?;
        let checksum = Checksum::fetch(&http, checksum_url)?;
        let name = checksum.name.clone();
        let mut progress = |done, total| {
            events(Event::Downloading {
                file: name.clone(),
                index: position + 1,
                count,
                done,
                total,
            });
        };
        let path = installer::download(&cdn, &link.url, &checksum, &download_dir, &mut progress)?;
        setup.get_or_insert(path);
    }
    let setup = setup.ok_or_else(|| Error::Download("the installer has no files".to_string()))?;

    // 2. Install, unless an earlier run already did.
    registry.claim(
        &game.id,
        State::Installing,
        std::process::id(),
        SystemTime::now(),
    );
    registry.save(paths)?;
    let target = target_dir(context, game, request.os);
    let mut installed_prefix = None;
    let task = match tool {
        None => match native::play_task(&target) {
            Some(task) => {
                events(Event::AlreadyInstalled {
                    target: target.clone(),
                });
                task
            }
            None => {
                if installer.files.len() > 1 {
                    return Err(Error::Install(format!(
                        "the Linux installer of {} comes in {} files, which is not handled yet",
                        game.title,
                        installer.files.len()
                    )));
                }
                events(Event::Installing {
                    target: target.clone(),
                });
                let work = paths.cache_dir.join("extract").join(game.id.as_str());
                native::install(&setup, &target, &work)?;
                native::play_task(&target).ok_or_else(|| {
                    Error::Install(format!("{} has no start.sh", target.display()))
                })?
            }
        },
        Some(tool) => {
            let task = match installer::primary_task(&target, &game.id) {
                Ok(task) => {
                    events(Event::AlreadyInstalled {
                        target: target.clone(),
                    });
                    task
                }
                Err(_) => {
                    events(Event::Installing {
                        target: target.clone(),
                    });
                    let prefix = paths.cache_dir.join("prefixes").join(game.id.as_str());
                    inno::install(&inno::Setup {
                        proton: tool,
                        steam_root: context.steam_root,
                        installer: &setup,
                        target: &target,
                        prefix: &prefix,
                        log: &install_log(paths, game),
                        timeout: INSTALL_TIMEOUT,
                    })?;
                    // The prefix is kept: what the installer wrote into it, its
                    // registry above all, is what the game expects to find. It
                    // becomes the game's Steam prefix once the appid is known.
                    installed_prefix = Some(prefix);
                    installer::primary_task(&target, &game.id)?
                }
            };
            // The per-game fixes are for Windows builds and their launchers.
            if let Some(settings) = gamestore_core::fixes::dgvoodoo(&game.id) {
                crate::tools::install_dgvoodoo(paths, &target, &settings)?;
            }
            gamestore_core::fixes::apply(&game.id, &target, task)
        }
    };

    // 3. The shortcut it will get, and so its appid.
    let icon = match tool {
        None => native::icon(&target).unwrap_or_default(),
        Some(_) => target.join(format!("goggame-{}.ico", game.id)),
    };
    let shortcut = Shortcut {
        app_name: game.title.clone(),
        exe: task.exe.display().to_string(),
        start_dir: task.working_dir.display().to_string(),
        launch_options: task.arguments.clone(),
        icon: if icon.is_file() {
            icon.display().to_string()
        } else {
            String::new()
        },
    };
    let appid = shortcut.appid();
    if let Some(prefix) = installed_prefix
        && let Some(problem) = adopt_prefix(context.steam_root, &prefix, appid)
    {
        warnings.push(problem);
    }

    // 4. Artwork. Steam reads it whether it is running or not, and a game without
    //    art is still a game, so a failure here is a warning.
    let mut art = 0;
    if request.artwork {
        events(Event::Artwork);
        match fetch_art(context, &http, &cdn, game, appid, request.users) {
            Ok(count) => art = count,
            Err(error) => warnings.push(format!("artwork skipped: {error}")),
        }
    }

    // 5. The Steam files.
    let tile = PendingTile {
        game_id: game.id.to_string(),
        title: game.title.clone(),
        users: request.users.iter().map(|user| user.id).collect(),
        app_name: shortcut.app_name.clone(),
        exe: shortcut.exe.clone(),
        start_dir: shortcut.start_dir.clone(),
        launch_options: shortcut.launch_options.clone(),
        icon: shortcut.icon.clone(),
        tool: tool.map(|tool| tool.name.clone()).unwrap_or_default(),
        replaces: previous,
    };
    let running = tile::steam_is_running_from(context.steam_root);
    let (pending, restarted) = match (running, request.steam) {
        (false, _) => {
            events(Event::WritingSteam);
            pending::write(context.steam_root, &tile)?;
            (false, false)
        }
        (true, SteamPolicy::Defer) => {
            pending::queue(paths, &tile)?;
            (true, false)
        }
        (true, SteamPolicy::Restart) => {
            if let Some(running) = tile::steam_game_running() {
                return Err(Error::Steam(format!(
                    "a game is running under Steam (appid {running}), and stopping Steam would \
                     close it. Everything up to here is done and kept: run this again once the \
                     game is closed, and only the Steam files are left to write"
                )));
            }
            if pending::has_steam_unit() {
                // Under a session unit the session may start Steam again the moment
                // it stops, so stopping it and writing in the gap is a race. Queue
                // instead, and restart through the unit: the hook writes the queue
                // before Steam starts, when it is certainly not running.
                pending::queue(paths, &tile)?;
                ensure_hook()?;
                events(Event::StoppingSteam);
                pending::restart_steam_and_wait()?;
                let applied = !pending::list(paths)
                    .iter()
                    .any(|(_, queued)| queued.game_id == tile.game_id);
                if !applied {
                    warnings.push(
                        "Steam restarted but the tile is still queued; it is added the next \
                         time Steam starts"
                            .to_string(),
                    );
                }
                (!applied, true)
            } else {
                events(Event::StoppingSteam);
                stop_steam(context.steam_root)?;
                events(Event::WritingSteam);
                let written = pending::write(context.steam_root, &tile);
                start_steam(&mut warnings);
                written?;
                (false, true)
            }
        }
        (true, SteamPolicy::Refuse) => {
            // Steam started while this was downloading; keep the work.
            pending::queue(paths, &tile)?;
            (true, false)
        }
    };

    let version = match tool {
        None => native::version(&target),
        Some(_) => None,
    }
    .or_else(|| installer.version.clone())
    .unwrap_or_default();
    Ok(Outcome {
        appid,
        exe: task.exe,
        target,
        version,
        os: request.os,
        pending,
        restarted,
        art,
        warnings,
    })
}

/// Install the steam-launcher drop-in if it is not there yet.
fn ensure_hook() -> Result<()> {
    let config_home = directories::BaseDirs::new()
        .map(|dirs| dirs.config_dir().to_path_buf())
        .ok_or(Error::MissingDirectory("config"))?;
    if !pending::hook_installed(&config_home) {
        let binary = std::env::current_exe()
            .map_err(|error| Error::io("finding the gamestore binary", error))?;
        pending::hook(&config_home, &binary, false)?;
    }
    Ok(())
}

/// Make the prefix a game was installed with its Steam prefix, `compatdata/<appid>`,
/// so the registry entries and anything else its installer put there are where
/// the game runs. Steam adopts a prefix made by the same Proton as its own. When
/// the game already has one (it has been played) that one is kept, since it may
/// hold saves, and the install prefix is discarded. Returns what went wrong, if
/// anything did.
pub fn adopt_prefix(steam_root: &Path, prefix: &Path, appid: u32) -> Option<String> {
    let compatdata = steam_root.join("steamapps/compatdata");
    let destination = compatdata.join(gamestore_core::steam::compatdata_name(appid));
    if destination.exists() {
        let _ = inno::discard_prefix(prefix);
        return Some(format!(
            "the game already has a Proton prefix ({}), which is kept; what its installer \
             wrote into the registry was not carried over",
            destination.display()
        ));
    }
    let moved = std::fs::create_dir_all(&compatdata)
        .and_then(|()| std::fs::rename(prefix, &destination))
        .or_else(|error| {
            // A Steam library on another filesystem than the cache: copy, keeping
            // the prefix's symlinks (dosdevices) as they are, then remove.
            if error.kind() != std::io::ErrorKind::CrossesDevices {
                return Err(error);
            }
            let copied = Command::new("cp")
                .arg("-a")
                .arg(prefix)
                .arg(&destination)
                .status()?;
            if !copied.success() {
                let _ = std::fs::remove_dir_all(&destination);
                return Err(std::io::Error::other("cp -a failed"));
            }
            let _ = std::fs::remove_dir_all(prefix);
            Ok(())
        });
    match moved {
        Ok(()) => None,
        Err(error) => {
            let _ = inno::discard_prefix(prefix);
            Some(format!(
                "the install prefix could not become the game's prefix ({error}); what its \
                 installer wrote into the registry was not carried over"
            ))
        }
    }
}

/// Where the installer's log for `game` goes.
pub fn install_log(paths: &Paths, game: &Game) -> PathBuf {
    paths.log_dir().join(format!("install-{}.log", game.id))
}

/// Put an installed game's artwork in place again, for every user in `users`:
/// the picked images first, then SteamGridDB's, then GOG's, as an install does.
/// For artwork made before a change to how it is made, such as the GOG badge.
/// Returns how many images were written per user.
pub fn refresh_art(
    context: &Context,
    game: &Game,
    appid: u32,
    users: &[SteamUser],
) -> Result<usize> {
    fetch_art(
        context,
        &UreqClient::new(),
        &DownloadClient::new(),
        game,
        appid,
        users,
    )
}

fn fetch_art(
    context: &Context,
    http: &UreqClient,
    cdn: &DownloadClient,
    game: &Game,
    appid: u32,
    users: &[SteamUser],
) -> Result<usize> {
    // What was picked by hand first; SteamGridDB's favourites for the rest.
    let mut arts = artwork::chosen(context.paths, &game.id);
    if arts.len() < artwork::Kind::ALL.len()
        && let Some(key) = context.config.steamgriddb.api_key.as_deref()
    {
        for art in artwork::steamgriddb(http, key, &game.id, &game.title)? {
            if !arts.iter().any(|known| known.kind == art.kind) {
                arts.push(art);
            }
        }
    }
    // GOG fills in only the kinds SteamGridDB had nothing for.
    if arts.len() < artwork::Kind::ALL.len() {
        for art in artwork::gog(http, &game.id, &game.title).unwrap_or_default() {
            if !arts.iter().any(|known| known.kind == art.kind) {
                arts.push(art);
            }
        }
    }
    let dirs: Vec<PathBuf> = users.iter().map(SteamUser::grid_dir).collect();
    artwork::save(cdn, &arts, appid, &dirs)?;
    Ok(arts.len())
}

/// Stop a Steam that is not run by a session unit, and wait for it to be gone.
fn stop_steam(steam_root: &Path) -> Result<()> {
    Command::new("steam")
        .arg("-shutdown")
        .status()
        .map_err(|error| Error::io("stopping Steam", error))?;

    let deadline = Instant::now() + Duration::from_secs(60);
    while tile::steam_is_running_from(steam_root) {
        if Instant::now() >= deadline {
            return Err(Error::Steam(
                "Steam did not exit within a minute; close it and run this again".to_string(),
            ));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Ok(())
}

fn start_steam(warnings: &mut Vec<String>) {
    warnings.push("start Steam again to see the new tile".to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_become_safe_directory_names() {
        assert_eq!(install_dir_name("Xeno Crisis"), "Xeno Crisis");
        assert_eq!(
            install_dir_name("Tomb Raider: The Angel of Darkness"),
            "Tomb Raider The Angel of Darkness"
        );
        assert_eq!(
            install_dir_name("Ultima™  Underworld I"),
            "Ultima Underworld I"
        );
        assert_eq!(install_dir_name("XCOM® 2"), "XCOM 2");
        assert_eq!(install_dir_name("../.."), "game");
    }
}
