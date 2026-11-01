// Prints the 52 characters from 'A' upward ('A'..'Z', then '['..'t') one
// byte at a time with the write_byte syscall, then exits.
.global _start

_start:
    mov x2, #'A'
    mov x3, #52
loop:
    mov x0, x2
    svc #0x1
    add x2, x2, #1
    subs x3, x3, #1
    b.ne loop
    svc #0x0
