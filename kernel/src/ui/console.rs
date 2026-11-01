//! A copy of everything printed to the console, kept as lines of text for
//! the Terminal window to display.
//!
//! Writers never wait: printing happens from panics and interrupts, so if
//! the buffer is busy the text only goes to the serial port.

use core::sync::atomic::{AtomicU64, Ordering};
use crate::std::lock::irq_mutex::IrqMutex;

pub const COLS: usize = 120;
const ROWS: usize = 256;

struct Console {
    lines: [[u8; COLS]; ROWS],
    lens: [u8; ROWS],
    /// Ring index of the line being written.
    current: usize,
    /// Lines written so far, capped at ROWS.
    count: usize,
    col: usize,
}

static CONSOLE: IrqMutex<Console> = IrqMutex::new(Console {
    lines: [[0; COLS]; ROWS],
    lens: [0; ROWS],
    current: 0,
    count: 1,
    col: 0,
});

/// Bumped on every change, so viewers know when to redraw.
static GENERATION: AtomicU64 = AtomicU64::new(0);

impl Console {
    fn newline(&mut self) {
        self.current = (self.current + 1) % ROWS;
        self.lens[self.current] = 0;
        self.count = (self.count + 1).min(ROWS);
        self.col = 0;
    }

    fn put(&mut self, byte: u8) {
        match byte {
            b'\n' => self.newline(),
            b'\r' => {}
            0x08 => self.col = self.col.saturating_sub(1),
            b'\t' => {
                for _ in 0..(8 - self.col % 8) {
                    self.put(b' ');
                }
            }
            0x20..=0x7e => {
                if self.col == COLS {
                    self.newline();
                }
                let line = self.current;
                self.lines[line][self.col] = byte;
                self.col += 1;
                self.lens[line] = self.lens[line].max(self.col as u8);
            }
            _ => {}
        }
    }
}

pub fn write(bytes: &[u8]) {
    if let Some(mut console) = CONSOLE.try_lock() {
        for &b in bytes {
            console.put(b);
        }
        GENERATION.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

/// Copies the last `out.len()` lines (oldest first) into `out`; returns
/// how many are valid, the row of the cursor and its column.
pub fn tail(out: &mut [[u8; COLS]], lens: &mut [usize]) -> (usize, usize, usize) {
    let console = CONSOLE.lock();
    let n = out.len().min(console.count);
    for i in 0..n {
        let line = (console.current + ROWS + 1 - n + i) % ROWS;
        out[i] = console.lines[line];
        lens[i] = console.lens[line] as usize;
    }
    (n, n - 1, console.col)
}
