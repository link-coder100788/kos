//! The Settings window: every global setting (see `crate::settings`), a
//! row each. Arrows (or Left/Right) step through a setting's values; text
//! settings are typed in and saved with Enter or by leaving the box. Each
//! change is saved and applied straight away.

use alloc::format;
use alloc::string::String;
use crate::drivers::input::{Key, MouseButton};
use crate::settings::{self, Kind, ALL};
use crate::ui::apps::{draw_button, App, MouseEvent, ACCENT, BG, FG};
use crate::ui::gfx::{text_width, Rect, Surface};

const W: i32 = 560;
const TOP: i32 = 62;
const ROW_H: i32 = 32;
const LABEL_X: i32 = 20;
const CONTROL_X: i32 = 260;
const CONTROL_W: i32 = W - 20 - CONTROL_X;
const ARROW_W: i32 = 26;
const H: i32 = TOP + ALL.len() as i32 * ROW_H + 70;
const RESET: Rect = Rect { x: W - 20 - 110, y: H - 40, w: 110, h: 28 };
const MUTED: u32 = 0x00707070;
const ERROR: u32 = 0x00b03030;
const SELECTED: u32 = 0x00dde6f2;

pub struct Settings {
    /// The highlighted row.
    selected: usize,
    /// Text being typed into the selected (text) row.
    editing: Option<String>,
    /// The last change's result, and whether it went wrong.
    message: (String, bool),
    /// `settings::generation()` when last drawn, to notice changes made
    /// elsewhere (like the console).
    seen: u32,
}

fn row(i: usize) -> Rect {
    Rect::new(10, TOP + i as i32 * ROW_H, W - 20, ROW_H - 2)
}

fn control(i: usize) -> Rect {
    Rect::new(CONTROL_X, row(i).y + 2, CONTROL_W, ROW_H - 6)
}

fn left_arrow(i: usize) -> Rect {
    let c = control(i);
    Rect::new(c.x, c.y, ARROW_W, c.h)
}

fn right_arrow(i: usize) -> Rect {
    let c = control(i);
    Rect::new(c.right() - ARROW_W, c.y, ARROW_W, c.h)
}

fn is_text(i: usize) -> bool {
    matches!(ALL[i].kind, Kind::Text { .. })
}

/// How a value is shown.
fn shown(i: usize, value: &str) -> String {
    match &ALL[i].kind {
        Kind::Number { unit, .. } => format!("{} {}", value, unit),
        Kind::Text { .. } if value.is_empty() => String::from("(none)"),
        _ => String::from(value),
    }
}

impl Settings {
    pub fn new() -> Self {
        Self { selected: 0, editing: None, message: (String::new(), false), seen: settings::generation() }
    }

    fn set(&mut self, i: usize, value: Option<&str>) {
        let key = ALL[i].key;
        self.message = match settings::set(key, value) {
            Ok(v) => (format!("{} is now {}", ALL[i].label, shown(i, &v)), false),
            Err(e) => (e, true),
        };
    }

    fn step(&mut self, i: usize, forward: bool) {
        let s = &ALL[i];
        if let Some(next) = settings::step(s, &settings::get(s.key), forward) {
            self.set(i, Some(&next));
        }
    }

    /// Saves what's been typed, if anything.
    fn finish_editing(&mut self) {
        if let Some(text) = self.editing.take() {
            if text != settings::get(ALL[self.selected].key) {
                self.set(self.selected, Some(&text));
            }
        }
    }

    fn select(&mut self, i: usize) {
        if i != self.selected {
            self.finish_editing();
            self.selected = i;
        }
    }
}

impl App for Settings {
    fn title(&self) -> String {
        String::from("Settings")
    }

    fn size(&self) -> (i32, i32) {
        (W, H)
    }

