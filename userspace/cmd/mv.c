#include "../libc/sys.h"

#define MAX_ARGS 16

void _start(void) {
    char* argv[MAX_ARGS];
    int argc = get_argv(argv, MAX_ARGS);
    if (argc != 2) { writeln("mv: usage: mv <source> <dest>"); exit(2); }
    size_t len;
    char* data = fs_read_all(argv[0], &len);
    if (!data) { writeln("mv: failed to read source"); exit(1); }
    if (fs_write(argv[1], data, len) < 0) { writeln("mv: failed to write to dest"); free(data); exit(1); }
    free(data);
    if (fs_remove(argv[0]) < 0) { writeln("mv: copied file but failed to remove source"); exit(1); }
    exit(0);
}
