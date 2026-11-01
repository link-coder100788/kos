//! The in-kernel test suite (`make test`).
//!
//! Unit tests run at boot, before the scheduler (`init::calls`). Tests
//! that need it, like running programs in user mode (`user_mode`), run on
//! a `tests` task afterwards, which then prints `test result: ok. N
//! passed` and powers the machine off. A failing test panics, and the
//! panic handler prints `test result: FAILED` and powers off too, so
//! `make test` always finishes and can tell which happened.

#[cfg(feature = "test")]
pub mod user_mode;
#[cfg(feature = "test")]
pub mod net;
#[cfg(feature = "test")]
pub mod sound;

use core::sync::atomic::{AtomicUsize, Ordering};
use crate::{id_println, println};

/// Tests that have passed so far.
static PASSED: AtomicUsize = AtomicUsize::new(0);

pub trait Test {
    fn run(&self);
}

impl<T> Test for T
where T: Fn(),
{
    fn run(&self) {
        println!("{}...\t", core::any::type_name::<T>());
        self();
        println!("OK");
        PASSED.fetch_add(1, Ordering::Relaxed);
    }
}

pub type TestSuite = &'static [&'static dyn Test];

/// Runs `tests`, announcing how many.
pub fn run_all(tests: &[&dyn Test]) {
    println!("Running {} tests", tests.len());
    for test in tests {
        test.run();
    }
}

/// Turns the (virtual) machine off: PSCI SYSTEM_OFF. QEMU exits.
pub fn power_off() -> ! {
    unsafe { core::arch::asm!("hvc #0", in("x0") 0x8400_0008u64, options(nomem, nostack)) };
    loop { core::hint::spin_loop(); }
}

/// The last thing `make test` prints when everything passed.
pub fn finish() -> ! {
    println!("test result: ok. {} passed", PASSED.load(Ordering::Relaxed));
    power_off()
}

/// Called by the panic handler in test builds: a test failed.
pub fn failed() -> ! {
    println!("test result: FAILED after {} passed", PASSED.load(Ordering::Relaxed));
    power_off()
}

/// The `tests` task: the tests that need the scheduler, then the result.
#[cfg(feature = "test")]
pub extern "C" fn test_task() -> ! {
    user_mode::run();
    net::run();
    sound::run();
    finish()
}

pub fn test_secondary_core_restart() {
    id_println!("testing core restart");
    panic!("test panic");
}
/// A big allocation makes the heap grow; freeing it makes it shrink back
/// (to within its cushion of free space).
pub fn heap_grows_and_shrinks_test() {
    use crate::mm::allocator::ALLOCATOR;
    use crate::mm::layout::HEAP_GROW;
    let (before, _) = ALLOCATOR.heap_size();
    let big = alloc::vec![1u8; 48 * 1024 * 1024];
    let (grown, _) = ALLOCATOR.heap_size();
    assert!(grown >= before + big.len(), "heap didn't grow: {} -> {}", before, grown);
    drop(big);
    let (after, _) = ALLOCATOR.heap_size();
    assert!(after < grown && after <= before + 2 * HEAP_GROW, "heap didn't shrink: {} -> {} -> {}", before, grown, after);
}

/// Allocates and frees a few thousand blocks of mixed sizes and alignments,
/// checking contents survive and that every byte comes back at the end.
pub fn heap_stress_test() {
    use alloc::alloc::{alloc, dealloc, Layout};
    use alloc::vec::Vec;
    use crate::mm::allocator::ALLOCATOR;

    let before = ALLOCATOR.heap_remaining().1;
    let mut rng: u64 = 0x2545_f491_4f6c_dd1d;
    let mut next = || { rng ^= rng << 13; rng ^= rng >> 7; rng ^= rng << 17; rng };
    let mut live: Vec<(*mut u8, Layout, u8)> = Vec::with_capacity(512);
    for round in 0..4000 {
        if live.len() < 400 && (next() % 3 != 0 || live.is_empty()) {
            let size = 1 + (next() % 4096) as usize;
            let align = 1 << (next() % 13); // up to 4 KiB
            let layout = Layout::from_size_align(size, align).unwrap();
            let ptr = unsafe { alloc(layout) };
            assert!(!ptr.is_null(), "heap test: out of memory at round {}", round);
            assert!(ptr as usize % align == 0, "heap test: misaligned {:p} for align {}", ptr, align);
            let tag = (round % 251) as u8;
            unsafe { core::ptr::write_bytes(ptr, tag, size) };
            live.push((ptr, layout, tag));
        } else {
            let i = (next() as usize) % live.len();
            let (ptr, layout, tag) = live.swap_remove(i);
            let bytes = unsafe { core::slice::from_raw_parts(ptr, layout.size()) };
            assert!(bytes.iter().all(|&b| b == tag), "heap test: block {:p} was overwritten", ptr);
            unsafe { dealloc(ptr, layout) };
        }
    }
    for (ptr, layout, _) in live.drain(..) {
        unsafe { dealloc(ptr, layout) };
    }
    drop(live);
    let after = ALLOCATOR.heap_remaining().1;
    assert_eq!(before, after, "heap test: leaked {} bytes", before as i64 - after as i64);
    println!("heap test: OK ({} bytes free before and after)", after);
}
