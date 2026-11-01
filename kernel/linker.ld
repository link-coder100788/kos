/* The kernel is linked to run in the higher half (TTBR1) at
   KERNEL_OFFSET + physical, but loaded at its physical address by the
   bootloader. Everything before the MMU is on (the code in .text.boot and
   the secondary-core entry stubs) must therefore be position-independent:
   see arch/boot.s. */

KERNEL_OFFSET = 0xFFFFFF8000000000;
KERNEL_PHYS = 0x40200000;

ENTRY(_kernel_entry)

SECTIONS
{
    . = KERNEL_OFFSET + KERNEL_PHYS;
    __kernel_start = .;

    /* Section boundaries are page-aligned so the MMU can give each its own
       permissions: text RX, rodata (incl. initcall/syscall tables) R,
       data/bss RW. */
    __text_start = .;
    .text : AT(ADDR(.text) - KERNEL_OFFSET) {
        /* The boot stubs run at their physical addresses with the MMU off
           (and are identity mapped later so secondary cores can use them).
           The bootloader jumps to these three fixed addresses. */
        __boot_start = .;
        KEEP(*(.text.boot))
        . = ALIGN(4096);
        __secondary_entry_start = .;
        KEEP(*(.text.secondary_entry))
        . = ALIGN(4096);
        __tertiary_entry_start = .;
        KEEP(*(.text.tertiary_entry))
        . = ALIGN(4096);
        __boot_end = .;
        *(.text .text.*)
    }

    . = ALIGN(4096);
    __text_end = .;

    __rodata_start = .;
    .rodata : AT(ADDR(.rodata) - KERNEL_OFFSET) {
        *(.rodata .rodata.*)
    }

    ___fn_section_start = .;

    .syscalls : AT(ADDR(.syscalls) - KERNEL_OFFSET) ALIGN(8) {
        __syscalls_start = .;
        KEEP(*(.syscalls))
        __syscalls_end = .;
    }

    .initcall.early : AT(ADDR(.initcall.early) - KERNEL_OFFSET) ALIGN(8) {
        __initcall_early_start = .;
        KEEP(*(SORT_BY_NAME(.initcall.early.*)))
        __initcall_early_end = .;
    }

    .initcall.middle : AT(ADDR(.initcall.middle) - KERNEL_OFFSET) ALIGN(8) {
        __initcall_middle_start = .;
        KEEP(*(SORT_BY_NAME(.initcall.middle.*)))
        __initcall_middle_end = .;
    }

    .initcall.late : AT(ADDR(.initcall.late) - KERNEL_OFFSET) ALIGN(8) {
        __initcall_late_start = .;
        KEEP(*(SORT_BY_NAME(.initcall.late.*)))
        __initcall_late_end = .;
    }

    ___fn_section_end = .;

    . = ALIGN(4096);
    __rodata_end = .;

    __data_start = .;
    .data : AT(ADDR(.data) - KERNEL_OFFSET) {
        *(.data .data.*)
    }

    .got : AT(ADDR(.got) - KERNEL_OFFSET) { *(.got .got.*) }
    .got.plt : AT(ADDR(.got.plt) - KERNEL_OFFSET) { *(.got.plt) }

    .bss (NOLOAD) : AT(ADDR(.bss) - KERNEL_OFFSET) ALIGN(16) {
        __bss_start = .;
        *(.bss .bss.*)
        __bss_end = .;
    }

    . = ALIGN(4096);
    _end = .;

    /DISCARD/ : {
        *(.eh_frame)
        *(.note .note.*)
    }
}
