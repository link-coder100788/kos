//! Window contents. Each window on the desktop is driven by an `App`,
//! which draws into the window's own surface and reacts to input.

pub mod about;
pub mod browser;
pub mod canvas;
pub mod image;
pub mod paint;
pub mod programs;
pub mod taskmgr;
pub mod terminal;
pub mod video;
pub mod widgets;

use alloc::string::String;
use crate::drivers::input::{Key, MouseButton};
use crate::ui::gfx::Surface;

/// Mouse input, in coordinates relative to the window's content area.
#[derive(Clone, Copy, Debug)]
pub enum MouseEvent {
    Down { x: i32, y: i32, button: MouseButton },
    Up { x: i32, y: i32, button: MouseButton },
    /// `dragging`: a button was pressed inside this window and is still down.
    Move { x: i32, y: i32, dragging: bool },
    Scroll(i32),
}

/// Every method that returns `bool` returns whether the content changed
/// and the window needs to be redrawn.
pub trait App: Send {
    fn title(&self) -> String;

    /// Size of the content area when the window opens.
    fn size(&self) -> (i32, i32);

    /// Whether the user can resize (and maximize) the window. If so,
    /// `resize` is called with each new size, before the next `draw`.
    fn resizable(&self) -> bool { false }

    /// The smallest content area the app still works in.
    fn min_size(&self) -> (i32, i32) { (160, 80) }

    /// The content area is now `w` x `h`.
    fn resize(&mut self, _w: i32, _h: i32) {}

    /// Draws the whole content area.
    fn draw(&mut self, surface: &mut Surface);

    fn on_key(&mut self, _key: Key, _ctrl: bool) -> bool { false }

    fn on_mouse(&mut self, _event: MouseEvent) -> bool { false }

    /// Called about 30 times a second.
    fn tick(&mut self) -> bool { false }

    /// Called when the user closes the window.
    fn on_close(&mut self) {}
}

pub const BG: u32 = 0x00f0f0f0;
pub const FG: u32 = 0x00202020;
pub const ACCENT: u32 = 0x003a6ea5;

/// Draws a push button with its label centered.
pub fn draw_button(surface: &mut Surface, rect: crate::ui::gfx::Rect, label: &str, highlighted: bool) {
    use crate::ui::gfx::{text_width, GLYPH_H};
    surface.fill_gradient(rect, 0x00ffffff, 0x00d8dce4);
    surface.rect_outline(rect, if highlighted { ACCENT } else { 0x008a8f99 });
    if highlighted {
        surface.rect_outline(rect.inset(1), ACCENT);
    }
    let tx = rect.x + (rect.w - text_width(label, 1)) / 2;
    let ty = rect.y + (rect.h - GLYPH_H * 2) / 2;
    surface.draw_text(tx, ty, label, FG, 1, 2);
}
