//! The border of the button in focus: a gradient in tones close to the button's
//! own purple that turns slowly round it, so the focus is found at a glance from
//! across the room and stays calm to look at.

use eframe::egui::{self, Color32, Pos2, Rect, Stroke, StrokeKind, pos2, vec2};

use crate::theme;

/// Seconds for the gradient to turn once round.
const LAP: f32 = 4.0;

/// The gradient's tones, round the border and back to the first: the button's
/// purple, a pale lilac, a pink purple and a blue violet.
const TONES: [Color32; 4] = [
    theme::ACCENT,
    Color32::from_rgb(232, 210, 255),
    Color32::from_rgb(236, 150, 245),
    Color32::from_rgb(140, 120, 255),
];

/// Draw the focus border outside `rect`, whose corners have `radius`.
pub fn ring(painter: &egui::Painter, rect: Rect, radius: f32) {
    let time = painter.ctx().input(|input| input.time) as f32;

    // A soft glow, so the border reads from far away.
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(8.0, theme::ACCENT.gamma_multiply(0.3)),
        StrokeKind::Outside,
    );

    // The gradient: each short piece of the border takes the tone of where it
    // is, shifted with time, so the colours flow round.
    let path = outline(rect.expand(2.0), radius + 2.0, 3.0);
    let count = path.len();
    if count < 2 {
        return;
    }
    let turn = (time / LAP).fract();
    for index in 0..count {
        let at = (index as f32 / count as f32 + turn).fract();
        painter.line_segment(
            [path[index], path[(index + 1) % count]],
            Stroke::new(4.0, tone(at)),
        );
    }
    painter.ctx().request_repaint();
}

/// The gradient's colour at `at`, from 0 to 1 round the border.
fn tone(at: f32) -> Color32 {
    let scaled = at * TONES.len() as f32;
    let from = scaled.floor() as usize % TONES.len();
    let to = (from + 1) % TONES.len();
    // Eased, so each tone lingers a little before the next.
    let t = scaled.fract();
    let eased = t * t * (3.0 - 2.0 * t);
    TONES[from].lerp_to_gamma(TONES[to], eased)
}

/// Points every `spacing` pixels round a rounded rectangle, clockwise from the
/// top left.
fn outline(rect: Rect, radius: f32, spacing: f32) -> Vec<Pos2> {
    let radius = radius.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let corners = [
        (pos2(rect.max.x - radius, rect.min.y + radius), -90.0_f32),
        (pos2(rect.max.x - radius, rect.max.y - radius), 0.0),
        (pos2(rect.min.x + radius, rect.max.y - radius), 90.0),
        (pos2(rect.min.x + radius, rect.min.y + radius), 180.0),
    ];
    // A fine polyline first: each corner's arc, which joins the next with a
    // straight edge.
    let mut fine = Vec::new();
    for (center, start) in corners {
        for step in 0..=8 {
            let angle = (start + step as f32 * 90.0 / 8.0).to_radians();
            fine.push(center + vec2(angle.cos(), angle.sin()) * radius);
        }
    }
    fine.push(fine[0]);

    // Then evenly spaced along it, so the light moves at one speed.
    let mut points = vec![fine[0]];
    let mut carried = 0.0;
    for pair in fine.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        let length = from.distance(to);
        let mut along = spacing - carried;
        while along <= length {
            points.push(from + (to - from) * (along / length));
            along += spacing;
        }
        carried = length - (along - spacing);
    }
    points
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_outline_goes_round_evenly() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 60.0));
        let points = outline(rect, 14.0, 3.0);
        // The perimeter of a 200×60 rectangle with 14 px corners.
        let perimeter = 2.0 * (200.0 + 60.0) - 8.0 * 14.0 + std::f32::consts::TAU * 14.0;
        let expected = (perimeter / 3.0) as usize;
        assert!(
            points.len().abs_diff(expected) <= 2,
            "{} vs {expected}",
            points.len()
        );
        for pair in points.windows(2) {
            assert!(pair[0].distance(pair[1]) <= 3.01);
        }
    }
}
