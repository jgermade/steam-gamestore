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

use gamestore_core::catalog::Game;
use gamestore_core::http::{DownloadClient, UreqClient};
use gamestore_core::installer::{self, Checksum};
use gamestore_core::registry::{Registry, State};
use gamestore_core::shortcuts::Shortcut;
use gamestore_core::{Config, Error, Paths, Result, artwork};

use crate::pending::{self, PendingTile};
use crate::proton::{self, ProtonTool};
use crate::shortcuts::SteamUser;
use crate::{inno, tile};

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
pub fn preflight(context: &Context, request: &Request) -> Result<ProtonTool> {
    if !request.game.has_windows_build() {
        return Err(Error::Install(format!(
            "{} has no Windows build on GOG, and the Windows build is what runs under Proton",
            request.game.title
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
    proton::choose(context.steam_root, None)
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
    registry.claim(
        id,
        State::Downloading,
        std::process::id(),
        SystemTime::now(),
    );
    registry.save(context.paths)?;

    let result = run(context, request, &tool, &mut registry, events);
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
    tool: &ProtonTool,
    registry: &mut Registry,
    events: &mut dyn FnMut(Event),
) -> Result<Outcome> {
    let (paths, game) = (context.paths, request.game);
    let http = UreqClient::new();
    let cdn = DownloadClient::new();
    let mut warnings = Vec::new();

    // 1. Download.
    let installer =
        installer::windows_installer(&http, context.access_token, &game.id, request.language)?;
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
    let target = install_root(context).join(install_dir_name(&game.title));
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
            if let Some(left) = inno::discard_prefix(&prefix) {
                warnings.push(format!(
                    "could not remove the install prefix {}",
                    left.display()
                ));
            }
            installer::primary_task(&target, &game.id)?
        }
    };

    // 3. The shortcut it will get, and so its appid.
    let icon = target.join(format!("goggame-{}.ico", game.id));
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
        tool: tool.name.clone(),
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
            events(Event::StoppingSteam);
            stop_steam(context.steam_root)?;
            events(Event::WritingSteam);
            let written = pending::write(context.steam_root, &tile);
            start_steam(&mut warnings);
            written?;
            (false, true)
        }
        (true, SteamPolicy::Refuse) => {
            // Steam started while this was downloading; keep the work.
            pending::queue(paths, &tile)?;
            (true, false)
        }
    };

    Ok(Outcome {
        appid,
        exe: task.exe,
        target,
        version: installer.version.clone().unwrap_or_default(),
        pending,
        restarted,
        art,
        warnings,
    })
}

/// Where the installer's log for `game` goes.
pub fn install_log(paths: &Paths, game: &Game) -> PathBuf {
    paths.log_dir().join(format!("install-{}.log", game.id))
}

fn fetch_art(
    context: &Context,
    http: &UreqClient,
    cdn: &DownloadClient,
    game: &Game,
    appid: u32,
    users: &[SteamUser],
) -> Result<usize> {
    let mut arts = match context.config.steamgriddb.api_key.as_deref() {
        Some(key) => artwork::steamgriddb(http, key, &game.id, &game.title)?,
        None => Vec::new(),
    };
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

/// Stop Steam and wait for it to be gone. Under a gamescope session Steam is a
/// systemd unit that is restarted the moment it exits, so the unit is stopped;
/// anywhere else Steam is asked to shut down.
fn stop_steam(steam_root: &Path) -> Result<()> {
    let status = if pending::has_steam_unit() {
        Command::new("systemctl")
            .args(["--user", "stop", pending::STEAM_UNIT])
            .status()
    } else {
        Command::new("steam").arg("-shutdown").status()
    };
    status.map_err(|error| Error::io("stopping Steam", error))?;

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
    if pending::has_steam_unit() {
        let _ = Command::new("systemctl")
            .args(["--user", "start", pending::STEAM_UNIT])
            .status();
    } else {
        warnings.push("start Steam again to see the new tile".to_string());
    }
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
