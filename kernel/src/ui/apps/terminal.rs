//! The Terminal: shows the console and types into the shell.

use alloc::string::String;
use alloc::vec;
use crate::arch::timer;
use crate::drivers::input::Key;
use crate::drivers::uart::feed_input;
use crate::ui::apps::App;
use crate::ui::console::{self, COLS};
use crate::ui::gfx::{Rect, Surface, GLYPH_W};

/// Columns and rows when it opens.
const START_COLS: i32 = 96;
const START_ROWS: i32 = 28;
const CELL_W: i32 = GLYPH_W;
const CELL_H: i32 = 16;
const PAD: i32 = 4;
const BG: u32 = 0x00101418;
const FG: u32 = 0x00d8d8d8;
const CURSOR: u32 = 0x0060c060;

pub struct Terminal {
    seen_generation: u64,
    cursor_visible: bool,
    last_blink: u64,
    /// Size of the window's content area.
    w: i32,
    h: i32,
}

impl Terminal {
    pub fn new() -> Self {
        Self {
            seen_generation: u64::MAX,
            cursor_visible: true,
            last_blink: 0,
            w: START_COLS * CELL_W + 2 * PAD,
            h: START_ROWS * CELL_H + 2 * PAD,
        }
    }

    /// Columns shown: as many as fit, up to the console's width.
    fn cols(&self) -> usize {
        (((self.w - 2 * PAD) / CELL_W).max(1) as usize).min(COLS)
    }

    fn rows(&self) -> usize {
        ((self.h - 2 * PAD) / CELL_H).max(1) as usize
    }
}

impl App for Terminal {
    fn title(&self) -> String {
        String::from("Terminal")
    }

    fn size(&self) -> (i32, i32) {
        (self.w, self.h)
    }

    fn resizable(&self) -> bool {
        true
    }

    fn min_size(&self) -> (i32, i32) {
        (20 * CELL_W + 2 * PAD, 4 * CELL_H + 2 * PAD)
    }

    fn resize(&mut self, w: i32, h: i32) {
        self.w = w;
        self.h = h;
    }

    fn draw(&mut self, surface: &mut Surface) {
        let (cols, rows) = (self.cols(), self.rows());
        let mut lines = vec![[0u8; COLS]; rows];
        let mut lens = vec![0usize; rows];
        let (count, cursor_row, cursor_col) = console::tail(&mut lines, &mut lens);
        self.seen_generation = console::generation();
        surface.fill(BG);
        for row in 0..count {
            let y = PAD + row as i32 * CELL_H;
            for (col, &b) in lines[row][..lens[row].min(cols)].iter().enumerate() {
                if b != b' ' {
                    surface.draw_char(PAD + col as i32 * CELL_W, y, b as char, FG, 1, 2);
                }
            }
        }
        if self.cursor_visible && cursor_col < cols {
            let x = PAD + cursor_col as i32 * CELL_W;
            let y = PAD + cursor_row as i32 * CELL_H;
            surface.fill_rect(Rect::new(x, y + CELL_H - 3, CELL_W, 2), CURSOR);
        }
    }

    fn on_key(&mut self, key: Key, ctrl: bool) -> bool {
        match key {
            // Ctrl+letter is the letter's control code: Ctrl+C is 0x03.
            Key::Char(c) if ctrl && c.is_ascii_alphabetic() => feed_input(c.to_ascii_lowercase() as u8 - b'a' + 1),
            Key::Char(c) => feed_input(c as u8),
            Key::Enter => feed_input(b'\r'),
            Key::Backspace => feed_input(0x7f),
            _ => {}
        }
        false
    }

    fn tick(&mut self) -> bool {
        let now = timer::uptime_ms();
        let mut changed = console::generation() != self.seen_generation;
        if now - self.last_blink >= 500 {
            self.last_blink = now;
            self.cursor_visible = !self.cursor_visible;
            changed = true;
        }
        changed
    }
}
