// setjmp and longjmp (setjmp.h). A jmp_buf holds what a function must keep
// for its caller: x19-x28, the frame pointer x29, the return address x30,
// sp, and d8-d15. longjmp puts them back, so setjmp's caller carries on as
// if setjmp had just returned, this time with longjmp's value.

.text
.global setjmp
setjmp:
    stp x19, x20, [x0, #0]
    stp x21, x22, [x0, #16]
    stp x23, x24, [x0, #32]
    stp x25, x26, [x0, #48]
    stp x27, x28, [x0, #64]
    stp x29, x30, [x0, #80]
    mov x2, sp
    str x2, [x0, #96]
    stp d8, d9, [x0, #104]
    stp d10, d11, [x0, #120]
    stp d12, d13, [x0, #136]
    stp d14, d15, [x0, #152]
    movz x0, #0
    ret

.global longjmp
longjmp:
    ldp x19, x20, [x0, #0]
    ldp x21, x22, [x0, #16]
    ldp x23, x24, [x0, #32]
    ldp x25, x26, [x0, #48]
    ldp x27, x28, [x0, #64]
    ldp x29, x30, [x0, #80]
    ldr x2, [x0, #96]
    mov sp, x2
    ldp d8, d9, [x0, #104]
    ldp d10, d11, [x0, #120]
    ldp d12, d13, [x0, #136]
    ldp d14, d15, [x0, #152]
    // setjmp returns v, or 1 if v is 0, so it can't look like the first time.
    cbnz x1, 1f
    movz x1, #1
1:  mov x0, x1
    ret
