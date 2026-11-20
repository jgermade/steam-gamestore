//! Small icons drawn from shapes: which build a game is, and controller buttons.
//! Shapes rather than images, so they are sharp at any size on any screen.

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Stroke, StrokeKind, pos2, vec2};
use gamestore_core::catalog::Os;

/// The build's mark inside `rect`: Windows' four panes, or a small Tux.
pub fn os(painter: &egui::Painter, rect: Rect, os: Os) {
    let size = rect.width().min(rect.height());
    let square = Rect::from_center_size(rect.center(), vec2(size, size));
    match os {
        Os::Windows => {
            let gap = size * 0.08;
            let pane = (size - gap) / 2.0;
            let blue = Color32::from_rgb(0, 164, 239);
            for (dx, dy) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
                let min = square.min + vec2(dx * (pane + gap), dy * (pane + gap));
                painter.rect_filled(
                    Rect::from_min_size(min, vec2(pane, pane)),
                    size * 0.04,
                    blue,
                );
            }
        }
        Os::Linux | Os::Mac => tux(painter, square),
    }
}

fn tux(painter: &egui::Painter, square: Rect) {
    let s = square.width();
    let at = |x: f32, y: f32| square.min + vec2(x * s, y * s);
    let black = Color32::from_rgb(20, 20, 24);
    let white = Color32::from_rgb(245, 245, 245);
    let orange = Color32::from_rgb(255, 176, 32);

    // Feet first, so the body sits on them.
    for x in [0.33, 0.67] {
        painter.add(egui::Shape::ellipse_filled(
            at(x, 0.92),
            vec2(s * 0.17, s * 0.07),
            orange,
        ));
    }
    // Body and head.
    painter.add(egui::Shape::ellipse_filled(
        at(0.5, 0.6),
        vec2(s * 0.33, s * 0.36),
        black,
    ));
    painter.circle_filled(at(0.5, 0.26), s * 0.22, black);
    // Belly.
    painter.add(egui::Shape::ellipse_filled(
        at(0.5, 0.66),
        vec2(s * 0.22, s * 0.27),
        white,
    ));
    // Eyes.
    for x in [0.42, 0.58] {
        painter.circle_filled(at(x, 0.22), s * 0.065, white);
        painter.circle_filled(at(x + 0.01, 0.23), s * 0.03, black);
    }
    // Beak.
    painter.add(egui::Shape::convex_polygon(
        vec![at(0.4, 0.32), at(0.6, 0.32), at(0.5, 0.42)],
        orange,
        Stroke::NONE,
    ));
}

/// How the controller's face buttons look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadStyle {
    /// Xbox colours: A green, B red, X blue, Y yellow. Steam's virtual pad is an
    /// Xbox 360 pad, so this is also what Steam Input looks like.
    Xbox,
    /// Plain light chips.
    Plain,
}

/// A controller button's chip at `left_center`; returns the space it took.
pub fn button(
    painter: &egui::Painter,
    left_center: Pos2,
    label: &str,
    style: PadStyle,
    font: &FontId,
) -> Rect {
    let face = match (style, label) {
        (PadStyle::Xbox, "A") => Some(Color32::from_rgb(96, 196, 48)),
        (PadStyle::Xbox, "B") => Some(Color32::from_rgb(232, 17, 35)),
        (PadStyle::Xbox, "X") => Some(Color32::from_rgb(0, 120, 215)),
        (PadStyle::Xbox, "Y") => Some(Color32::from_rgb(255, 185, 0)),
        _ => None,
    };
    let galley = painter.layout_no_wrap(label.to_string(), font.clone(), Color32::BLACK);
    let height = galley.size().y + 8.0;
    // Arrows are drawn: the UI font has ◀▶ but not ▲▼, and a missing glyph
    // shows as an empty box.
    let arrows: Option<[f32; 2]> = match label {
        "◀▶" => Some([180.0, 0.0]),
        "▲▼" => Some([270.0, 90.0]),
        _ => None,
    };
    if let Some(angles) = arrows {
        let size = height * 0.42;
        let rect = Rect::from_min_size(
            pos2(left_center.x, left_center.y - height / 2.0),
            vec2(size * 2.0 + 30.0, height),
        );
        painter.rect_filled(rect, 8.0, Color32::from_rgb(236, 236, 242));
        for (index, angle) in angles.iter().enumerate() {
            let center = pos2(
                rect.min.x + 12.0 + size / 2.0 + index as f32 * (size + 6.0),
                rect.center().y,
            );
            let (sin, cos) = angle.to_radians().sin_cos();
            let point = |forward: f32, side: f32| {
                center + vec2(cos * forward - sin * side, sin * forward + cos * side) * size / 2.0
            };
            painter.add(egui::Shape::convex_polygon(
                vec![point(1.0, 0.0), point(-0.8, 0.95), point(-0.8, -0.95)],
                Color32::BLACK,
                Stroke::NONE,
            ));
        }
        return rect;
    }
    match face {
        Some(colour) => {
            // A round face button, in the controller's colour.
            let radius = height / 2.0;
            let center = left_center + vec2(radius, 0.0);
            painter.circle_filled(center, radius, colour);
            painter.text(
                center,
                Align2::CENTER_CENTER,
                label,
                font.clone(),
                Color32::WHITE,
            );
            Rect::from_center_size(center, vec2(height, height))
        }
        None => {
            let rect = Rect::from_min_size(
                pos2(left_center.x, left_center.y - height / 2.0),
                vec2(galley.size().x + 16.0, height),
            );
            painter.rect_filled(rect, 8.0, Color32::from_rgb(236, 236, 242));
            painter.rect_stroke(rect, 8.0, Stroke::NONE, StrokeKind::Inside);
            painter.galley(rect.min + vec2(8.0, 4.0), galley, Color32::BLACK);
            rect
        }
    }
}
