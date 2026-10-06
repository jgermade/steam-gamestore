//! Installs started from the UI, one at a time, off the UI thread.
//!
//! The work is `gamestore_platform_linux::install`, the same code the CLI runs,
//! with Steam writes deferred: the UI is launched by Steam and cannot stop it. A
//! second install waits for the first rather than competing for bandwidth.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use gamestore_core::catalog::{Game, ProductId};
use gamestore_core::{Config, Paths, Result};
use gamestore_platform_linux::install::{self, Context, Event, Request, SteamPolicy};
use gamestore_platform_linux::shortcuts::SteamUser;

/// Gets a valid GOG access token, refreshing it if it has expired. Called on the
/// install thread when an install starts, so a UI left open for hours still works.
pub type TokenSource = Arc<dyn Fn() -> Result<String> + Send + Sync>;

/// What the machine is, for the install thread.
#[derive(Clone)]
pub struct Machine {
    pub config: Config,
    pub paths: Paths,
    pub steam_root: PathBuf,
    pub users: Vec<SteamUser>,
    pub token: TokenSource,
    pub language: String,
}

/// Where one game's install has got to.
#[derive(Debug, Clone, PartialEq)]
pub enum Progress {
    Queued,
    /// `fraction` of all installer files, with the file count for the label.
    Downloading {
        fraction: f32,
        index: usize,
        count: usize,
    },
    /// The silent installer, which reports nothing until it is done.
    Installing,
    /// Artwork and the Steam files.
    Finishing,
    /// Installed; `pending` means it shows up in Steam after a restart.
    Done {
        pending: bool,
    },
    Failed(String),
}

impl Progress {
    pub fn active(&self) -> bool {
        matches!(
            self,
            Progress::Queued
                | Progress::Downloading { .. }
                | Progress::Installing
                | Progress::Finishing
        )
    }
}

pub struct Jobs {
    queue: Sender<Game>,
    updates: Receiver<(ProductId, Progress)>,
}

impl Jobs {
    pub fn new(machine: Machine, wake: impl Fn() + Send + Sync + 'static) -> Self {
        let (queue, waiting) = channel::<Game>();
        let (report, updates) = channel();
        std::thread::spawn(move || {
            for game in waiting {
                let id = game.id.clone();
                let send = |progress| {
                    let _ = report.send((id.clone(), progress));
                    wake();
                };
                let result = run(&machine, &game, &send);
                send(match result {
                    Ok(pending) => Progress::Done { pending },
                    Err(error) => Progress::Failed(error.to_string()),
                });
            }
        });
        Self { queue, updates }
    }

    /// Queue a game. The caller marks it `Queued` itself, so the cover changes at
    /// once.
    pub fn install(&self, game: Game) {
        let _ = self.queue.send(game);
    }

    /// Progress reported since the last call.
    pub fn updates(&self) -> Vec<(ProductId, Progress)> {
        self.updates.try_iter().collect()
    }
}

fn run(machine: &Machine, game: &Game, send: &dyn Fn(Progress)) -> Result<bool> {
    let token = (machine.token)()?;
    let context = Context {
        config: &machine.config,
        paths: &machine.paths,
        steam_root: &machine.steam_root,
        access_token: &token,
    };
    let request = Request {
        game,
        users: &machine.users,
        language: &machine.language,
        artwork: true,
        steam: SteamPolicy::Defer,
    };

    // Downloads report per file; the bar covers all of them, so it never runs
    // to the end and starts again.
    let mut last = -1.0_f32;
    let outcome = install::install(&context, &request, &mut |event| match event {
        Event::Downloading {
            index,
            count,
            done,
            total,
            ..
        } => {
            let within = if total == 0 {
                1.0
            } else {
                done as f32 / total as f32
            };
            let fraction = ((index - 1) as f32 + within) / count.max(1) as f32;
            // A few hundred updates per file is plenty for a bar.
            if fraction - last >= 0.002 || done == total {
                last = fraction;
                send(Progress::Downloading {
                    fraction,
                    index,
                    count,
                });
            }
        }
        Event::Installing { .. } => send(Progress::Installing),
        Event::AlreadyInstalled { .. }
        | Event::Artwork
        | Event::StoppingSteam
        | Event::WritingSteam => send(Progress::Finishing),
    })?;
    Ok(outcome.pending)
}
