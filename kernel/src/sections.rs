//! Symbols defined by the linker script (`linker.ld`).

unsafe extern "C" {
    pub static mut __bss_start: u8;
    pub static mut __bss_end: u8;
    pub static _end: u8;
}

unsafe extern "C" {
    pub static __initcall_early_start: u8;
    pub static __initcall_early_end: u8;
    pub static __initcall_middle_start: u8;
    pub static __initcall_middle_end: u8;
    pub static __initcall_late_start: u8;
    pub static __initcall_late_end: u8;
}

unsafe extern "C" {
    pub static __syscalls_start: crate::syscall::SyscallEntry;
    pub static __syscalls_end: crate::syscall::SyscallEntry;
}

unsafe extern "C" {
    pub static ___fn_section_start: u8;
    pub static ___fn_section_end: u8;
}

unsafe extern "C" {
    pub static __secondary_entry_start: u8;
    pub static __tertiary_entry_start: u8;
}

unsafe extern "C" {
    pub static __kernel_start: u8;
    pub static __text_start: u8;
    pub static __text_end: u8;
    pub static __rodata_start: u8;
    pub static __rodata_end: u8;
    pub static __data_start: u8;
}

unsafe extern "C" {
    pub static __boot_start: u8;
    pub static __boot_end: u8;
}
