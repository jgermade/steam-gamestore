//! Colours and sizes, chosen for a TV at sofa distance: high contrast, nothing
//! that relies on transparency over artwork to be read.

use eframe::egui::Color32;

pub const BACKGROUND: Color32 = Color32::from_rgb(18, 18, 24);
pub const PANEL: Color32 = Color32::from_rgb(32, 32, 42);
pub const KEY: Color32 = Color32::from_rgb(52, 52, 66);
pub const PLACEHOLDER: Color32 = Color32::from_rgb(44, 44, 58);
pub const TEXT: Color32 = Color32::from_rgb(236, 236, 242);
pub const DIM: Color32 = Color32::from_rgb(150, 150, 165);
/// Focus. GOG's purple, bright enough to read as a frame from across the room.
pub const ACCENT: Color32 = Color32::from_rgb(190, 140, 255);
pub const INSTALLED: Color32 = Color32::from_rgb(70, 190, 110);
pub const WORKING: Color32 = Color32::from_rgb(80, 160, 255);
pub const WAITING: Color32 = Color32::from_rgb(240, 180, 60);
pub const FAILED: Color32 = Color32::from_rgb(235, 80, 80);
/// The progress bar's track: solid, so the bar reads over any cover.
pub const TRACK: Color32 = Color32::from_rgb(10, 10, 14);
