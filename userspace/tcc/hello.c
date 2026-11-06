// The first program built by tcc: it links with the clang-built C library.
#include "../tcclibc/sys.h"
#include "../tcclibc/stdio.h"
#include "../tcclibc/stdlib.h"

void _start(void) {
    writeln("Hello from tcc!");
    printf("%d + %d = %d, %s, %.2f\n", 2, 3, 2 + 3, "printf works", 3.14159);
    char* s = malloc(32);
    snprintf(s, 32, "malloc'd %d bytes", 32);
    writeln(s);
    free(s);
    exit();
}
