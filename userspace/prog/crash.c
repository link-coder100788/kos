// Writes to an unmapped address on purpose: the kernel should kill this
// program and keep running.
#include "../libc/sys.h"

void _start(void) {
    writeln("About to write to an unmapped address...");
    *(volatile int*)0x1000 = 42;
    writeln("This line should never print");
    exit();
}
