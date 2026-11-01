//! PL011 UART: the serial console.
//!
//! Output goes to the UART and to `ui::console` (shown in the Terminal
//! window). Input comes from two places: bytes received by the UART
//! interrupt, and keys typed into the Terminal window (`feed_input`).
//! The shell reads it, or the foreground program while one is running.
//!
//! Ctrl+C (byte 0x03) is caught as it arrives: with a foreground program
//! running, it kills the program, even one that isn't reading input. At
//! the shell prompt it cancels the line being typed.

use core::fmt;
use core::fmt::Write;
use core::fmt::Arguments;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use crate::scheduler::wait::WaitQueue;
use crate::std::sync::spsc::SpscQueue;
use core::ptr::read_volatile;
use core::ptr::write_volatile;
use core::arch::asm;
use alloc::boxed::Box;
use alloc::string::String;
use crate::mm::layout::phys_to_virt;

const UART0_BASE: usize = phys_to_virt(0x0900_0000);
const UART0_DR: *mut u32 = UART0_BASE as *mut u32;
const UART0_FR: *mut u32 = (UART0_BASE + 0x18) as *mut u32;
const UART0_IMSC: *mut u32 = (UART0_BASE + 0x38) as *mut u32;
const UART0_ICR: *mut u32 = (UART0_BASE + 0x44) as *mut u32;
const FR_RXFE: u32 = 1 << 4;
const FR_TXFF: u32 = 1 << 5;
/// Receive and receive-timeout interrupts.
const INT_RX: u32 = (1 << 4) | (1 << 6);

/// Bytes received by the UART interrupt (its only producer).
static RX: SpscQueue<u8, 1024> = SpscQueue::new();
/// Bytes typed into the Terminal window (the UI task is the only producer).
static TERMINAL_RX: SpscQueue<u8, 256> = SpscQueue::new();
static RX_WAIT: WaitQueue = WaitQueue::new();

/// Turns on receive interrupts. Until then nothing reads the UART.
pub fn init_irq() {
    unsafe {
        write_volatile(UART0_ICR, 0x7ff);
        write_volatile(UART0_IMSC, INT_RX);
    }
    crate::arch::gic::register(crate::arch::gic::UART_IRQ, uart_irq);
}

fn uart_irq() {
    unsafe {
        // Clear first, then drain: a byte that arrives after the last read
        // raises a fresh interrupt instead of having its interrupt cleared.
        write_volatile(UART0_ICR, INT_RX);
        while read_volatile(UART0_FR) & FR_RXFE == 0 {
            let byte = read_volatile(UART0_DR) as u8;
            if !interrupts_program(byte) {
                // Dropped if nobody has read the last 1024 bytes.
                let _ = RX.push(byte);
            }
        }
    }
    RX_WAIT.wake_all();
}

/// Delivers a byte typed into the Terminal window to the console reader,
/// as if it had come in over the serial port. UI task only.
pub fn feed_input(byte: u8) {
    if !interrupts_program(byte) {
        let _ = TERMINAL_RX.push(byte);
    }
    RX_WAIT.wake_all();
}

/// Ctrl+C.
const INTERRUPT: u8 = 0x03;

/// If `byte` is Ctrl+C and a foreground program is running, kills it and
/// returns true: the byte is used up. Safe from IRQ context.
fn interrupts_program(byte: u8) -> bool {
    byte == INTERRUPT && crate::process::interrupt_foreground()
}

/// Set after a '\r', so a following '\n' (terminals that send CR LF for
/// Enter) doesn't count as a second, empty line.
static LAST_WAS_CR: AtomicBool = AtomicBool::new(false);

pub struct Uart {}

impl Uart {
    /// Reads a line with echo and backspace editing, without the newline.
    /// Escape sequences (arrow keys etc.) are ignored.
    pub fn read_s(&self) -> String {
        self.read_s_unless(|| false).unwrap()
    }

    /// Like `read_s`, but gives up (returning None) once `stop()` is true.
    /// `stop` runs with IRQs masked, like a `WaitQueue` condition.
    pub fn read_s_unless(&self, stop: impl Fn() -> bool) -> Option<String> {
        let mut s = String::new();
        loop {
            let b = self.read_b_unless(&stop)?;
            let after_cr = LAST_WAS_CR.swap(b == b'\r', Ordering::Relaxed);
            match b {
                b'\n' if after_cr => {}
                b'\r' | b'\n' => {
                    crate::print!("\n");
                    return Some(s);
                }
                INTERRUPT => {
                    crate::print!("^C\n");
                    return Some(String::new());
                }
                0x7f | 0x08 => {
                    if s.pop().is_some() {
                        crate::print!("\x08 \x08");
                    }
                }
                0x1b => {
                    // ESC [ <final byte>: skip until the final byte.
                    if self.read_b_unless(&stop)? == b'[' {
                        while !(0x40..=0x7e).contains(&self.read_b_unless(&stop)?) {}
                    }
                }
                0x20..=0x7e => {
                    s.push(b as char);
                    crate::print!("{}", b as char);
                }
                _ => {}
            }
        }
    }

    pub fn write_b(&self, byte: u8) {
        unsafe {
            while read_volatile(UART0_FR) & FR_TXFF != 0 { asm!("nop"); }
            write_volatile(UART0_DR, byte as u32);
        }
    }

