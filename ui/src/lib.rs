//! The controller-driven UI: `gamestore ui`.
//!
//! A fullscreen window meant for a TV under gamescope, launched from a Steam tile:
//! the GOG library as a grid of covers, search with an on-screen keyboard, and
//! install with progress drawn on the cover. Gamepad and keyboard drive the same
//! navigation, so it can be tested away from the TV.
//!
//! Installs are `gamestore_platform_linux::install`, the CLI's own code, with
//! Steam writes deferred: launched by Steam, the UI cannot stop it, so new games
//! appear after Steam restarts, which the UI offers to do.
//!
//! Recorded in `RECORD/2026-09-16.controller-ui-requirement.completed.md` and
//! `RECORD/2026-10-06.controller-ui.WIP.md`.

mod app;
mod covers;
mod focus;
mod icons;
mod input;
mod jobs;
mod media;
mod osk;
mod text;
mod theme;

pub use jobs::{Machine, TokenSource};

use eframe::egui;
use gamestore_core::catalog::Game;
use gamestore_core::{Error, Result};

/// Open the UI and return when it is closed.
pub fn run(machine: Machine, games: Vec<Game>) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("gamestore")
            .with_fullscreen(true),
        ..Default::default()
    };
    eframe::run_native(
        "gamestore",
        options,
        Box::new(move |creation| {
            creation.egui_ctx.set_visuals(egui::Visuals::dark());
            Ok(Box::new(app::App::new(&creation.egui_ctx, machine, games)))
        }),
    )
    .map_err(|error| Error::Ui(error.to_string()))
}
