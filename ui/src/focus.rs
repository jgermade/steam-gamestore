//! The border of the button in focus: a pale line in a breathing glow, with a
//! white light running round it, so the focus is found at a glance from across the room and
//! a still screen still shows which button A presses.

use eframe::egui::{self, Color32, Pos2, Rect, Stroke, StrokeKind, pos2, vec2};

use crate::theme;

/// Seconds for the light to go once round.
const LAP: f32 = 2.2;
/// How much of the border the light covers, as a fraction.
const TAIL: f32 = 0.28;
/// The tail is drawn in this many pieces, each fainter than the one before.
const PIECES: usize = 24;

/// Draw the focus border outside `rect`, whose corners have `radius`.
pub fn ring(painter: &egui::Painter, rect: Rect, radius: f32) {
    let time = painter.ctx().input(|input| input.time) as f32;

    // A soft glow that breathes, under a steady pale line.
    let breath = 0.5 + 0.5 * (time * std::f32::consts::TAU / 1.6).sin();
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(
            4.0 + 6.0 * breath,
            theme::ACCENT.gamma_multiply(0.25 + 0.2 * breath),
        ),
        StrokeKind::Outside,
    );
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(3.0, theme::ACCENT.lerp_to_gamma(Color32::WHITE, 0.45)),
        StrokeKind::Outside,
    );

    // The light: a white stretch of the line with a purple halo outside it,
    // fading behind its head. Drawn in a few continuous pieces, each a little
    // fainter, so the joins stay smooth.
    let lines = [
        (rect.expand(4.0), 10.0, theme::ACCENT),
        (rect.expand(1.5), 5.0, Color32::WHITE),
    ];
    for (around, width, colour) in lines {
        let path = outline(around, radius + (around.width() - rect.width()) / 2.0, 3.0);
        let count = path.len();
        if count < 2 {
            return;
        }
        let head = ((time / LAP).fract() * count as f32) as usize;
        let tail = ((count as f32 * TAIL) as usize).max(PIECES);
        let piece = tail / PIECES;
        for index in 0..PIECES {
            let fade = 1.0 - index as f32 / PIECES as f32;
            // One point of overlap with the next piece, so there is no gap.
            let points: Vec<Pos2> = (index * piece..=(index + 1) * piece)
                .map(|step| path[(head + count - step) % count])
                .collect();
            painter.add(egui::Shape::line(
                points,
                Stroke::new(width * (0.5 + 0.5 * fade), colour.gamma_multiply(fade)),
            ));
        }
    }
    painter.ctx().request_repaint();
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
