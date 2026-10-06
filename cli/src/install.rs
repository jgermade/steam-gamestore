//! `gamestore install`: a GOG game, from the library to a Steam tile.
//!
//! The installer-based path from `RECORD/2026-10-06.manual-install-prototype.completed.md`:
//! download the offline installer verified chunk by chunk, run it silently through
//! Steam's Proton, then add the shortcut, its Proton mapping and its artwork.
//!
//! Every step can be run again. The download resumes and keeps what verified; a
//! game already in its install directory is not installed twice; the shortcut and
//! the mapping replace themselves. So when something fails, the answer is always
//! "run the same command again".
//!
//! Steam is only needed stopped for the last few seconds: it owns `shortcuts.vdf`
//! and `config.vdf` while it runs and writes them back on exit.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

use gamestore_core::catalog::{Catalog, Game, ProductId};
use gamestore_core::http::{DownloadClient, UreqClient};
use gamestore_core::installer::{self, Checksum};
use gamestore_core::registry::{Registry, State};
use gamestore_core::shortcuts::Shortcut;
use gamestore_core::{Config, Error, Paths, Result, artwork, steam};
use gamestore_platform_linux::shortcuts::{self, SteamUser};
use gamestore_platform_linux::{inno, proton, tile};

/// How long an installer may run before it is taken to be stuck.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// The systemd unit a SteamOS-style gamescope session runs Steam under. Stopping
/// it is the only way to keep the session from starting Steam again at once.
const STEAM_UNIT: &str = "steam-launcher.service";

/// What `gamestore install` was asked for.
#[derive(Debug, Clone)]
pub struct Options {
    pub query: String,
    pub language: String,
    pub user: Option<u32>,
    pub all_users: bool,
    pub restart_steam: bool,
    pub no_art: bool,
}

/// The game `query` means: an id, an exact title, or a title fragment that only
/// one game matches.
pub fn find_game<'a>(catalog: &'a Catalog, query: &str) -> Result<&'a Game> {
    if let Some(game) = catalog.get(&ProductId::new(query.trim())) {
        return Ok(game);
    }
    let matches = catalog.search(query);
    if let Some(exact) = matches
        .iter()
        .find(|game| game.title.trim().eq_ignore_ascii_case(query.trim()))
    {
        return Ok(exact);
    }
    match matches.as_slice() {
        [only] => Ok(only),
        [] => Err(Error::Install(format!(
            "nothing in the library matches {query:?}; `gamestore library --search` lists titles"
        ))),
        many => {
            let listed: Vec<_> = many
                .iter()
                .take(10)
                .map(|game| format!("  {}  {}", game.id, game.title))
                .collect();
            Err(Error::Install(format!(
                "{query:?} matches {} games; name one by id or by its whole title:\n{}",
                many.len(),
                listed.join("\n")
            )))
        }
    }
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

