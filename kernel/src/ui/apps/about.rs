//! About KOS: live system information.

use alloc::format;
use alloc::string::String;
use crate::arch::{smp, timer};
use crate::mm::{allocator::ALLOCATOR, frame};
use crate::ui::apps::{App, ACCENT, BG, FG};
use crate::ui::gfx::Surface;

pub struct About {
    last_second: u64,
}

impl About {
    pub fn new() -> Self {
        Self { last_second: u64::MAX }
    }
}

impl App for About {
    fn title(&self) -> String {
        String::from("About KOS")
    }

    fn size(&self) -> (i32, i32) {
        (360, 280)
    }

    fn draw(&mut self, s: &mut Surface) {
        s.fill(BG);
        s.draw_text(20, 16, "KOS", ACCENT, 4, 4);
        s.draw_text(20, 54, "A hobby OS for 64-bit ARM", FG, 1, 2);
        let (heap_size, heap_used) = ALLOCATOR.heap_size();
        let (frames_free, _) = frame::stats();
        let mib = |bytes: usize| bytes / (1024 * 1024);
        let cores = (0..5).filter(|&c| smp::is_online(c)).count();
        let up = timer::uptime_ms() / 1000;
        let lines = [
            format!("Uptime     {:02}:{:02}:{:02}", up / 3600, up / 60 % 60, up % 60),
            format!("Cores      {} online", cores),
            format!("Heap       {} of {} MiB used", mib(heap_used), mib(heap_size)),
            format!("Free RAM   {} MiB", mib(frames_free * crate::mm::layout::PAGE_SIZE)),
            format!("Core 0     {}% idle", crate::scheduler::idle_percent()),
            format!(""),
            format!("Made with <3 by Noah Pegna")
        ];
        for (i, line) in lines.iter().enumerate() {
            s.draw_text(20, 92 + i as i32 * 24, line, FG, 1, 2);
        }
    }

    fn tick(&mut self) -> bool {
        let second = timer::uptime_ms() / 1000;
        let changed = second != self.last_second;
        self.last_second = second;
        changed
    }
}
