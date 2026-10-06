//! Gamepad and keyboard, as one stream of navigation actions.
//!
//! Both feed the same [`Nav`] values, so everything the UI does can be driven
//! from a physical keyboard too, which is how it gets tested away from the TV.
//!
//! Under Steam Input, Steam presents a virtual pad and the physical one stays
//! visible to evdev, so every press would arrive twice. When a pad Steam created
//! is connected, only that one is listened to.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use eframe::egui::{self, Key};
use gilrs::{Axis, Button, EventType, GamepadId, Gilrs};

/// What a press means, whatever it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Nav {
    Up,
    Down,
    Left,
    Right,
    /// A, or Enter.
    Accept,
    /// B, or Escape.
    Back,
    /// X, or Backspace.
    Action,
    /// Y, or Tab.
    Search,
    /// LB, or Page Up.
    Previous,
    /// RB, or Page Down.
    Next,
    /// Start, or F10.
    Menu,
}

impl Nav {
    fn repeats(self) -> bool {
        matches!(
            self,
            Nav::Up | Nav::Down | Nav::Left | Nav::Right | Nav::Action
        )
    }
}

/// Wait before the first repeat, and between repeats after that.
const FIRST_REPEAT: Duration = Duration::from_millis(380);
const REPEAT: Duration = Duration::from_millis(85);
/// How far a stick has to lean to count as a direction, and how far back it has
/// to come to let go: the gap keeps a stick resting near the edge from chattering.
const PRESS: f32 = 0.6;
const RELEASE: f32 = 0.35;

pub struct Input {
    gilrs: Option<Gilrs>,
    /// Held directions and buttons, with when they next repeat.
    held: HashMap<Nav, Instant>,
    /// Which way each stick axis is currently leaning.
    stick_x: i8,
    stick_y: i8,
}

impl Input {
    pub fn new() -> Self {
        Self {
            gilrs: Gilrs::new().ok(),
            held: HashMap::new(),
            stick_x: 0,
            stick_y: 0,
        }
    }

    /// How the face buttons should be drawn: in Xbox colours for an Xbox pad,
    /// or for Steam's virtual pad, which is an Xbox 360 one.
    pub fn style(&self) -> crate::icons::PadStyle {
        let xbox = self.gilrs.as_ref().is_some_and(|gilrs| {
            gilrs.gamepads().any(|(_, pad)| {
                let name = pad.name().to_ascii_lowercase();
                ["xbox", "x-box", "microsoft", "steam", "xinput"]
                    .iter()
                    .any(|mark| name.contains(mark))
            })
        });
        if xbox {
            crate::icons::PadStyle::Xbox
        } else {
            crate::icons::PadStyle::Plain
        }
    }

    /// The pads to listen to: Steam's virtual ones when there are any.
    fn listened(gilrs: &Gilrs, id: GamepadId) -> bool {
        let steam = |name: &str| name.to_ascii_lowercase().contains("steam");
        let any_steam = gilrs.gamepads().any(|(_, pad)| steam(pad.name()));
        !any_steam || steam(gilrs.gamepad(id).name())
    }

    /// Everything pressed since the last call, including repeats of held keys.
    pub fn poll(&mut self, ctx: &egui::Context) -> Vec<Nav> {
        let mut out = Vec::new();
        let now = Instant::now();

        if let Some(gilrs) = self.gilrs.as_mut() {
            while let Some(event) = gilrs.next_event() {
                if !Self::listened(gilrs, event.id) {
                    continue;
                }
                match event.event {
                    EventType::ButtonPressed(button, _) => {
                        if let Some(nav) = button_nav(button) {
                            out.push(nav);
                            if nav.repeats() {
                                self.held.insert(nav, now + FIRST_REPEAT);
                            }
                        }
                    }
                    EventType::ButtonReleased(button, _) => {
                        if let Some(nav) = button_nav(button) {
                            self.held.remove(&nav);
                        }
                    }
                    EventType::AxisChanged(axis, value, _) => {
                        let (state, positive, negative) = match axis {
                            Axis::LeftStickX => (&mut self.stick_x, Nav::Right, Nav::Left),
                            // gilrs reports up as positive.
                            Axis::LeftStickY => (&mut self.stick_y, Nav::Up, Nav::Down),
                            _ => continue,
                        };
                        let leaning = if value > PRESS {
                            1
                        } else if value < -PRESS {
                            -1
                        } else if value.abs() < RELEASE {
                            0
                        } else {
                            *state
                        };
                        if leaning != *state {
                            for nav in [positive, negative] {
                                self.held.remove(&nav);
                            }
                            if leaning != 0 {
                                let nav = if leaning > 0 { positive } else { negative };
                                out.push(nav);
                                self.held.insert(nav, now + FIRST_REPEAT);
                            }
                            *state = leaning;
                        }
                    }
                    EventType::Disconnected => {
                        self.held.clear();
                        self.stick_x = 0;
                        self.stick_y = 0;
                    }
                    _ => {}
                }
            }
        }

        for (nav, next) in self.held.iter_mut() {
            if now >= *next {
                out.push(*nav);
                *next = now + REPEAT;
            }
        }

        // The keyboard repeats on its own, through the system's key repeat.
        ctx.input(|input| {
            for (key, nav) in [
                (Key::ArrowUp, Nav::Up),
                (Key::ArrowDown, Nav::Down),
                (Key::ArrowLeft, Nav::Left),
                (Key::ArrowRight, Nav::Right),
                (Key::Enter, Nav::Accept),
                (Key::Escape, Nav::Back),
                (Key::Backspace, Nav::Action),
                (Key::Tab, Nav::Search),
                (Key::PageUp, Nav::Previous),
                (Key::PageDown, Nav::Next),
                (Key::F10, Nav::Menu),
            ] {
                if input.key_pressed(key) {
                    out.push(nav);
                }
            }
        });

        out
    }

    /// Whether anything is held, so the UI keeps repainting to deliver repeats.
    pub fn busy(&self) -> bool {
        !self.held.is_empty()
    }
}

fn button_nav(button: Button) -> Option<Nav> {
    Some(match button {
        Button::DPadUp => Nav::Up,
        Button::DPadDown => Nav::Down,
        Button::DPadLeft => Nav::Left,
        Button::DPadRight => Nav::Right,
        Button::South => Nav::Accept,
        Button::East => Nav::Back,
        Button::West => Nav::Action,
        Button::North => Nav::Search,
        Button::LeftTrigger => Nav::Previous,
        Button::RightTrigger => Nav::Next,
        Button::Start => Nav::Menu,
        _ => return None,
    })
}