    fn draw(&mut self, s: &mut Surface) {
        self.seen = settings::generation();
        s.fill(BG);
        s.draw_text(20, 14, "Settings", ACCENT, 2, 3);
        s.draw_text(20, 42, "For everyone on this computer, kept in /.kos/settings", MUTED, 1, 1);
        for (i, setting) in ALL.iter().enumerate() {
            let r = row(i);
            if i == self.selected {
                s.fill_rect(r, SELECTED);
            }
            s.draw_text(LABEL_X, r.y + 8, setting.label, FG, 1, 2);
            let c = control(i);
            if is_text(i) {
                let editing = i == self.selected && self.editing.is_some();
                let value = self.editing.clone().filter(|_| editing).unwrap_or_else(|| settings::get(setting.key));
                s.fill_rect(c, 0x00ffffff);
                s.rect_outline(c, if editing { ACCENT } else { 0x00a0a0a0 });
                // The end of long text, so the caret stays in view.
                let fits = ((c.w - 16) / 8) as usize;
                let tail: String = value.chars().skip(value.chars().count().saturating_sub(fits)).collect();
                let (text, color) = if value.is_empty() && !editing { (shown(i, ""), MUTED) } else { (tail, FG) };
                let width = s.draw_text(c.x + 8, c.y + 5, &text, color, 1, 2);
                if editing {
                    s.fill_rect(Rect::new(c.x + 9 + width, c.y + 4, 2, c.h - 8), FG);
                }
            } else {
                draw_button(s, left_arrow(i), "<", false);
                draw_button(s, right_arrow(i), ">", false);
                let value = shown(i, &settings::get(setting.key));
                let x = c.x + (c.w - text_width(&value, 1)) / 2;
                s.draw_text(x, c.y + 5, &value, FG, 1, 2);
            }
        }
        let help = ALL[self.selected].help;
        s.draw_text(20, H - 62, help, MUTED, 1, 1);
        let (message, error) = &self.message;
        s.draw_text(20, H - 33, message, if *error { ERROR } else { ACCENT }, 1, 2);
        draw_button(s, RESET, "Reset all", false);
    }

    fn on_key(&mut self, key: Key, _ctrl: bool) -> bool {
        let i = self.selected;
        if let Some(text) = self.editing.as_mut() {
            match key {
                Key::Char(c) if (' '..='~').contains(&c) && text.len() < 200 => { text.push(c); return true; }
                Key::Backspace => { text.pop(); return true; }
                Key::Enter => { self.finish_editing(); return true; }
                Key::Escape => { self.editing = None; return true; }
                _ => {}
            }
        }
        match key {
            Key::Up => self.select((i + ALL.len() - 1) % ALL.len()),
            Key::Down | Key::Tab => self.select((i + 1) % ALL.len()),
            Key::Left if !is_text(i) => self.step(i, false),
            Key::Right | Key::Enter | Key::Char(' ') if !is_text(i) => self.step(i, true),
            // Typing into a text row starts editing it.
            Key::Enter | Key::Backspace if is_text(i) => self.editing = Some(settings::get(ALL[i].key)),
            Key::Char(c) if is_text(i) && (' '..='~').contains(&c) => {
                let mut text = settings::get(ALL[i].key);
                text.push(c);
                self.editing = Some(text);
            }
            _ => return false,
        }
        true
    }

    fn on_mouse(&mut self, event: MouseEvent) -> bool {
        let MouseEvent::Down { x, y, button: MouseButton::Left } = event else { return false };
        if RESET.contains(x, y) {
            self.editing = None;
            self.message = match settings::reset_all() {
                Ok(()) => (String::from("Every setting is back to its default"), false),
                Err(e) => (e, true),
            };
            return true;
        }
        let Some(i) = (0..ALL.len()).find(|&i| row(i).contains(x, y)) else { return false };
        self.select(i);
        if is_text(i) {
            if control(i).contains(x, y) && self.editing.is_none() {
                self.editing = Some(settings::get(ALL[i].key));
            }
        } else if left_arrow(i).contains(x, y) {
            self.step(i, false);
        } else if right_arrow(i).contains(x, y) || control(i).contains(x, y) {
            self.step(i, true);
        }
        true
    }

    fn tick(&mut self) -> bool {
        settings::generation() != self.seen
    }

    fn on_close(&mut self) {
        self.finish_editing();
    }
}
