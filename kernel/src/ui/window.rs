//! The window API for the rest of the kernel (and, via syscalls, programs).
//!
//! All window state belongs to the UI task. Everyone else talks to it
//! through a request queue, and reads back button presses from an event
//! queue, so callers never block on the compositor.

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use crate::std::lock::irq_mutex::IrqMutex;
use crate::ui::apps::App;

pub type WindowId = u32;

/// One piece of content for a program's window. `x`/`y` are relative to
/// the window's content area; the window manager decides where the window
/// itself goes.
pub enum WindowElementDesc {
    Label { x: usize, y: usize, text: String, color: u32, scale: usize },
    Button { x: usize, y: usize, w: usize, h: usize, label: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowEvent {
    /// The button at this element index (0-based, in the order elements
    /// were given) was clicked or activated with the keyboard.
    ElementActivated(u32),
    /// The user closed the window.
    Closed,
}

pub enum Request {
    Open { id: WindowId, app: Box<dyn App> },
    Close(WindowId),
}

static NEXT_ID: AtomicU32 = AtomicU32::new(1);
static REQUESTS: IrqMutex<VecDeque<Request>> = IrqMutex::new(VecDeque::new());
static EVENTS: IrqMutex<VecDeque<(WindowId, WindowEvent)>> = IrqMutex::new(VecDeque::new());

pub(crate) fn next_id() -> WindowId {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

fn request(req: Request) {
    REQUESTS.lock().push_back(req);
    crate::ui::task::wake_ui();
}

/// Opens a window showing `app`.
pub fn open_app(app: Box<dyn App>) -> WindowId {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    request(Request::Open { id, app });
    id
}

/// Opens a window of labels and buttons; poll it with `poll_window_event`.
pub fn open_window(title: &str, elements: Vec<WindowElementDesc>) -> WindowId {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let app = crate::ui::apps::widgets::ProgramWindow::new(id, String::from(title), elements);
    request(Request::Open { id, app: Box::new(app) });
    id
}

pub fn close_window(id: WindowId) {
    request(Request::Close(id));
}

/// Pops the oldest pending event for a window, if any.
pub fn poll_window_event(id: WindowId) -> Option<WindowEvent> {
    let mut events = EVENTS.lock();
    let index = events.iter().position(|(w, _)| *w == id)?;
    events.remove(index).map(|(_, e)| e)
}

pub(crate) fn push_event(id: WindowId, event: WindowEvent) {
    let mut events = EVENTS.lock();
    // Don't let events for windows nobody polls pile up forever.
    if events.len() >= 256 {
        events.pop_front();
    }
    events.push_back((id, event));
}

pub(crate) fn take_requests() -> VecDeque<Request> {
    core::mem::take(&mut *REQUESTS.lock())
}

pub(crate) fn has_requests() -> bool {
    !REQUESTS.lock().is_empty()
}
