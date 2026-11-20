//! The on-screen keyboard, drawn by the UI itself.
//!
//! Steam's own keyboard belongs to Steam's overlay and cannot be relied on for a
//! window under gamescope (`RECORD/2026-09-16.controller-ui-requirement.completed.md`).
//! A to type, X to delete, Y for a space, B to close, Start to finish. Search is
//! case-insensitive, so there is no shift: one layer of lowercase letters, digits
//! and the few symbols game titles use.

use eframe::egui::{self, Align2, Color32, FontId, Rect, Stroke, StrokeKind, pos2, vec2};

use crate::input::Nav;
use crate::theme;

const ROWS: [&str; 4] = ["1234567890", "qwertyuiop", "asdfghjklñ", "zxcvbnm:'-"];
/// The last row: space, delete, done.
const SPECIAL: usize = 3;

/// What a press on the keyboard did to the text.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Typing,
    /// Close the keyboard, keeping the text.
    Done,
}

#[derive(Debug, Default)]
pub struct Keyboard {
    row: usize,
    column: usize,
}

impl Keyboard {
    fn width(row: usize) -> usize {
        if row < ROWS.len() {
            ROWS[row].chars().count()
        } else {
            SPECIAL
        }
    }

    pub fn handle(&mut self, nav: Nav, text: &mut String) -> Outcome {
        let rows = ROWS.len() + 1;
        match nav {
            Nav::Up => {
                self.row = (self.row + rows - 1) % rows;
                self.fit_column();
            }
            Nav::Down => {
                self.row = (self.row + 1) % rows;
                self.fit_column();
            }
            Nav::Left => {
                let width = Self::width(self.row);
                self.column = (self.column + width - 1) % width;
            }
            Nav::Right => self.column = (self.column + 1) % Self::width(self.row),
            Nav::Accept => {
                if self.row < ROWS.len() {
                    if let Some(c) = ROWS[self.row].chars().nth(self.column) {
                        text.push(c);
                    }
                } else {
                    match self.column {
                        0 => text.push(' '),
                        1 => {
                            text.pop();
                        }
                        _ => return Outcome::Done,
                    }
                }
            }
            Nav::Action => {
                text.pop();
            }
            Nav::Search => text.push(' '),
            Nav::Back | Nav::Menu => return Outcome::Done,
            Nav::Previous | Nav::Next => {}
        }
        Outcome::Typing
    }

    /// Moving between rows of different widths keeps the column roughly in place.
    fn fit_column(&mut self) {
        let width = Self::width(self.row);
        if self.row == ROWS.len() {
            self.column = (self.column * SPECIAL / 10).min(SPECIAL - 1);
        } else if self.column >= width {
            self.column = width - 1;
        }
    }

    /// Draw along the bottom of `screen`.
    pub fn show(&self, painter: &egui::Painter, screen: Rect, labels: [&str; 3]) {
        let key = (screen.width() / 16.0).clamp(48.0, 110.0);
        let gap = key * 0.12;
        let width = 10.0 * key + 9.0 * gap;
        let height = 5.0 * key + 4.0 * gap;
        let origin = pos2(
            screen.center().x - width / 2.0,
            screen.max.y - height - key * 0.6,
        );
        let panel = Rect::from_min_size(
            origin - vec2(gap * 2.0, gap * 2.0),
            vec2(width, height) + vec2(gap * 4.0, gap * 4.0),
        );
        painter.rect_filled(panel, 16.0, theme::PANEL);

        let font = FontId::proportional(key * 0.45);
        let draw = |rect: Rect, label: &str, focused: bool| {
            painter.rect_filled(rect, 10.0, if focused { theme::ACCENT } else { theme::KEY });
            if focused {
                painter.rect_stroke(
                    rect,
                    10.0,
                    Stroke::new(3.0, Color32::WHITE),
                    StrokeKind::Outside,
                );
            }
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                label,
                font.clone(),
                if focused { Color32::BLACK } else { theme::TEXT },
            );
        };

        for (row, keys) in ROWS.iter().enumerate() {
            for (column, c) in keys.chars().enumerate() {
                let min = origin + vec2(column as f32 * (key + gap), row as f32 * (key + gap));
                let focused = self.row == row && self.column == column;
                draw(
                    Rect::from_min_size(min, vec2(key, key)),
                    &c.to_string(),
                    focused,
                );
            }
        }
        let wide = (width - 2.0 * gap) / 3.0;
        for (column, label) in labels.iter().enumerate() {
            let min = origin + vec2(column as f32 * (wide + gap), 4.0 * (key + gap));
            let focused = self.row == ROWS.len() && self.column == column;
            draw(Rect::from_min_size(min, vec2(wide, key)), label, focused);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_moving_and_deleting() {
        let mut keyboard = Keyboard::default();
        let mut text = String::new();

        // Row 1 starts with q; Right twice is e.
        keyboard.handle(Nav::Down, &mut text);
        keyboard.handle(Nav::Right, &mut text);
        keyboard.handle(Nav::Right, &mut text);
        keyboard.handle(Nav::Accept, &mut text);
        keyboard.handle(Nav::Search, &mut text);
        assert_eq!(text, "e ");

        keyboard.handle(Nav::Action, &mut text);
        assert_eq!(text, "e");

        // Left from the first column wraps to the last.
        keyboard.handle(Nav::Up, &mut text);
        keyboard.handle(Nav::Left, &mut text);
        keyboard.handle(Nav::Left, &mut text);
        keyboard.handle(Nav::Left, &mut text);
        keyboard.handle(Nav::Accept, &mut text);
        assert_eq!(text, "e0");
    }

    #[test]
    fn the_last_row_spaces_deletes_and_finishes() {
        let mut keyboard = Keyboard::default();
        let mut text = "ab".to_string();
        keyboard.handle(Nav::Up, &mut text);
        assert_eq!(keyboard.row, ROWS.len());

        keyboard.handle(Nav::Accept, &mut text);
        assert_eq!(text, "ab ");
        keyboard.handle(Nav::Right, &mut text);
        keyboard.handle(Nav::Accept, &mut text);
        assert_eq!(text, "ab");
        keyboard.handle(Nav::Right, &mut text);
        assert_eq!(keyboard.handle(Nav::Accept, &mut text), Outcome::Done);
        assert_eq!(keyboard.handle(Nav::Back, &mut text), Outcome::Done);
    }
}
