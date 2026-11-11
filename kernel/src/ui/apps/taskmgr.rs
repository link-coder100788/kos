//! The Task Manager: every task on core 0, programs first, with how much
//! CPU they've used over the last second and how much memory each
//! program has, and a line on the GPU. Select a program and click End task
//! (or press Delete) to kill it.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use crate::arch::timer;
use crate::drivers::virtio::kgl;
use crate::drivers::input::{Key, MouseButton};
use crate::process::{self, Console};
use crate::scheduler::{self, TaskId};
use crate::ui::apps::{draw_button, App, MouseEvent, ACCENT, BG, FG};
use crate::ui::gfx::Rect;
use crate::ui::gfx::Surface;

/// Size when it opens.
const START_W: i32 = 610;
const START_H: i32 = 400;
const GPU_Y: i32 = 32;
const HEADER_Y: i32 = 56;
const TOP: i32 = 78;
const ROW_H: i32 = 22;
/// Below the rows: the "scroll for more" line, the status and the button.
const FOOTER_H: i32 = 84;
const REFRESH_MS: u64 = 1000;
/// Column x positions: id, name, kind, core, state, CPU, memory.
const COLS: [i32; 7] = [12, 56, 220, 330, 380, 470, 526];
const DIM: u32 = 0x00606060;

struct Row {
    id: TaskId,
    name: String,
    /// None for kernel tasks.
    console: Option<Console>,
    core: usize,
    state: &'static str,
    cpu: u64,
    /// Everything mapped for a program: image, heap and stack.
    memory: usize,
}

pub struct TaskManager {
    rows: Vec<Row>,
    selected: Option<TaskId>,
    scroll: usize,
    status: String,
    last_refresh: u64,
    /// Each task's ticks, and each core's, at the last refresh.
    last_ticks: BTreeMap<TaskId, u64>,
    last_all: [u64; scheduler::MAX_CORES],
    /// The GPU's stats at the last refresh, and when (timer::now), and
    /// what the line about it says.
    last_gpu: Option<(kgl::Stats, u64)>,
    gpu: String,
    /// Size of the window's content area.
    w: i32,
    h: i32,
}

impl TaskManager {
    pub fn new() -> Self {
        let mut tm = Self {
            rows: Vec::new(),
            selected: None,
            scroll: 0,
            status: String::from("Select a program to end it"),
            last_refresh: 0,
            last_ticks: BTreeMap::new(),
            last_all: [0; scheduler::MAX_CORES],
            last_gpu: None,
            gpu: String::new(),
            w: START_W,
            h: START_H,
        };
        tm.refresh();
        tm
    }

    fn refresh(&mut self) {
        let programs: BTreeMap<TaskId, process::ProcessInfo> = process::list().into_iter().map(|p| (p.task, p)).collect();
        let all: [u64; scheduler::MAX_CORES] = core::array::from_fn(scheduler::all_ticks);
        let mut ticks = BTreeMap::new();
        let mut rows: Vec<Row> = scheduler::tasks().into_iter().map(|t| {
            let before = self.last_ticks.get(&t.id).copied().unwrap_or(t.ticks);
            ticks.insert(t.id, t.ticks);
            let program = programs.get(&t.id);
            // Over the last second, of its own core's time.
            let elapsed = all[t.core].saturating_sub(self.last_all[t.core]).max(1);
            Row {
                id: t.id,
                name: t.name,
                console: program.map(|p| p.console),
                core: t.core,
                state: t.state,
                cpu: (t.ticks - before) * 100 / elapsed,
                memory: program.map_or(0, |p| p.memory),
            }
        }).collect();
        // Programs first, then kernel tasks, each in the order started.
        rows.sort_by_key(|r| (r.console.is_none(), r.id));
        self.rows = rows;
        self.last_ticks = ticks;
        self.last_all = all;
        self.refresh_gpu();
        self.last_refresh = timer::uptime_ms();
        if self.selected.is_some_and(|id| !self.rows.iter().any(|r| r.id == id)) {
            self.selected = None;
        }
        self.scroll = self.scroll.min(self.rows.len().saturating_sub(self.visible_rows()));
    }

    fn refresh_gpu(&mut self) {
        let now = timer::now();
        let Some(stats) = kgl::stats() else {
            self.gpu = String::from("GPU: none (this QEMU has no kos-gl device)");
            return;
        };
        let contexts = format!("{} context{}", stats.contexts, if stats.contexts == 1 { "" } else { "s" });
        self.gpu = match &self.last_gpu {
            // Over the last second, as for CPU.
            Some((before, then)) => {
                let elapsed = (now - then).max(1);
                let fps = (stats.frames - before.frames) * timer::frequency() / elapsed;
                let busy = ((stats.busy - before.busy) * 100 / elapsed).min(100);
                format!("GPU: host OpenGL, {}, {} frames/s, busy {}%", contexts, fps, busy)
            }
            None => format!("GPU: host OpenGL, {}", contexts),
        };
        self.last_gpu = Some((stats, now));
    }

    fn selected_row(&self) -> Option<&Row> {
        self.rows.iter().find(|r| Some(r.id) == self.selected)
    }

    fn can_end(&self) -> bool {
        self.selected_row().is_some_and(|r| r.console.is_some())
    }

