#include "../libc/sys.h"

#define MAX_ARGS 16

void _start(void) {
    char* argv[MAX_ARGS];
    int argc = get_argv(argv, MAX_ARGS);
    while (1) {
        if (argc == 1) { writeln(argv[0]); } else { write("yes"); }
    }
    exit(0);
}
