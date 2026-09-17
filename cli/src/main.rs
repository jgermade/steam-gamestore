//! The `gamestore` binary.
//!
//! The platform crate is chosen at compile time, so a Linux build never links the
//! Windows one and the other way round.

use std::process::ExitCode;
use std::time::SystemTime;

use std::io::{BufRead, Write};

use clap::{Parser, Subcommand};
use gamestore_core::http::UreqClient;
use gamestore_core::registry::{Registry, State};
use gamestore_core::tokens::{self, Session, TokenStore};
use gamestore_core::{APP_NAME, Config, Paths, Platform, Result, logging};
use tracing::error;

mod platform;
#[cfg(target_os = "linux")]
mod steam;

#[derive(Debug, Parser)]
#[command(name = APP_NAME, version, about = "Install and launch GOG games from the Steam library")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show what gamestore resolved on this machine.
    Info,
    /// Show the effective configuration and where it comes from.
    Config,
    /// Log in to GOG: open the printed URL, then paste where you land.
    Login,
    /// Forget the stored GOG session.
    Logout,
    /// List the GOG library, from the local cache unless asked to refresh.
    Library {
        /// Fetch from GOG instead of reading the cache.
        #[arg(long)]
        refresh: bool,
        /// Only show games whose title contains this.
        #[arg(long)]
        search: Option<String>,
    },
    /// Add gamestore to Steam as a non-Steam tile, to test it from Big Picture.
    #[cfg(target_os = "linux")]
    SteamInstall {
        /// Tile name, as Steam shows it. It is half of what the appid derives from.
        #[arg(long, default_value = gamestore_platform_linux::tile::TILE_NAME)]
        name: String,
        /// The gamestore arguments the tile runs.
        #[arg(long, default_value = gamestore_platform_linux::tile::DEFAULT_COMMAND)]
        command: String,
        /// Write into this Steam account only.
        #[arg(long)]
        user: Option<u32>,
        /// Write into every Steam account on the machine.
        #[arg(long)]
        all_users: bool,
        /// Point the tile straight at the binary, with no terminal to show its output.
        #[arg(long)]
        no_terminal: bool,
    },
    /// Remove the gamestore tile from Steam.
    #[cfg(target_os = "linux")]
    SteamUninstall {
        /// Tile name to remove; it has to match the one it was installed under.
        #[arg(long, default_value = gamestore_platform_linux::tile::TILE_NAME)]
        name: String,
        /// Remove from this Steam account only.
        #[arg(long)]
        user: Option<u32>,
        /// Remove from every Steam account on the machine.
        #[arg(long)]
        all_users: bool,
        /// The tile was installed with --no-terminal.
        #[arg(long)]
        no_terminal: bool,
    },
    /// Derive the Steam appid for a non-Steam shortcut, to compare against Steam.
    Appid {
        /// The executable path, unquoted; it is quoted the way Steam stores it.
        #[arg(long)]
        exe: String,
        /// The shortcut's display name, exactly as Steam shows it.
        #[arg(long)]
        name: String,
    },
}

/// The session, and why its tokens are kept where they are.
struct Stored {
    session: Session<UreqClient, Box<dyn TokenStore>>,
    /// Set when the keyring could not be used and a file is holding the tokens.
    keyring_unavailable: Option<String>,
}

fn open(config: &Config, paths: &Paths) -> Result<Stored> {
    let credentials = config.credentials(paths, |key| std::env::var(key).ok())?;
    let opened = tokens::open_store(paths, &tokens::account_from_env());

    Ok(Stored {
        session: Session::new(credentials, UreqClient::new(), opened.store),
        keyring_unavailable: opened.keyring_unavailable,
    })
}

