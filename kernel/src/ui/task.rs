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
    crate::scheduler::follow_session();
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
    crate::filesys::fs_task::wait_until_ready();
    let mut logged_in = None;
    loop {
        // Logging in (here or at the console) opens the desktop; logging
        // out closes everything and asks again.
        let now = crate::users::logged_in();
        if logged_in != Some(now) || (!now && !wm.has_windows()) {
            if logged_in != Some(now) {
                wm.close_all();
            }
            logged_in = Some(now);
            wm.set_locked(!now);
            // Before the first account exists, the Terminal sets KOS up.
            let app: Box<dyn apps::App> = if now || !crate::users::any() {
                Box::new(apps::terminal::Terminal::new())
            } else {
                Box::new(apps::login::Login::new())
            };
            wm.open(window::next_id(), app);
        }
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
