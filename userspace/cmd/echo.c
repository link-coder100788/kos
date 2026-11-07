#include "../libc/sys.h"

#define MAX_ARGS 16

void _start(void) {
    char* argv[MAX_ARGS];
    int argc = get_argv(argv, MAX_ARGS);
    for (int i = 0; i < argc; i++) {
        if (i > 0) { write(" "); }
        write(argv[i]);
    }
    write("\n");
    exit(0);
}
