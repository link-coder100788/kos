//! Lists the programs on the disk; click one to run it.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use crate::drivers::input::MouseButton;
use crate::filesys::fs_ipc::{fs_call, FsRequest, FsResponse};
use crate::ui::apps::{draw_button, App, MouseEvent, ACCENT, BG, FG};
use crate::ui::gfx::{Rect, Surface};

/// Size when it opens.
const START_W: i32 = 360;
const START_ROWS: usize = 12;
const ROW_H: i32 = 30;
const TOP: i32 = 44;
/// Below the rows: the status line.
const FOOTER_H: i32 = 30;

pub struct Programs {
    names: Vec<String>,
    hover: Option<usize>,
    status: String,
    /// Size of the window's content area.
    w: i32,
    h: i32,
}

impl Programs {
    pub fn new() -> Self {
        let mut programs = Self {
            names: Vec::new(),
            hover: None,
            status: String::new(),
            w: START_W,
            h: TOP + START_ROWS as i32 * ROW_H + FOOTER_H,
        };
        programs.refresh();
        programs
    }

    fn refresh(&mut self) {
        self.names = match fs_call(FsRequest::List { cwd: "/".to_string(), path: ".".to_string() }) {
            FsResponse::Listing(entries) => entries
                .into_iter()
                .filter(|e| !e.is_dir && e.name.ends_with(".elf.b64"))
                .map(|e| e.name)
                .collect(),
            _ => Vec::new(),
        };
        self.status = if self.names.is_empty() { "No programs found".to_string() } else { "Output appears in the Terminal".to_string() };
    }

    /// The programs that fit, from the top.
    fn shown(&self) -> usize {
        (((self.h - TOP - FOOTER_H) / ROW_H).max(0) as usize).min(self.names.len())
    }

    fn row_rect(&self, i: usize) -> Rect {
        Rect::new(10, TOP + i as i32 * ROW_H, self.w - 20, ROW_H - 4)
    }

    fn refresh_rect(&self) -> Rect {
        Rect::new(self.w - 100, 8, 90, 26)
    }
}

impl App for Programs {
    fn title(&self) -> String {
        String::from("Programs")
    }

    fn size(&self) -> (i32, i32) {
        (self.w, self.h)
    }

    fn resizable(&self) -> bool {
        true
    }

    fn min_size(&self) -> (i32, i32) {
        (240, TOP + 2 * ROW_H + FOOTER_H)
    }

    fn resize(&mut self, w: i32, h: i32) {
        self.w = w;
        self.h = h;
    }

    fn draw(&mut self, s: &mut Surface) {
        s.fill(BG);
        s.draw_text(12, 14, "Click to run", FG, 1, 2);
        draw_button(s, self.refresh_rect(), "Refresh", false);
        for (i, name) in self.names.iter().take(self.shown()).enumerate() {
            let r = self.row_rect(i);
            let hovered = self.hover == Some(i);
            s.fill_rect(r, if hovered { ACCENT } else { 0x00ffffff });
            s.rect_outline(r, 0x00c0c4cc);
            let label = name.trim_end_matches(".elf.b64");
            s.draw_text(r.x + 10, r.y + 5, label, if hovered { 0x00ffffff } else { FG }, 1, 2);
        }
        s.draw_text(12, self.h - 24, &self.status, 0x00606060, 1, 2);
    }

    fn on_mouse(&mut self, event: MouseEvent) -> bool {
        match event {
            MouseEvent::Move { x, y, .. } => {
                let hover = (0..self.shown()).find(|&i| self.row_rect(i).contains(x, y));
                let changed = hover != self.hover;
                self.hover = hover;
                changed
            }
            MouseEvent::Down { x, y, button: MouseButton::Left } => {
                if self.refresh_rect().contains(x, y) {
                    self.refresh();
                    return true;
                }
                if let Some(i) = (0..self.shown()).find(|&i| self.row_rect(i).contains(x, y)) {
                    crate::loader::run(self.names[i].clone());
                    self.status = format!("Started {}", self.names[i].trim_end_matches(".elf.b64"));
                    return true;
                }
                false
            }
            _ => false,
        }
    }
}
