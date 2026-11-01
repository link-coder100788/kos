// Early boot: the only kernel code that runs with the MMU off.
//
// The bootloader jumps here at physical addresses (0x4020_0000 for core 0,
// 0x4020_1000 / 0x4020_2000 for secondary cores), but the kernel is linked
// at KERNEL_OFFSET + physical. So this code only uses PC-relative
// addressing (adrp, bl) until the MMU is on, then jumps up to the real
// (high) address of the Rust entry point.
//
// boot_l1 maps the first 2 GiB twice: identity through TTBR0 (so the next
// instruction after enabling the MMU still fetches) and at KERNEL_OFFSET
// through TTBR1. mm::vm::init later replaces it with fine-grained tables.

.equ KERNEL_OFFSET, 0xFFFFFF8000000000

// Must match arch/mmu.rs: Device-nGnRE, Normal WB, Normal NC.
.equ MAIR_VALUE, 0x44FF04
// 39-bit VA for both halves, 4 KiB granules, write-back cacheable inner
// shareable walks, 40-bit PA.
.equ TCR_VALUE, (25 | (1 << 8) | (1 << 10) | (3 << 12) | (25 << 16) | (1 << 24) | (1 << 26) | (3 << 28) | (2 << 30) | (2 << 32))

.section .text.boot, "ax"
.global _kernel_entry
_kernel_entry:
    bl boot_enable_mmu
    ldr x1, =KERNEL_OFFSET
    mov x0, sp
    add sp, x0, x1
    ldr x0, =rust_main
    br x0

// Turns the MMU on with boot_l1. Position independent; clobbers x0.
boot_enable_mmu:
    adrp x0, boot_l1
    msr ttbr0_el1, x0
    msr ttbr1_el1, x0
    ldr x0, =MAIR_VALUE
    msr mair_el1, x0
    ldr x0, =TCR_VALUE
    msr tcr_el1, x0
    isb
    tlbi vmalle1
    dsb ish
    isb
    mrs x0, sctlr_el1
    orr x0, x0, #(1 << 0)       // M: MMU on
    orr x0, x0, #(1 << 2)       // C: data cache
    orr x0, x0, #(1 << 12)      // I: instruction cache
    bic x0, x0, #(1 << 1)       // A: no alignment checks
    bic x0, x0, #(1 << 19)      // WXN
    msr sctlr_el1, x0
    isb
    ret

// A secondary core restarting itself (arch/exceptions/restart_core.rs)
// branches here at its physical address, which stays identity mapped in
// TTBR0. Turns the MMU and caches off and re-enters the bootloader.
.global restart_trampoline
restart_trampoline:
    mrs x0, sctlr_el1
    bic x0, x0, #(1 << 0)
    bic x0, x0, #(1 << 2)
    bic x0, x0, #(1 << 12)
    msr sctlr_el1, x0
    isb
    mov x0, #0
    br x0
.ltorg

.section .text.secondary_entry, "ax"
secondary_stub:
    bl boot_enable_mmu
    ldr x1, =KERNEL_OFFSET
    mov x0, sp
    add sp, x0, x1
    ldr x0, =secondary_core_entry
    br x0
.ltorg

.section .text.tertiary_entry, "ax"
tertiary_stub:
    bl boot_enable_mmu
    ldr x1, =KERNEL_OFFSET
    mov x0, sp
    add sp, x0, x1
    ldr x0, =async_core_entry
    br x0
.ltorg

.section .rodata.boot_l1, "a"
.balign 4096
boot_l1:
    .quad 0x0000000000000401    // 0-1 GiB: flash + MMIO, Device-nGnRE, AF
    .quad 0x0000000040000705    // 1-2 GiB: RAM, Normal WB, inner shareable, AF
    .fill 510, 8, 0
