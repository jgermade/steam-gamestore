//! `gamestore install`: a GOG game, from the library to a Steam tile.
//!
//! The work is `gamestore_platform_linux::install`, which the UI uses too; this is
//! the terminal's view of it: find the game, pick the accounts, print the events.

use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use gamestore_core::catalog::{Catalog, Game, ProductId};
use gamestore_core::{Config, Error, Paths, Result, steam};
use gamestore_platform_linux::install::{self, Context, Event, Request, SteamPolicy};
use gamestore_platform_linux::shortcuts;

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

/// Install `options.query`.
pub fn run(
    config: &Config,
    paths: &Paths,
    steam_root: &Path,
    access_token: &str,
    catalog: &Catalog,
    options: &Options,
) -> Result<()> {
    let game = find_game(catalog, &options.query)?;
    let users = crate::steam::pick_users(
        shortcuts::steam_users(steam_root)?,
        options.user,
        options.all_users,
    )?;
    let context = Context {
        config,
        paths,
        steam_root,
        access_token,
    };
    let request = Request {
        game,
        users: &users,
        language: &options.language,
        artwork: !options.no_art,
        steam: if options.restart_steam {
            SteamPolicy::Restart
        } else {
            SteamPolicy::Refuse
        },
    };

    let tool = install::preflight(&context, &request)?;
    println!("{}  ({})", game.title, game.id);
    println!("proton:   {} ({})", tool.name, tool.dir.display());

    let mut printer = Printer::default();
    let outcome = install::install(&context, &request, &mut |event| printer.show(event));
    printer.end_line();
    let outcome = outcome?;

    println!("exe:      {}", outcome.exe.display());
    println!("appid:    {}", outcome.appid);
    if request.artwork {
        println!(
            "art:      {} image{}",
            outcome.art,
            if outcome.art == 1 { "" } else { "s" }
        );
    }
    for warning in &outcome.warnings {
        println!("warning:  {warning}");
    }
    println!();
    if outcome.pending {
        println!(
            "{} is installed. Steam started while it was being installed, so its tile is \
             queued: it is added the next time Steam starts (`gamestore steam-hook` makes that \
             automatic) or with `gamestore steam-apply` while Steam is closed.",
            game.title
        );
    } else {
        println!(
            "{} is in the library{}. Steam will make its Proton prefix at the first launch,",
            game.title,
            if outcome.restarted {
                ""
            } else {
                " once Steam starts"
            }
        );
        println!(
            "in {}/steamapps/compatdata/{}.",
            steam_root.display(),
            steam::compatdata_name(outcome.appid)
        );
    }
    Ok(())
}

/// Prints events, rewriting the download line in place a few times a second.
#[derive(Default)]
struct Printer {
    last: Option<Instant>,
    open_line: bool,
}

impl Printer {
    fn show(&mut self, event: Event) {
        match event {
            Event::Downloading {
                file,
                index,
                count,
                done,
                total,
            } => {
                let finished = done == total;
                if !finished
                    && self
                        .last
                        .is_some_and(|last| last.elapsed() < Duration::from_millis(300))
                {
                    return;
                }
                self.last = Some(Instant::now());
                let mib = |bytes: u64| bytes as f64 / (1024.0 * 1024.0);
                let percent = if total == 0 {
                    100.0
                } else {
                    done as f64 * 100.0 / total as f64
                };
                print!(
                    "\rdownload: [{index}/{count}] {file}  {:.1} / {:.1} MiB  ({percent:.0}%)   ",
                    mib(done),
                    mib(total)
                );
                let _ = std::io::stdout().flush();
                self.open_line = true;
                if finished {
                    self.end_line();
                }
            }
            Event::Installing { target } => {
                self.end_line();
                println!(
                    "install:  running the installer silently into {}",
                    target.display()
                );
            }
            Event::AlreadyInstalled { target } => {
                self.end_line();
                println!(
                    "install:  already in {}, not running the installer again",
                    target.display()
                );
            }
            Event::Artwork => {}
            Event::StoppingSteam => println!("steam:    stopping Steam for a moment"),
            Event::WritingSteam => println!("steam:    writing the tile and the Proton mapping"),
        }
    }

    fn end_line(&mut self) {
        if self.open_line {
            println!();
            self.open_line = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gamestore_core::catalog::Os;
    use std::time::SystemTime;

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
}
