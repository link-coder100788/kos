#ifndef C_SYSCALL_H
#define C_SYSCALL_H

// The one way into the kernel (syscall.S): every syscall wrapper in this
// library calls kos_syscall, since tcc can't compile inline asm that does
// `svc`. Syscall `nr` gets a0..a4 in x0..x4 and the sub-command in x16;
// the result is what it leaves in x0. An `nr` it doesn't know returns -1.

#include <stdint.h>

uint64_t kos_syscall(uint64_t a0, uint64_t a1, uint64_t a2, uint64_t a3, uint64_t a4,
                     uint64_t sub, uint64_t nr);

// kos_syscall with the syscall number and sub-command first, and the
// arguments cast to what it takes.
#define KOS_SVC(nr, sub, a0, a1, a2, a3, a4) \
    kos_syscall((uint64_t)(a0), (uint64_t)(a1), (uint64_t)(a2), (uint64_t)(a3), (uint64_t)(a4), (uint64_t)(sub), (nr))

#endif //C_SYSCALL_H
