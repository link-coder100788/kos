#include "../libc/sys.h"
#include "../libc/stdio.h"

void _start(void) {
    size_t mem = mem_free();
    printf("free: %ld b (%ld mb) heap remaining\n", mem, mem / 1024 / 1024);
    exit();
}
