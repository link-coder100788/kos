.section .text.boot
.global _start

_start:
    mrs x0, mpidr_el1
    and x0, x0, #0xFF
    cbz x0, primary_core

secondary_cores:
    mrs x2, mpidr_el1
    and x2, x2, #0xFF        // x2 = Core ID (1, 2, 3, or 4)
    sub x2, x2, #1           // Shift ID to (0, 1, 2, or 3) for clean indexing
    mov x3, #0x10000         // Each core gets a 64 KB stack (0x10000 bytes)
    mul x3, x2, x3           // x3 = (Core ID - 1) * 64 KB
    ldr x4, =0x40080000      // Stack Base limit (Core 0 stack sits safe above this)
    sub sp, x4, x3           // Assign isolated stack pointer
    mov x0, #(3 << 20)
    msr cpacr_el1, x0
    isb
    mrs x0, mpidr_el1
    and x0, x0, #0xFF        // x0 = Core ID
    cmp x0, #2
    b.ls run_secondary       // If Core ID <= 2 (Cores 1 & 2), jump to secondary loop

run_tertiary:
    ldr x1, =0x40202000      // Async core entry address (__tertiary_entry_start)
    br x1

run_secondary:
    ldr x1, =0x40201000      // Sync core entry address (__secondary_entry_start)
    br x1

primary_core:
    ldr x1, =0x40100000
    mov sp, x1
    mov x0, #(3 << 20)
    msr cpacr_el1, x0
    isb
    bl bootloader_main

2:
    wfe
    b 2b
