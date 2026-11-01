//! Canvas windows: a window a program draws pixels into (the canvas
//! syscalls; `userspace/libc/canvas.h` on the C side).
//!
//! The program draws whole frames in its own memory and presents each one;
//! presenting copies the frame here, and the window shows it on its next
//! tick. Mouse and keyboard input go the other way, into a queue the
//! program polls. The window (on the UI task) and the program (on its own
//! task) share both through a `Shared` behind a lock.

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use crate::drivers::input::{Key, MouseButton};
use crate::std::lock::irq_mutex::IrqMutex;
use crate::ui::apps::{App, MouseEvent};
use crate::ui::gfx::Surface;
use crate::ui::window::WindowId;

/// Canvas sizes are limited to what fits on the screen with a title bar.
pub const MAX_W: usize = 1000;
pub const MAX_H: usize = 700;
/// Events beyond this are dropped until the program catches up.
const MAX_EVENTS: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanvasEvent {
    MouseDown { x: i32, y: i32, button: u8 },
    MouseUp { x: i32, y: i32, button: u8 },
    MouseMove { x: i32, y: i32, buttons: u8 },
    Key(u32),
    Closed,
    /// The wheel turned: clicks, positive away from you.
    Scroll(i32),
}

struct Shared {
    width: usize,
    height: usize,
    /// The latest presented frame, 0x00RRGGBB.
    pixels: Vec<u32>,
    dirty: bool,
    events: VecDeque<CanvasEvent>,
    /// Mouse buttons held down, one bit each (left 1, right 2, middle 4).
    buttons: u8,
}

/// Canvases by window id, for the syscalls to find them.
static CANVASES: IrqMutex<BTreeMap<WindowId, Arc<IrqMutex<Shared>>>> = IrqMutex::new(BTreeMap::new());

struct CanvasApp {
    title: String,
    shared: Arc<IrqMutex<Shared>>,
}

impl Shared {
    fn push(&mut self, event: CanvasEvent) {
        if self.events.len() < MAX_EVENTS {
            self.events.push_back(event);
        }
    }
}

fn button_bit(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => 1,
        MouseButton::Right => 2,
        MouseButton::Middle => 4,
    }
}

/// Key codes for programs: ASCII for characters, Enter, Backspace, Tab and
/// Escape, Ctrl+letter as 1-26 like a terminal; 0x100 and up for keys
/// without one (see canvas.h).
fn key_code(key: Key, ctrl: bool) -> Option<u32> {
    Some(match key {
        Key::Char(c) if ctrl && c.is_ascii_alphabetic() => (c.to_ascii_lowercase() as u8 - b'a' + 1) as u32,
        Key::Char(c) => c as u32,
        Key::Enter => b'\n' as u32,
        Key::Backspace => 8,
        Key::Tab => b'\t' as u32,
        Key::Escape => 27,
        Key::Up => 0x100,
        Key::Down => 0x101,
        Key::Left => 0x102,
        Key::Right => 0x103,
        Key::Home => 0x104,
        Key::End => 0x105,
        Key::PageUp => 0x106,
        Key::PageDown => 0x107,
        Key::Delete => 0x108,
        _ => return None,
    })
}

impl App for CanvasApp {
    fn title(&self) -> String {
        self.title.clone()
    }

    fn size(&self) -> (i32, i32) {
        let s = self.shared.lock();
        (s.width as i32, s.height as i32)
    }

    fn draw(&mut self, surface: &mut Surface) {
        let s = self.shared.lock();
        if surface.pixels.len() == s.pixels.len() {
            surface.pixels.copy_from_slice(&s.pixels);
        }
    }

    fn on_key(&mut self, key: Key, ctrl: bool) -> bool {
        if let Some(code) = key_code(key, ctrl) {
            self.shared.lock().push(CanvasEvent::Key(code));
        }
        false
    }

    fn on_mouse(&mut self, event: MouseEvent) -> bool {
        let mut s = self.shared.lock();
        let event = match event {
            MouseEvent::Down { x, y, button } => {
                s.buttons |= button_bit(button);
                CanvasEvent::MouseDown { x, y, button: button_bit(button) }
            }
            MouseEvent::Up { x, y, button } => {
                s.buttons &= !button_bit(button);
                CanvasEvent::MouseUp { x, y, button: button_bit(button) }
            }
            MouseEvent::Move { x, y, .. } => {
                // Only moves over the canvas, or drags that started on it.
                let inside = x >= 0 && y >= 0 && (x as usize) < s.width && (y as usize) < s.height;
                if !inside && s.buttons == 0 {
                    return false;
                }
                CanvasEvent::MouseMove { x, y, buttons: s.buttons }
            }
            MouseEvent::Scroll(clicks) => CanvasEvent::Scroll(clicks),
        };
        s.push(event);
        false
    }

    fn tick(&mut self) -> bool {
        core::mem::take(&mut self.shared.lock().dirty)
    }

    fn on_close(&mut self) {
        self.shared.lock().push(CanvasEvent::Closed);
    }
}

/// Opens a `width` x `height` canvas window, black until the first frame.
pub fn open(title: &str, width: usize, height: usize) -> Option<WindowId> {
    if width == 0 || height == 0 || width > MAX_W || height > MAX_H {
        return None;
    }
    let shared = Arc::new(IrqMutex::new(Shared {
        width,
        height,
        pixels: vec![0; width * height],
        dirty: false,
        events: VecDeque::new(),
        buttons: 0,
    }));
    let id = crate::ui::window::open_app(Box::new(CanvasApp { title: String::from(title), shared: shared.clone() }));
    CANVASES.lock().insert(id, shared);
    Some(id)
}

/// Shows a new frame of exactly width x height pixels. False if `id`
/// isn't a canvas or the frame is the wrong size.
pub fn present(id: WindowId, frame: &[u32]) -> bool {
    let Some(shared) = CANVASES.lock().get(&id).cloned() else { return false };
    let mut s = shared.lock();
    if frame.len() != s.pixels.len() {
        return false;
    }
    s.pixels.copy_from_slice(frame);
    s.dirty = true;
    drop(s);
    crate::ui::task::wake_ui();
    true
}

/// The frame size a canvas expects, in pixels.
pub fn pixel_count(id: WindowId) -> Option<usize> {
    CANVASES.lock().get(&id).map(|s| s.lock().pixels.len())
}

/// The next input event, `Ok(None)` if there's none yet, or `Err` if `id`
/// isn't a canvas.
pub fn poll(id: WindowId) -> Result<Option<CanvasEvent>, ()> {
    let shared = CANVASES.lock().get(&id).cloned().ok_or(())?;
    let event = shared.lock().events.pop_front();
    Ok(event)
}

/// Closes a canvas window and forgets it.
pub fn close(id: WindowId) {
    if CANVASES.lock().remove(&id).is_some() {
        crate::ui::window::close_window(id);
    }
}
