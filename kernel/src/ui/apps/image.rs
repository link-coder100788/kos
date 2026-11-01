//! Shows a picture. Pictures bigger than the window scroll: use the mouse
//! wheel, the arrow keys, Page Up / Page Down, Home / End, or drag it.

use alloc::string::String;
use crate::drivers::input::{Key, MouseButton};
use crate::ui::apps::{App, MouseEvent};
use crate::ui::gfx::{Rect, Surface};

const MAX_W: i32 = 960;
const MAX_H: i32 = 640;
const SCROLLBAR_W: i32 = 8;
/// Pixels moved per arrow key press or wheel click.
const STEP: i32 = 48;

pub struct ImageViewer {
    name: String,
    image: Surface,
    /// Size of the window's content area.
    w: i32,
    h: i32,
    /// Image pixel shown at the top-left of the window.
    scroll_x: i32,
    scroll_y: i32,
    /// Last mouse position while dragging the image around.
    drag: Option<(i32, i32)>,
}

impl ImageViewer {
    pub fn new(name: String, image: Surface) -> Self {
        let w = image.width.clamp(120, MAX_W);
        let h = image.height.clamp(60, MAX_H);
        Self { name, image, w, h, scroll_x: 0, scroll_y: 0, drag: None }
    }

    fn max_scroll(&self) -> (i32, i32) {
        ((self.image.width - self.w).max(0), (self.image.height - self.h).max(0))
    }

    fn scroll_by(&mut self, dx: i32, dy: i32) -> bool {
        let (max_x, max_y) = self.max_scroll();
        let x = (self.scroll_x + dx).clamp(0, max_x);
        let y = (self.scroll_y + dy).clamp(0, max_y);
        let changed = (x, y) != (self.scroll_x, self.scroll_y);
        self.scroll_x = x;
        self.scroll_y = y;
        changed
    }
}

impl App for ImageViewer {
    fn title(&self) -> String {
        self.name.clone()
    }

    fn size(&self) -> (i32, i32) {
        (self.w, self.h)
    }

    fn resizable(&self) -> bool {
        true
    }

    fn min_size(&self) -> (i32, i32) {
        (120, 60)
    }

    fn resize(&mut self, w: i32, h: i32) {
        self.w = w;
        self.h = h;
        // Keep the scroll position inside the new bounds.
        self.scroll_by(0, 0);
    }

    fn draw(&mut self, s: &mut Surface) {
        s.fill(0x00303030);
        s.blit(&self.image, -self.scroll_x, -self.scroll_y);
        let (max_x, max_y) = self.max_scroll();
        if max_y > 0 {
            let track = Rect::new(self.w - SCROLLBAR_W - 2, 2, SCROLLBAR_W, self.h - 4);
            s.fill_rect(track, 0x00e0e0e0);
            let thumb_h = (track.h * self.h / self.image.height).max(16);
            let thumb_y = track.y + (track.h - thumb_h) * self.scroll_y / max_y;
            s.fill_rect(Rect::new(track.x, thumb_y, SCROLLBAR_W, thumb_h), 0x00909090);
        }
        if max_x > 0 {
            // Stop short of the vertical bar, if there is one.
            let right = if max_y > 0 { SCROLLBAR_W + 4 } else { 2 };
            let track = Rect::new(2, self.h - SCROLLBAR_W - 2, self.w - 2 - right, SCROLLBAR_W);
            s.fill_rect(track, 0x00e0e0e0);
            let thumb_w = (track.w * self.w / self.image.width).max(16);
            let thumb_x = track.x + (track.w - thumb_w) * self.scroll_x / max_x;
            s.fill_rect(Rect::new(thumb_x, track.y, thumb_w, SCROLLBAR_W), 0x00909090);
        }
    }

    fn on_key(&mut self, key: Key, _ctrl: bool) -> bool {
        let (max_x, max_y) = self.max_scroll();
        match key {
            Key::Up => self.scroll_by(0, -STEP),
            Key::Down => self.scroll_by(0, STEP),
            Key::Left => self.scroll_by(-STEP, 0),
            Key::Right => self.scroll_by(STEP, 0),
            Key::PageUp => self.scroll_by(0, -(self.h - STEP)),
            Key::PageDown => self.scroll_by(0, self.h - STEP),
            Key::Home => self.scroll_by(-max_x, -max_y),
            Key::End => self.scroll_by(max_x, max_y),
            _ => false,
        }
    }

    fn on_mouse(&mut self, event: MouseEvent) -> bool {
        match event {
            MouseEvent::Down { x, y, button: MouseButton::Left } => {
                self.drag = Some((x, y));
                false
            }
            MouseEvent::Move { x, y, dragging: true } => {
                let Some((last_x, last_y)) = self.drag else { return false };
                self.drag = Some((x, y));
                self.scroll_by(last_x - x, last_y - y)
            }
            MouseEvent::Up { button: MouseButton::Left, .. } => {
                self.drag = None;
                false
            }
            MouseEvent::Scroll(clicks) => self.scroll_by(0, -clicks * STEP),
            _ => false,
        }
    }
}
