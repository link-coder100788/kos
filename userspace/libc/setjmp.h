#ifndef C_SETJMP_H
#define C_SETJMP_H

// setjmp(j) returns 0, and longjmp(j, v) later returns from that setjmp a
// second time, with v (1 if v is 0). The function that called setjmp must
// not have returned yet. See setjmp.S.

// x19-x30, sp and d8-d15: what a function must keep for its caller.
typedef unsigned long jmp_buf[21];

int setjmp(jmp_buf j);
__attribute__((noreturn)) void longjmp(jmp_buf j, int v);

#endif //C_SETJMP_H