/// Install `options.query`.
pub fn run(
    config: &Config,
    paths: &Paths,
    steam_root: &Path,
    access_token: &str,
    catalog: &Catalog,
    options: &Options,
) -> Result<()> {
    let game = find_game(catalog, &options.query)?.clone();
    if !game.has_windows_build() {
        return Err(Error::Install(format!(
            "{} has no Windows build on GOG, and the Windows build is what runs under Proton",
            game.title
        )));
    }
    let users = crate::steam::pick_users(
        shortcuts::steam_users(steam_root)?,
        options.user,
        options.all_users,
    )?;
    if tile::steam_is_running_from(steam_root) && !options.restart_steam {
        return Err(Error::Steam(
            "Steam is running, and it would discard the new tile when it exits. Close it \
             first, or pass --restart-steam to have it stopped for the few seconds the \
             Steam files take and started again"
                .to_string(),
        ));
    }
    let tool = proton::choose(steam_root, None)?;

    println!("{}  ({})", game.title, game.id);
    println!("proton:   {} ({})", tool.name, tool.dir.display());

    let mut registry = Registry::load(paths)?;
    registry.claim(
        &game.id,
        State::Downloading,
        std::process::id(),
        SystemTime::now(),
    );
    registry.save(paths)?;

    let result = install(
        config,
        paths,
        steam_root,
        access_token,
        &game,
        &users,
        &tool,
        options,
        &mut registry,
    );
    if let Err(error) = &result {
        registry.mark_failed(&game.id, error.to_string());
        registry.save(paths)?;
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn install(
    config: &Config,
    paths: &Paths,
    steam_root: &Path,
    access_token: &str,
    game: &Game,
    users: &[SteamUser],
    tool: &proton::ProtonTool,
    options: &Options,
    registry: &mut Registry,
) -> Result<()> {
    let http = UreqClient::new();
    let cdn = DownloadClient::new();

    // 1. Download.
    let installer = installer::windows_installer(&http, access_token, &game.id, &options.language)?;
    println!(
        "installer: {} {} ({} file{})",
        installer.language,
        installer.version.as_deref().unwrap_or(""),
        installer.files.len(),
        if installer.files.len() == 1 { "" } else { "s" }
    );
    let download_dir = installer::download_dir(paths, &game.id);
    let mut setup = None;
    for file in &installer.files {
        let link = installer::resolve(&http, access_token, file)?;
        let checksum_url = link.checksum.as_deref().ok_or_else(|| {
            Error::Download(format!(
                "GOG gives no checksum for {}, so it cannot be verified",
                file.id
            ))
        })?;
        let checksum = Checksum::fetch(&http, checksum_url)?;
        let path = installer::download(
            &cdn,
            &link.url,
            &checksum,
            &download_dir,
            &mut progress(&checksum.name),
        )?;
        println!();
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
    let target = config
        .install_root(paths)
        .join(install_dir_name(&game.title));
    let task = match installer::primary_task(&target, &game.id) {
        Ok(task) => {
            println!(
                "install:  already in {}, not running the installer again",
                target.display()
            );
            task
        }
        Err(_) => {
            println!(
                "install:  running the installer silently into {}",
                target.display()
            );
            let prefix = paths.cache_dir.join("prefixes").join(game.id.as_str());
            let log = paths.log_dir().join(format!("install-{}.log", game.id));
            let started = Instant::now();
            inno::install(&inno::Setup {
                proton: tool,
                steam_root,
                installer: &setup,
                target: &target,
                prefix: &prefix,
                log: &log,
                timeout: INSTALL_TIMEOUT,
            })?;
            println!(
                "          done in {}s (log: {})",
                started.elapsed().as_secs(),
                log.display()
            );
            if let Some(left) = inno::discard_prefix(&prefix) {
                println!(
                    "          (could not remove the install prefix {})",
                    left.display()
                );
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
    println!("exe:      {}", task.exe.display());
    println!("appid:    {appid}");

    // 4. Artwork. Steam reads these whether it is running or not, and a game
    //    without art is still a game, so a failure here is a warning.
    if !options.no_art {
        match fetch_art(config, &http, &cdn, game, appid, users) {
            Ok(0) => println!("art:      none found"),
            Ok(count) => println!(
                "art:      {count} image{}",
                if count == 1 { "" } else { "s" }
            ),
            Err(error) => println!("art:      skipped ({error})"),
        }
    }

    // 5. The Steam files, with Steam stopped.
    let restart = tile::steam_is_running_from(steam_root);
    if restart {
        stop_steam(steam_root)?;
    }
    let written = write_steam(steam_root, users, &shortcut, appid, &tool.name);
    if restart {
        start_steam();
    }
    written?;

    let version = installer.version.clone().unwrap_or_default();
    registry.mark_installed(&game.id, version, target.clone(), appid);
    registry.save(paths)?;

    println!();
    println!(
        "{} is in the library{}. Steam will make its Proton prefix at the first launch,",
        game.title,
        if restart { "" } else { " once Steam starts" }
    );
    println!(
        "in {}/steamapps/compatdata/{}.",
        steam_root.display(),
        steam::compatdata_name(appid)
    );
    Ok(())
}

fn write_steam(
    steam_root: &Path,
    users: &[SteamUser],
    shortcut: &Shortcut,
    appid: u32,
    tool: &str,
) -> Result<()> {
    for user in users {
        let mut existing = shortcuts::read(user)?;
        existing.upsert(shortcut)?;
        shortcuts::save(user, &existing)?;
        println!("steam:    tile in account {}", user.id);
    }
    proton::map(steam_root, appid, tool)?;
    println!("steam:    runs with {tool}");
    Ok(())
}

fn fetch_art(
    config: &Config,
    http: &UreqClient,
    cdn: &DownloadClient,
    game: &Game,
    appid: u32,
    users: &[SteamUser],
) -> Result<usize> {
    let mut arts = match config.steamgriddb.api_key.as_deref() {
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

/// A progress line that rewrites itself, at most a few times a second.
fn progress(name: &str) -> impl FnMut(u64, u64) + '_ {
    let mut last = Instant::now() - Duration::from_secs(1);
    move |done, total| {
        if last.elapsed() < Duration::from_millis(300) && done != total {
            return;
        }
        last = Instant::now();
        let mib = |bytes: u64| bytes as f64 / (1024.0 * 1024.0);
        let percent = if total == 0 {
            100.0
        } else {
            done as f64 * 100.0 / total as f64
        };
        print!(
            "\rdownload: {name}  {:.1} / {:.1} MiB  ({percent:.0}%)   ",
            mib(done),
            mib(total)
        );
        let _ = std::io::stdout().flush();
    }
}

/// Stop Steam and wait for it to be gone. Under a gamescope session Steam is a
/// systemd unit that is restarted the moment it exits, so the unit is stopped;
/// anywhere else Steam is asked to shut down.
fn stop_steam(steam_root: &Path) -> Result<()> {
    println!("steam:    stopping Steam for a moment");
    let unit = has_steam_unit();
    let status = if unit {
        Command::new("systemctl")
            .args(["--user", "stop", STEAM_UNIT])
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

fn start_steam() {
    if has_steam_unit() {
        let _ = Command::new("systemctl")
            .args(["--user", "start", STEAM_UNIT])
            .status();
        println!("steam:    started again");
    } else {
        println!("steam:    start Steam again to see the new tile");
    }
}

fn has_steam_unit() -> bool {
    Command::new("systemctl")
        .args(["--user", "cat", STEAM_UNIT])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gamestore_core::catalog::Os;

    fn game(id: &str, title: &str) -> Game {
        Game {
            id: ProductId::new(id),
            title: title.to_string(),
            slug: String::new(),
            image: None,
            runs_on: vec![Os::Windows],
        }
    }

    fn catalog() -> Catalog {
        let mut catalog = Catalog::empty(SystemTime::now());
        catalog.games = vec![
            game("1955095293", "Xeno Crisis"),
            game("1207659089", "Tomb Raider: The Angel of Darkness"),
            game("1202885143", "Tomb Raider: Anniversary"),
            game("1", "Xeno"),
        ];
        catalog
    }

    #[test]
    fn a_game_is_found_by_id_exact_title_or_unique_fragment() {
        let catalog = catalog();
        assert_eq!(
            find_game(&catalog, "1955095293").unwrap().title,
            "Xeno Crisis"
        );
        assert_eq!(find_game(&catalog, "xeno").unwrap().id.as_str(), "1");
        assert_eq!(
            find_game(&catalog, "anniversary").unwrap().id.as_str(),
            "1202885143"
        );
    }

    #[test]
    fn an_ambiguous_fragment_lists_the_candidates() {
        let error = find_game(&catalog(), "tomb raider")
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("1207659089") && error.contains("1202885143"),
            "{error}"
        );
    }

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
