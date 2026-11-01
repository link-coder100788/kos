//! The UI task: owns the screen and every window.

use alloc::boxed::Box;
use crate::arch::timer;
use crate::drivers::{display, input};
use crate::println;
use crate::scheduler::wait::WaitQueue;
use crate::ui::{apps, window, wm::Wm};

static UI_WAIT: WaitQueue = WaitQueue::new();

/// Wakes the UI task after queueing input or a window request.
pub fn wake_ui() {
    UI_WAIT.wake_all();
}

/// Frame interval for app animations (terminal output, clocks).
const FRAME_MS: u64 = 33;

pub extern "C" fn ui_task() -> ! {
    match display::init() {
        Ok(name) => log::info!("{}x{} display ({})", display::WIDTH, display::HEIGHT, name),
        Err(e) => {
            log::warn!("{}; UI disabled", e);
            loop {
                UI_WAIT.wait_until(|| false);
            }
        }
    }
    let mut wm = Wm::new();
    wm.open(window::next_id(), Box::new(apps::terminal::Terminal::new()));
    loop {
        for request in window::take_requests() {
            wm.handle_request(request);
        }
        while let Some(event) = input::EVENTS.pop() {
            wm.handle_input(event);
        }
        wm.tick();
        wm.render();
        let next_frame = timer::deadline_after_ms(FRAME_MS);
        UI_WAIT.wait_until_deadline(Some(next_frame), || !input::EVENTS.is_empty() || window::has_requests());
    }
}
