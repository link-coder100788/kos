// kos_syscall (see syscall.h): sets x16 to the sub-command, then runs
// `svc #nr`. The syscall number is part of the svc instruction itself (the
// kernel reads it from ESR), so there's one svc per number, picked by
// comparing. tcc's assembler can't write svc, so each is spelled as its
// encoding: 0xd4000001 with the number in bits 5..20.

#define SVC(n) .int 0xd4000001 + ((n) << 5)

.text
.global kos_syscall
kos_syscall:
    mov x16, x5
    subs xzr, x6, #0
    beq 0f
    subs xzr, x6, #1
    beq 1f
    subs xzr, x6, #2
    beq 2f
    subs xzr, x6, #3
    beq 3f
    subs xzr, x6, #4
    beq 4f
    subs xzr, x6, #5
    beq 5f
    subs xzr, x6, #6
    beq 6f
    subs xzr, x6, #7
    beq 7f
    subs xzr, x6, #8
    beq 8f
    subs xzr, x6, #9
    beq 9f
    subs xzr, x6, #10
    beq 10f
    subs xzr, x6, #11
    beq 11f
    subs xzr, x6, #12
    beq 12f
    subs xzr, x6, #13
    beq 13f
    subs xzr, x6, #14
    beq 14f
    subs xzr, x6, #15
    beq 15f
    subs xzr, x6, #16
    beq 16f
    subs xzr, x6, #17
    beq 17f
    subs xzr, x6, #18
    beq 18f
    movn x0, #0      // x0 = -1: no such syscall
    ret
0:  SVC(0)
    ret
1:  SVC(1)
    ret
2:  SVC(2)
    ret
3:  SVC(3)
    ret
4:  SVC(4)
    ret
5:  SVC(5)
    ret
6:  SVC(6)
    ret
7:  SVC(7)
    ret
8:  SVC(8)
    ret
9:  SVC(9)
    ret
10:  SVC(10)
    ret
11:  SVC(11)
    ret
12:  SVC(12)
    ret
13:  SVC(13)
    ret
14:  SVC(14)
    ret
15:  SVC(15)
    ret
16:  SVC(16)
    ret
17:  SVC(17)
    ret
18:  SVC(18)
    ret