impl Stored {
    fn report(&self) {
        match (self.session.user_id(), self.session.expires_at()) {
            (Some(user_id), Some(expires_at)) => {
                println!("Logged in as GOG user {user_id}.");
                match expires_at.duration_since(std::time::SystemTime::now()) {
                    Ok(left) => println!(
                        "The access token is valid for {} minutes; it refreshes itself after that.",
                        left.as_secs() / 60
                    ),
                    Err(_) => {
                        println!("The access token has expired and will be refreshed on use.")
                    }
                }
            }
            _ => println!("No session stored."),
        }

        println!("Stored in: {}", self.session.store().describe());
        // Said out loud on purpose: a file is a real downgrade from a keyring, and
        // the user should learn it here rather than from the source.
        if let Some(reason) = &self.keyring_unavailable {
            println!("Not in the keyring: {reason}");
            println!("Anyone who can read that file can use the session.");
        }
    }
}

fn main() -> ExitCode {
    logging::init();

    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            error!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let paths = Paths::resolve()?;
    let config = Config::load(&paths)?;
    let platform = platform::current();

    match cli.command {
        Command::Info => {
            println!("version:      {}", env!("CARGO_PKG_VERSION"));
            println!("platform:     {}", platform.id());
            println!("proton:       {}", platform.uses_proton());
            println!("config file:  {}", paths.config_file().display());
            println!("install root: {}", config.install_root(&paths).display());
            println!("cache dir:    {}", paths.cache_dir.display());
            println!("log dir:      {}", paths.log_dir().display());
            match platform.steam_root() {
                Ok(root) => println!("steam root:   {}", root.display()),
                Err(error) => println!("steam root:   unavailable ({error})"),
            }
            match open(&config, &paths) {
                Ok(mut stored) => {
                    match stored.session.restore() {
                        Ok(true) => println!(
                            "gog session:  logged in as {}",
                            stored.session.user_id().unwrap_or("?")
                        ),
                        Ok(false) => println!("gog session:  none stored"),
                        Err(error) => println!("gog session:  unavailable ({error})"),
                    }
                    println!("gog tokens:   {}", stored.session.store().describe());
                    if let Some(reason) = &stored.keyring_unavailable {
                        println!("              {reason}");
                    }
                }
                Err(error) => println!("gog session:  unavailable ({error})"),
            }
        }
        Command::Login => {
            let mut stored = open(&config, &paths)?;

            println!("Open this address and log in to GOG:");
            println!();
            println!("  {}", stored.session.credentials().authorization_url());
            println!();
            println!("GOG then lands on a page whose address carries `code=...`.");
            print!("Paste that address (or just the code) here: ");
            std::io::stdout()
                .flush()
                .map_err(|error| gamestore_core::Error::io("writing to the terminal", error))?;

            let mut pasted = String::new();
            std::io::stdin()
                .lock()
                .read_line(&mut pasted)
                .map_err(|error| gamestore_core::Error::io("reading from the terminal", error))?;

            stored.session.log_in(&pasted)?;
            stored.report();
        }
        Command::Logout => {
            let mut stored = open(&config, &paths)?;
            stored.session.log_out()?;
            println!("The stored session was forgotten.");
        }
        Command::Library { refresh, search } => {
            let mut stored = open(&config, &paths)?;
            stored.session.restore()?;

            let catalog = if refresh {
                let token = stored.session.access_token()?;
                let fetched =
                    gamestore_core::catalog::fetch(&UreqClient::new(), &token, SystemTime::now())?;
                fetched.save(&paths)?;
                fetched
            } else {
                match gamestore_core::catalog::Catalog::load(&paths)? {
                    Some(cached) => cached,
                    None => {
                        let token = stored.session.access_token()?;
                        gamestore_core::catalog::load_or_fetch(
                            &paths,
                            &UreqClient::new(),
                            &token,
                            SystemTime::now(),
                        )?
                    }
                }
            };

            // Anything left mid-install by a machine switched off at the wall is
            // cleared here, before it is drawn as a permanently busy row.
            let mut registry = Registry::load(&paths)?;
            let reset = registry.reconcile(SystemTime::now());
            if !reset.is_empty() {
                registry.save(&paths)?;
            }

            let games = catalog.search(search.as_deref().unwrap_or_default());
            for game in &games {
                // The Windows marker is the one that matters: it is the build that
                // gets installed under Proton.
                let windows = if game.has_windows_build() {
                    "win"
                } else {
                    "   "
                };
                let state = match registry.state_of(&game.id) {
                    State::NotInstalled => "-",
                    State::Queued => "queued",
                    State::Downloading => "downloading",
                    State::Installing => "installing",
                    State::Installed => "installed",
                    State::UpdateAvailable => "update",
                    State::Failed => "failed",
                };
                println!("{:<12} {windows}  {state:<12} {}", game.id, game.title);
            }
            println!();
            println!("{} of {} games.", games.len(), catalog.len());
            for id in &reset {
                let title = catalog.get(id).map_or("?", |game| game.title.as_str());
                println!("{title} was interrupted before it finished; start it again.");
            }
        }
        Command::Appid { exe, name } => {
            // The point of this command is the mini PC: `03-platform-linux.md`
            // calls an appid mismatch the risk to test end to end early, and the
            // way to test it is to compare these numbers against a shortcut Steam
            // made itself.
            let stored_exe = gamestore_core::steam::quote_exe(&exe);
            let appid = gamestore_core::steam::shortcut_appid(&stored_exe, &name);

            println!("exe as stored: {stored_exe}");
            println!("name:          {name}");
            println!("appid:         {appid}");
            println!(
                "shortcut id:   {}",
                gamestore_core::steam::shortcut_id(&stored_exe, &name)
            );
            println!(
                "compatdata:    steamapps/compatdata/{}",
                gamestore_core::steam::compatdata_name(appid)
            );
            // Which appid variant the artwork files use is still unverified —
            // `RECORD/2026-09-17.pending-roadmap-changes.WIP.md` lists it as one of
            // the things the mini PC run has to settle — so it is offered as the
            // thing to check, not stated as the answer.
            println!(
                "artwork:       expected under userdata/<user>/config/grid/ keyed on {appid} (unverified)"
            );
        }
        #[cfg(target_os = "linux")]
        Command::SteamInstall {
            name,
            command,
            user,
            all_users,
            no_terminal,
        } => {
            let binary = current_binary()?;
            let tile = steam::Tile {
                name,
                command,
                terminal: !no_terminal,
            };

            steam::install(
                &paths,
                &platform.steam_root()?,
                &binary,
                &tile,
                user,
                all_users,
            )?;
        }
        #[cfg(target_os = "linux")]
        Command::SteamUninstall {
            name,
            user,
            all_users,
            no_terminal,
        } => {
            let binary = current_binary()?;
            let tile = steam::Tile {
                name,
                command: String::new(),
                terminal: !no_terminal,
            };

            steam::uninstall(
                &paths,
                &platform.steam_root()?,
                &binary,
                &tile,
                user,
                all_users,
            )?;
        }
        Command::Config => {
            let path = paths.config_file();
            let origin = if path.exists() {
                path.display().to_string()
            } else {
                format!("{} (not created yet, showing defaults)", path.display())
            };
            println!("# {origin}");
            print!("{}", config.to_toml_redacted()?);
        }
    }

    Ok(())
}

/// Where this binary is, which is what a Steam tile has to point at.
///
/// It is resolved rather than assumed so that a tile installed from a `cargo run`
/// checkout points at the checkout, and one installed from `~/.local/bin` points
/// there — and so the difference is visible in what the command prints.
#[cfg(target_os = "linux")]
fn current_binary() -> Result<std::path::PathBuf> {
    std::env::current_exe()
        .map_err(|error| gamestore_core::Error::io("finding the gamestore binary", error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_command_line_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn the_platform_matches_the_target_it_was_built_for() {
        let expected = if cfg!(target_os = "linux") {
            "linux"
        } else if cfg!(target_os = "windows") {
            "windows"
        } else {
            "unsupported"
        };

        assert_eq!(platform::current().id(), expected);
    }
}
