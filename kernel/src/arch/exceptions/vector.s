// Exception vectors. Every exception saves a full TrapFrame (see
// scheduler/task.rs for the layout) on the current stack. Sync and IRQ
// handlers return the frame to resume, which may belong to a different
// task: that's how context switches happen.
//
// Exceptions from programs (EL0) land on SP_EL1, which is left pointing
// just above the program's saved frame on its kernel stack whenever we
// return to it, so every task traps onto its own kernel stack.

.equ FRAME_SIZE, 816

.macro save_frame
    sub sp, sp, #FRAME_SIZE
    stp x0, x1, [sp, #16 * 0]
    stp x2, x3, [sp, #16 * 1]
    stp x4, x5, [sp, #16 * 2]
    stp x6, x7, [sp, #16 * 3]
    stp x8, x9, [sp, #16 * 4]
    stp x10, x11, [sp, #16 * 5]
    stp x12, x13, [sp, #16 * 6]
    stp x14, x15, [sp, #16 * 7]
    stp x16, x17, [sp, #16 * 8]
    stp x18, x19, [sp, #16 * 9]
    stp x20, x21, [sp, #16 * 10]
    stp x22, x23, [sp, #16 * 11]
    stp x24, x25, [sp, #16 * 12]
    stp x26, x27, [sp, #16 * 13]
    stp x28, x29, [sp, #16 * 14]
    str x30, [sp, #240]
    mrs x0, elr_el1
    mrs x1, spsr_el1
    stp x0, x1, [sp, #248]
    mrs x0, fpcr
    mrs x1, fpsr
    stp x0, x1, [sp, #264]
    str xzr, [sp, #280]
    mrs x0, sp_el0
    str x0, [sp, #288]
    add x0, sp, #304
    stp q0, q1, [x0, #32 * 0]
    stp q2, q3, [x0, #32 * 1]
    stp q4, q5, [x0, #32 * 2]
    stp q6, q7, [x0, #32 * 3]
    stp q8, q9, [x0, #32 * 4]
    stp q10, q11, [x0, #32 * 5]
    stp q12, q13, [x0, #32 * 6]
    stp q14, q15, [x0, #32 * 7]
    stp q16, q17, [x0, #32 * 8]
    stp q18, q19, [x0, #32 * 9]
    stp q20, q21, [x0, #32 * 10]
    stp q22, q23, [x0, #32 * 11]
    stp q24, q25, [x0, #32 * 12]
    stp q26, q27, [x0, #32 * 13]
    stp q28, q29, [x0, #32 * 14]
    stp q30, q31, [x0, #32 * 15]
.endm

// Restores the frame at sp and returns from the exception.
.macro restore_frame_eret
    ldr x0, [sp, #288]
    msr sp_el0, x0
    add x0, sp, #304
    ldp q0, q1, [x0, #32 * 0]
    ldp q2, q3, [x0, #32 * 1]
    ldp q4, q5, [x0, #32 * 2]
    ldp q6, q7, [x0, #32 * 3]
    ldp q8, q9, [x0, #32 * 4]
    ldp q10, q11, [x0, #32 * 5]
    ldp q12, q13, [x0, #32 * 6]
    ldp q14, q15, [x0, #32 * 7]
    ldp q16, q17, [x0, #32 * 8]
    ldp q18, q19, [x0, #32 * 9]
    ldp q20, q21, [x0, #32 * 10]
    ldp q22, q23, [x0, #32 * 11]
    ldp q24, q25, [x0, #32 * 12]
    ldp q26, q27, [x0, #32 * 13]
    ldp q28, q29, [x0, #32 * 14]
    ldp q30, q31, [x0, #32 * 15]
    ldp x0, x1, [sp, #264]
    msr fpcr, x0
    msr fpsr, x1
    ldp x0, x1, [sp, #248]
    msr elr_el1, x0
    msr spsr_el1, x1
    ldp x0, x1, [sp, #16 * 0]
    ldp x2, x3, [sp, #16 * 1]
    ldp x4, x5, [sp, #16 * 2]
    ldp x6, x7, [sp, #16 * 3]
    ldp x8, x9, [sp, #16 * 4]
    ldp x10, x11, [sp, #16 * 5]
    ldp x12, x13, [sp, #16 * 6]
    ldp x14, x15, [sp, #16 * 7]
    ldp x16, x17, [sp, #16 * 8]
    ldp x18, x19, [sp, #16 * 9]
    ldp x20, x21, [sp, #16 * 10]
    ldp x22, x23, [sp, #16 * 11]
    ldp x24, x25, [sp, #16 * 12]
    ldp x26, x27, [sp, #16 * 13]
    ldp x28, x29, [sp, #16 * 14]
    ldr x30, [sp, #240]
    add sp, sp, #FRAME_SIZE
    eret
.endm

.section .text.vectors
.align 11
.global exception_vector_table

exception_vector_table:
// Current EL with SP_EL0
.align 7
    b sync_entry
.align 7
    b irq_entry
.align 7
    b fiq_entry
.align 7
    b serror_entry
// Current EL with SP_ELx
.align 7
    b sync_entry
.align 7
    b irq_entry
.align 7
    b fiq_entry
.align 7
    b serror_entry
// Lower EL, AArch64
.align 7
    b sync_entry
.align 7
    b irq_entry
.align 7
    b fiq_entry
.align 7
    b serror_entry
// Lower EL, AArch32
.align 7
    b sync_entry
.align 7
    b irq_entry
.align 7
    b fiq_entry
.align 7
    b serror_entry

sync_entry:
    save_frame
    mov x0, sp
    bl rust_sync_handler
    mov sp, x0
    restore_frame_eret

irq_entry:
    save_frame
    mov x0, sp
    bl rust_irq_handler
    mov sp, x0
    restore_frame_eret

fiq_entry:
    save_frame
    bl rust_fiq_handler
    restore_frame_eret

serror_entry:
    save_frame
    bl rust_serror_handler
1:  wfe
    b 1b

// start_first_task(frame: *const TrapFrame) -> !
// Switches to a task's initial frame without coming from an exception.
.section .text
.global start_first_task
start_first_task:
    mov sp, x0
    restore_frame_eret
