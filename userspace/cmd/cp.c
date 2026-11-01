#include "../libc/sys.h"

#define MAX_ARGS 16

void _start(void) {
    char* argv[MAX_ARGS];
    int argc = get_argv(argv, MAX_ARGS);
    if (argc != 2) { writeln("cp: usage: cp <source> <dest>"); exit(); }
    size_t len;
    char* data = fs_read_all(argv[0], &len);
    if (!data) { writeln("cp: failed to read source"); exit(); }
    if (fs_write(argv[1], data, len) < 0) { writeln("cp: failed to write to dest"); }
    free(data);
    exit();
}
