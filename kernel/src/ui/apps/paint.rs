//! A tiny paint program: pick a color, draw with the mouse. "Read" reads
//! the digits drawn, with the recogniser trained in `tools/digitclass`.

use alloc::format;
use alloc::string::String;
use crate::drivers::input::{Key, MouseButton};
use crate::ui::apps::{draw_button, App, MouseEvent};
use crate::ui::gfx::{text_width, Rect, Surface, GLYPH_H};

/// Size when it opens.
const START_W: i32 = 480;
const START_H: i32 = 360;
const BAR_H: i32 = 32;
const SWATCH: i32 = 24;
const PALETTE: [u32; 8] = [0x00000000, 0x00ffffff, 0x00e53935, 0x00fb8c00, 0x00fdd835, 0x0043a047, 0x001e88e5, 0x008e24aa];
const WHITE: u32 = 0x00ffffff;
/// Below this, a digit shows as "?".
const SURE: f32 = 0.7;

pub struct Paint {
    /// The drawing, below the bar. It's as big as the window has ever
    /// been, so making the window smaller and then bigger loses nothing.
    canvas: Surface,
    color: u32,
    last: Option<(i32, i32)>,
    /// What "Read" made of the drawing, shown until it changes.
    reading: String,
    /// Size of the window's content area.
    w: i32,
    h: i32,
}

impl Paint {
    pub fn new() -> Self {
        Self {
            canvas: Surface::new(START_W, START_H - BAR_H, WHITE),
            color: PALETTE[0],
            last: None,
            reading: String::new(),
            w: START_W,
            h: START_H,
        }
    }

    fn clear_rect(&self) -> Rect {
        Rect::new(self.w - 72, 4, 64, BAR_H - 8)
    }

    fn read_rect(&self) -> Rect {
        Rect::new(self.w - 144, 4, 64, BAR_H - 8)
    }

    /// Reads the digits on the canvas: any pixel that isn't white is ink.
    fn read(&self) -> String {
        let model = match crate::assistant::digits() {
            Ok(model) => model,
            Err(e) => {
                log::warn!("paint: {}", e);
                return String::from("Can't read: no digit model on the disk");
            }
        };
        let (w, h) = (self.canvas.width as usize, self.canvas.height as usize);
        let ink: alloc::vec::Vec<f32> = self.canvas.pixels.iter().map(|&p| if p == WHITE { 0.0 } else { 1.0 }).collect();
        let started = crate::arch::timer::uptime_ms();
        let readings = model.read_all(&ink, w, h);
        readings.iter().enumerate().for_each(|r| log::debug!("paint: digit {}: {}", r.0, r.1));
        log::debug!("paint: read {} digits in {} ms", readings.len(), crate::arch::timer::uptime_ms() - started);
        match readings.as_slice() {
            [] => String::from("Draw a digit first"),
            [one] if one.confidence < SURE => format!("{} or {}?", one.digit, one.second.0),
            all => {
                let text: String = all.iter().map(|r| if r.confidence < SURE { '?' } else { (b'0' + r.digit) as char }).collect();
                if text.contains('?') { format!("Number: {} (? = not sure)", text) } else { format!("Number: {}", text) }
            }
        }
    }

    fn swatch_rect(i: usize) -> Rect {
        Rect::new(8 + i as i32 * (SWATCH + 6), 4, SWATCH, SWATCH)
    }
}

impl App for Paint {
    fn title(&self) -> String {
        String::from("Paint")
    }

    fn size(&self) -> (i32, i32) {
        (self.w, self.h)
    }

    fn resizable(&self) -> bool {
        true
    }

    fn min_size(&self) -> (i32, i32) {
        // The swatches and the Read and Clear buttons.
        (Self::swatch_rect(PALETTE.len()).x + 152, BAR_H + 40)
    }

    /// Keeps the drawing where it is, growing it (with white) to fill the
    /// window if need be.
    fn resize(&mut self, w: i32, h: i32) {
        self.w = w;
        self.h = h;
        let (cw, ch) = (w.max(self.canvas.width), (h - BAR_H).max(self.canvas.height));
        if (cw, ch) != (self.canvas.width, self.canvas.height) {
            let mut canvas = Surface::new(cw, ch, WHITE);
            canvas.blit(&self.canvas, 0, 0);
            self.canvas = canvas;
        }
    }

    fn draw(&mut self, s: &mut Surface) {
        s.fill_rect(Rect::new(0, 0, self.w, BAR_H), 0x00e4e6ea);
        for (i, &c) in PALETTE.iter().enumerate() {
            let r = Self::swatch_rect(i);
            s.fill_rect(r, c);
            s.rect_outline(r, if c == self.color { 0x00ff00ff } else { 0x00808080 });
            if c == self.color {
                s.rect_outline(r.inset(1), 0x00ff00ff);
            }
        }
        draw_button(s, self.read_rect(), "Read", false);
        draw_button(s, self.clear_rect(), "Clear", false);
        s.blit(&self.canvas, 0, BAR_H);
        if !self.reading.is_empty() {
            let label = Rect::new(8, self.h - GLYPH_H * 2 - 16, text_width(&self.reading, 1) + 16, GLYPH_H * 2 + 8);
            s.fill_rect(label, 0x00fff8c4);
            s.rect_outline(label, 0x008a8f99);
            s.draw_text(label.x + 8, label.y + 4, &self.reading, 0x00000000, 1, 2);
        }
    }

    fn on_key(&mut self, key: Key, _ctrl: bool) -> bool {
        if key == Key::Char('c') {
            self.canvas.fill(WHITE);
            self.reading.clear();
            return true;
        }
        false
    }

    fn on_mouse(&mut self, event: MouseEvent) -> bool {
        match event {
            MouseEvent::Down { x, y, button: MouseButton::Left } => {
                if y < BAR_H {
                    if self.clear_rect().contains(x, y) {
                        self.canvas.fill(WHITE);
                        self.reading.clear();
                        return true;
                    }
                    if self.read_rect().contains(x, y) {
                        self.reading = self.read();
                        return true;
                    }
                    if let Some(i) = (0..PALETTE.len()).find(|&i| Self::swatch_rect(i).contains(x, y)) {
                        self.color = PALETTE[i];
                        return true;
                    }
                    return false;
                }
                self.last = Some((x, y - BAR_H));
                self.reading.clear();
                self.canvas.line(x, y - BAR_H, x, y - BAR_H, 4, self.color);
                true
            }
            MouseEvent::Move { x, y, dragging: true } => {
                let Some((lx, ly)) = self.last else { return false };
                self.canvas.line(lx, ly, x, y - BAR_H, 4, self.color);
                self.last = Some((x, y - BAR_H));
                true
            }
            MouseEvent::Up { .. } => {
                self.last = None;
                false
            }
            _ => false,
        }
    }
}