    fn end_task(&mut self) -> bool {
        let Some(row) = self.selected_row() else { return false };
        let (id, name) = (row.id, row.name.clone());
        self.status = match process::kill(id) {
            Ok(()) => format!("Ended {}", name),
            Err(e) => format!("Can't end {}: {}", name, e),
        };
        // Look again soon, once it's gone.
        self.last_refresh = timer::uptime_ms().saturating_sub(REFRESH_MS - 200);
        true
    }

    /// How many rows fit.
    fn visible_rows(&self) -> usize {
        ((self.h - TOP - FOOTER_H) / ROW_H).max(1) as usize
    }

    fn end_task_rect(&self) -> Rect {
        Rect::new(self.w - 112, self.h - 38, 100, 28)
    }

    fn row_rect(&self, i: usize) -> Rect {
        Rect::new(6, TOP + i as i32 * ROW_H, self.w - 12, ROW_H)
    }

    fn row_at(&self, x: i32, y: i32) -> Option<usize> {
        (0..self.visible_rows()).find(|&i| self.row_rect(i).contains(x, y)).map(|i| i + self.scroll).filter(|&i| i < self.rows.len())
    }
}

impl App for TaskManager {
    fn title(&self) -> String {
        String::from("Task Manager")
    }

    fn size(&self) -> (i32, i32) {
        (self.w, self.h)
    }

    fn resizable(&self) -> bool {
        true
    }

    fn min_size(&self) -> (i32, i32) {
        (START_W, TOP + 4 * ROW_H + FOOTER_H)
    }

    fn resize(&mut self, w: i32, h: i32) {
        self.w = w;
        self.h = h;
        self.scroll = self.scroll.min(self.rows.len().saturating_sub(self.visible_rows()));
    }

    fn draw(&mut self, s: &mut Surface) {
        let rows = self.visible_rows();
        s.fill(BG);
        let programs = self.rows.iter().filter(|r| r.console.is_some()).count();
        s.draw_text(12, 12, &format!("{} programs, {} kernel tasks", programs, self.rows.len() - programs), FG, 1, 2);
        let idle: Vec<String> = scheduler::scheduler_cores().iter()
            .map(|&c| format!("{}:{}%", c, scheduler::idle_percent_of(c))).collect();
        let idle = format!("idle {}", idle.join(" "));
        s.draw_text(self.w - 12 - 8 * idle.len() as i32, 12, &idle, DIM, 1, 2);
        s.draw_text(12, GPU_Y, &self.gpu, DIM, 1, 2);
        for (x, label) in COLS.iter().zip(["ID", "Name", "Kind", "Core", "State", "CPU", "Memory"]) {
            s.draw_text(*x, HEADER_Y, label, DIM, 1, 2);
        }
        s.fill_rect(Rect::new(6, TOP - 3, self.w - 12, 1), 0x00c0c4cc);
        for (i, row) in self.rows.iter().skip(self.scroll).take(rows).enumerate() {
            let r = self.row_rect(i);
            let selected = Some(row.id) == self.selected;
            if selected {
                s.fill_rect(r, ACCENT);
            }
            let color = if selected { 0x00ffffff } else if row.console.is_some() { FG } else { DIM };
            let name: String = row.name.chars().take(19).collect();
            let kind = match row.console {
                Some(c) => format!("program {}", c.name()),
                None => String::from("kernel"),
            };
            let memory = if row.console.is_some() { format!("{} KiB", row.memory.div_ceil(1024)) } else { String::from("-") };
            let cells = [format!("{}", row.id), name, kind, format!("{}", row.core), String::from(row.state), format!("{}%", row.cpu), memory];
            for (x, cell) in COLS.iter().zip(cells.iter()) {
                s.draw_text(*x, r.y + 3, cell, color, 1, 2);
            }
        }
        if self.rows.len() > rows {
            s.draw_text(12, TOP + rows as i32 * ROW_H + 2, &format!("{}-{} of {} (scroll for more)",
                self.scroll + 1, (self.scroll + rows).min(self.rows.len()), self.rows.len()), DIM, 1, 2);
        }
        s.draw_text(12, self.h - 31, &self.status, DIM, 1, 2);
        draw_button(s, self.end_task_rect(), "End task", self.can_end());
    }

    fn on_mouse(&mut self, event: MouseEvent) -> bool {
        match event {
            MouseEvent::Down { x, y, button: MouseButton::Left } => {
                if self.end_task_rect().contains(x, y) {
                    return self.can_end() && self.end_task();
                }
                if let Some(i) = self.row_at(x, y) {
                    let row = &self.rows[i];
                    self.selected = Some(row.id);
                    self.status = if row.console.is_some() {
                        format!("{} (task {})", row.name, row.id)
                    } else {
                        format!("{} is part of the kernel and can't be ended", row.name)
                    };
                    return true;
                }
                false
            }
            MouseEvent::Scroll(lines) => {
                let max = self.rows.len().saturating_sub(self.visible_rows()) as isize;
                let new = (self.scroll as isize + lines as isize).clamp(0, max) as usize;
                let changed = new != self.scroll;
                self.scroll = new;
                changed
            }
            _ => false,
        }
    }

    fn on_key(&mut self, key: Key, _ctrl: bool) -> bool {
        match key {
            Key::Delete if self.can_end() => self.end_task(),
            _ => false,
        }
    }

    fn tick(&mut self) -> bool {
        if timer::uptime_ms() - self.last_refresh >= REFRESH_MS {
            self.refresh();
            return true;
        }
        false
    }
}