    pub fn write_s(&self, s: &str) {
        for b in s.bytes() {
            if b == b'\n' { self.write_b(b'\r'); }
            self.write_b(b);
        }
    }

    /// Blocks until a byte arrives from either input source.
    pub fn read_b(&self) -> u8 {
        self.read_b_unless(&|| false).unwrap()
    }

    fn read_b_unless(&self, stop: &impl Fn() -> bool) -> Option<u8> {
        let mut byte = None;
        RX_WAIT.wait_until(|| {
            byte = RX.pop().or_else(|| TERMINAL_RX.pop());
            byte.is_some() || stop()
        });
        let val = byte?;
        log::trace!("console input byte {:#04x}", val);
        Some(val)
    }
}

impl Write for Uart {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_s(s);
        crate::ui::console::write(s.as_bytes());
        Ok(())
    }
}

/// Core id of whoever is printing, or NO_OWNER. Keeps lines from different
/// cores from interleaving. Re-entry from the same core (an exception or a
/// preempted task printing while another print is in progress) goes
/// straight through instead of deadlocking.
static CONSOLE_OWNER: AtomicU64 = AtomicU64::new(NO_OWNER);
const NO_OWNER: u64 = u64::MAX;

pub fn _print(args: Arguments) {
    let core = crate::arch::psci::get_core_id();
    let reentrant = CONSOLE_OWNER.load(Ordering::Relaxed) == core;
    if !reentrant {
        while CONSOLE_OWNER.compare_exchange(NO_OWNER, core, Ordering::Acquire, Ordering::Relaxed).is_err() {
            core::hint::spin_loop();
        }
    }
    let mut uart = Uart {};
    let _ = uart.write_fmt(args);
    if !reentrant {
        CONSOLE_OWNER.store(NO_OWNER, Ordering::Release);
    }
}

/// Writes raw bytes to the console (for programs' write syscalls), under
/// the same lock as `println!`.
pub fn console_write(bytes: &[u8]) {
    let core = crate::arch::psci::get_core_id();
    let reentrant = CONSOLE_OWNER.load(Ordering::Relaxed) == core;
    if !reentrant {
        while CONSOLE_OWNER.compare_exchange(NO_OWNER, core, Ordering::Acquire, Ordering::Relaxed).is_err() {
            core::hint::spin_loop();
        }
    }
    let uart = Uart {};
    for &b in bytes {
        if b == b'\n' { uart.write_b(b'\r'); }
        uart.write_b(b);
    }
    crate::ui::console::write(bytes);
    if !reentrant {
        CONSOLE_OWNER.store(NO_OWNER, Ordering::Release);
    }
}

/// Releases the console if this core holds it. For the panic handler: a
/// core that dies mid-print would otherwise lock every other core out.
pub fn force_unlock_console() {
    let core = crate::arch::psci::get_core_id();
    let _ = CONSOLE_OWNER.compare_exchange(core, NO_OWNER, Ordering::Release, Ordering::Relaxed);
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::drivers::uart::_print(format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! println {
    () => {
        $crate::print!("\n")
    };
    ($($arg:tt)*) => {
        // One _print call per line, so the line is printed under one lock.
        $crate::drivers::uart::_print(format_args!("{}\n", format_args!($($arg)*)))
    };
}

pub struct UartNonBlocking {}

impl UartNonBlocking {
    pub fn write_b(&self, byte: u8) -> Result<(), &'static str> {
        if unsafe { read_volatile(UART0_FR) & (1 << 5) == 0 } {
            unsafe { write_volatile(UART0_DR, byte as u32) };
            Ok(())
        } else {
            Err("UART busy")
        }
    }
}

pub trait UartDriver {
    fn write_byte(&self, byte: u8) -> Result<(), &'static str>;
}

impl UartDriver for Uart {
    fn write_byte(&self, byte: u8) -> Result<(), &'static str> {
        self.write_b(byte);
        Ok(())
    }
}
impl UartDriver for UartNonBlocking {
    fn write_byte(&self, byte: u8) -> Result<(), &'static str> {
        self.write_b(byte)
    }
}

pub fn write_uart_byte(uart: Box<dyn UartDriver>, byte: u8) -> Result<(), &'static str> {
    uart.write_byte(byte)
}

pub fn write_uart_byte_s<T: UartDriver>(uart: &T, byte: u8) -> Result<(), &'static str> {
    uart.write_byte(byte)
}

#[macro_export]
macro_rules! dbg {
    ($val:expr) => {{
        let tmp = $val;
        crate::println!("[{}:{}:{}] {} = {:?}", file!(), line!(), column!(), stringify!($val), &tmp);
        tmp
    }};
}



#[macro_export]
macro_rules! id_println {
    () => {{
        let core_id: u64;
        unsafe {
            core::arch::asm!(
                "mrs {0}, mpidr_el1",
                out(reg) core_id,
                options(nomem, nostack, preserves_flags)
            );
        }
        $crate::println!("[Core {}]", core_id & 0xff);
    }};

    ($($arg:tt)+) => {{
        let core_id: u64;
        unsafe {
            core::arch::asm!(
                "mrs {0}, mpidr_el1",
                out(reg) core_id,
                options(nomem, nostack, preserves_flags)
            );
        }
        $crate::println!("[Core {}]: {}", core_id & 0xff, format_args!($($arg)+));
    }};
}
