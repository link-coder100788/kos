pub mod calls;

use crate::sections::*;

/// One boot step, placed in an `.initcall.*` linker section by
/// `#[initcall]` and run in (level, priority) order.
#[repr(C)]
pub struct Initcall {
    pub name: &'static str,
    pub func: fn(),
}

fn run(level: &str, start: *const u8, end: *const u8) {
    unsafe {
        let mut current = start.cast::<Initcall>();
        let end = end.cast::<Initcall>();
        while current < end {
            let call = &*current;
            let started = crate::arch::timer::uptime_ms();
            (call.func)();
            log::debug!("initcall {}::{} done in {} ms", level, call.name, crate::arch::timer::uptime_ms() - started);
            current = current.add(1);
        }
    }
}

pub unsafe fn early() {
    run("early", &raw const __initcall_early_start, &raw const __initcall_early_end);
}

pub unsafe fn middle() {
    run("middle", &raw const __initcall_middle_start, &raw const __initcall_middle_end);
}

pub unsafe fn late() {
    run("late", &raw const __initcall_late_start, &raw const __initcall_late_end);
}
