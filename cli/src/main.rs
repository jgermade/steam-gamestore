//! The `gamestore` binary.
//!
//! The platform crate is chosen at compile time, so a Linux build never links the
//! Windows one and the other way round.

use std::process::ExitCode;

use std::io::{BufRead, Write};

use clap::{Parser, Subcommand};
use gamestore_core::http::UreqClient;
use gamestore_core::tokens::{self, Session, TokenStore};
use gamestore_core::{APP_NAME, Config, Paths, Platform, Result, logging};
use tracing::error;

mod platform;

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
