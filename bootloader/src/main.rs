#![no_main]
#![no_std]

use core::arch::global_asm;
use core::panic::PanicInfo;
use core::ptr::copy_nonoverlapping;
use core::ptr::write_volatile;
use core::mem::transmute;

global_asm!(include_str!("boot.s"));

#[repr(align(16))]
struct AlignedPayload {
    data: [u8; include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../build/kernel.bin")).len()],
}

static KERNEL_PAYLOAD: AlignedPayload = AlignedPayload {
    data: *include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../build/kernel.bin")),
};

const KERNEL_LOAD_ADDRESS: *mut u8 = 0x40200000 as *mut u8;

unsafe extern "C" { fn _start(); }

#[unsafe(no_mangle)] pub extern "C" fn bootloader_main() -> ! {
    local_keep(_start);
    unsafe { write_volatile(0x09000000 as *mut u32, b'`' as u32); }
    unsafe {
        copy_nonoverlapping(KERNEL_PAYLOAD.data.as_ptr(), KERNEL_LOAD_ADDRESS, KERNEL_PAYLOAD.data.len());
        let kernel_entry: extern "C" fn() -> ! = transmute(KERNEL_LOAD_ADDRESS);
        write_volatile(0x09000000 as *mut u32, b'!' as u32);
        kernel_entry();
    }
}

fn local_keep(_f: unsafe extern "C" fn()) {
    core::hint::black_box(_f);
}

#[panic_handler] fn panic(_info: &PanicInfo) -> ! {
    unsafe { write_volatile(0x09000000 as *mut u32, b'*' as u32); }
    loop {}
}
