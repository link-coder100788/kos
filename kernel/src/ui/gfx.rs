//! 2D drawing: rectangles and pixel surfaces.
//!
//! Colors are 0x00RRGGBB (the framebuffer's XRGB8888). Every drawing call
//! is clipped to the surface's `clip` rectangle, which the compositor uses
//! to redraw only the part of the screen that changed.

use alloc::vec;
use alloc::vec::Vec;
use crate::ui::font8x8::FONT8X8;

/// Glyph cell size at scale 1. Text can be drawn "tall" (8x16), which
/// reads much better at normal sizes.
pub const GLYPH_W: i32 = 8;
pub const GLYPH_H: i32 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub const EMPTY: Rect = Rect::new(0, 0, 0, 0);

    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    pub fn right(&self) -> i32 { self.x + self.w }
    pub fn bottom(&self) -> i32 { self.y + self.h }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    pub fn intersect(&self, other: Rect) -> Rect {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let r = self.right().min(other.right());
        let b = self.bottom().min(other.bottom());
        if r <= x || b <= y { Rect::EMPTY } else { Rect::new(x, y, r - x, b - y) }
    }

    /// The smallest rectangle covering both.
    pub fn union(&self, other: Rect) -> Rect {
        if self.is_empty() { return other; }
        if other.is_empty() { return *self; }
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let r = self.right().max(other.right());
        let b = self.bottom().max(other.bottom());
        Rect::new(x, y, r - x, b - y)
    }

    pub fn offset(&self, dx: i32, dy: i32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.w, self.h)
    }

    pub fn inset(&self, by: i32) -> Rect {
        Rect::new(self.x + by, self.y + by, self.w - 2 * by, self.h - 2 * by)
    }
}

pub struct Surface {
    pub width: i32,
    pub height: i32,
    pub pixels: Vec<u32>,
    /// Drawing outside this rectangle is discarded.
    pub clip: Rect,
}

