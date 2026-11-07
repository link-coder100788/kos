// The first C program: write, read a line, echo it back.
#include "../libc/sys.h"

void _start(void) {
    writeln("Hello, KOS!");
    write("This string ");
    write("was printed from a functional c program ");
    writeln("compiled on my mac!");
    uint8_t em = 33;
    write_byte(&em);
    writeln("");
    char buf[256];
    long bytes_read = readln(buf, sizeof(buf) - 1);
    buf[bytes_read < 0 ? 0 : bytes_read] = '\0';
    writeln(buf);
    exit(0);
}