impl Surface {
    pub fn new(width: i32, height: i32, color: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![color; (width * height) as usize],
            clip: Rect::new(0, 0, width, height),
        }
    }

    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }

    /// Restricts drawing to `rect` (within the surface) until `reset_clip`.
    pub fn set_clip(&mut self, rect: Rect) {
        self.clip = rect.intersect(self.bounds());
    }

    pub fn reset_clip(&mut self) {
        self.clip = self.bounds();
    }

    pub fn put(&mut self, x: i32, y: i32, color: u32) {
        if self.clip.contains(x, y) {
            self.pixels[(y * self.width + x) as usize] = color;
        }
    }

    pub fn get(&self, x: i32, y: i32) -> u32 {
        if self.bounds().contains(x, y) { self.pixels[(y * self.width + x) as usize] } else { 0 }
    }

    pub fn fill(&mut self, color: u32) {
        self.fill_rect(self.bounds(), color);
    }

    pub fn fill_rect(&mut self, rect: Rect, color: u32) {
        let r = rect.intersect(self.clip);
        if r.is_empty() { return; }
        for y in r.y..r.bottom() {
            let row = (y * self.width) as usize;
            self.pixels[row + r.x as usize..row + r.right() as usize].fill(color);
        }
    }

    /// A 1-pixel outline just inside `rect`.
    pub fn rect_outline(&mut self, rect: Rect, color: u32) {
        self.fill_rect(Rect::new(rect.x, rect.y, rect.w, 1), color);
        self.fill_rect(Rect::new(rect.x, rect.bottom() - 1, rect.w, 1), color);
        self.fill_rect(Rect::new(rect.x, rect.y, 1, rect.h), color);
        self.fill_rect(Rect::new(rect.right() - 1, rect.y, 1, rect.h), color);
    }

    /// A vertical gradient from `top` to `bottom` color.
    pub fn fill_gradient(&mut self, rect: Rect, top: u32, bottom: u32) {
        let lerp = |a: u32, b: u32, shift: u32, t: i32, n: i32| -> u32 {
            let a = ((a >> shift) & 0xff) as i32;
            let b = ((b >> shift) & 0xff) as i32;
            ((a + (b - a) * t / n.max(1)) as u32) << shift
        };
        for i in 0..rect.h {
            let c = lerp(top, bottom, 16, i, rect.h) | lerp(top, bottom, 8, i, rect.h) | lerp(top, bottom, 0, i, rect.h);
            self.fill_rect(Rect::new(rect.x, rect.y + i, rect.w, 1), c);
        }
    }

    /// Bresenham line, `thickness` pixels wide.
    pub fn line(&mut self, mut x0: i32, mut y0: i32, x1: i32, y1: i32, thickness: i32, color: u32) {
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        let half = thickness / 2;
        loop {
            self.fill_rect(Rect::new(x0 - half, y0 - half, thickness, thickness), color);
            if x0 == x1 && y0 == y1 { break; }
            let e2 = 2 * err;
            if e2 >= dy { err += dy; x0 += sx; }
            if e2 <= dx { err += dx; y0 += sy; }
        }
    }

    /// Draws one character with each font pixel `sx` x `sy` screen pixels.
    pub fn draw_char(&mut self, x: i32, y: i32, c: char, color: u32, sx: i32, sy: i32) {
        let glyph = FONT8X8[if (c as u32) < 128 { c as usize } else { b'?' as usize }];
        let cell = Rect::new(x, y, GLYPH_W * sx, GLYPH_H * sy);
        let visible = cell.intersect(self.clip);
        if visible.is_empty() { return; }
        if visible == cell && sx == 1 {
            // Fast path (terminal text): whole glyph visible, write pixels directly.
            for (row, &bits) in glyph.iter().enumerate() {
                if bits == 0 { continue; }
                for dy in 0..sy {
                    let start = ((y + row as i32 * sy + dy) * self.width + x) as usize;
                    let out = &mut self.pixels[start..start + 8];
                    for col in 0..8 {
                        if bits & (1 << col) != 0 {
                            out[col] = color;
                        }
                    }
                }
            }
            return;
        }
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..8 {
                if bits & (1 << col) != 0 {
                    self.fill_rect(Rect::new(x + col * sx, y + row as i32 * sy, sx, sy), color);
                }
            }
        }
    }

    /// Draws `text` on one line; returns its width in pixels.
    pub fn draw_text(&mut self, x: i32, y: i32, text: &str, color: u32, sx: i32, sy: i32) -> i32 {
        let mut cx = x;
        for c in text.chars() {
            self.draw_char(cx, y, c, color, sx, sy);
            cx += GLYPH_W * sx;
        }
        cx - x
    }

    /// Copies `src` onto this surface with its top-left at (`x`, `y`).
    pub fn blit(&mut self, src: &Surface, x: i32, y: i32) {
        let dst = Rect::new(x, y, src.width, src.height).intersect(self.clip);
        if dst.is_empty() { return; }
        for row in dst.y..dst.bottom() {
            let src_row = ((row - y) * src.width) as usize;
            let src_start = src_row + (dst.x - x) as usize;
            let dst_start = (row * self.width + dst.x) as usize;
            self.pixels[dst_start..dst_start + dst.w as usize]
                .copy_from_slice(&src.pixels[src_start..src_start + dst.w as usize]);
        }
    }
}

impl Surface {
    /// Copies `src` stretched (or shrunk) to fill `dst`, picking the
    /// nearest source pixel for each destination pixel.
    pub fn blit_scaled(&mut self, src: &Surface, dst: Rect) {
        if dst.w == src.width && dst.h == src.height {
            return self.blit(src, dst.x, dst.y);
        }
        let r = dst.intersect(self.clip);
        if r.is_empty() || src.width <= 0 || src.height <= 0 {
            return;
        }
        for y in r.y..r.bottom() {
            let sy = (y - dst.y) as i64 * src.height as i64 / dst.h as i64;
            let src_row = (sy * src.width as i64) as usize;
            let row = (y * self.width) as usize;
            for x in r.x..r.right() {
                let sx = ((x - dst.x) as i64 * src.width as i64 / dst.w as i64) as usize;
                self.pixels[row + x as usize] = src.pixels[src_row + sx];
            }
        }
    }
}

/// Width in pixels of `text` drawn with horizontal scale `sx`.
pub fn text_width(text: &str, sx: i32) -> i32 {
    text.chars().count() as i32 * GLYPH_W * sx
}

pub const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | b as u32
}
