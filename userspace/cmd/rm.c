// rm: deletes files and empty folders.
//
//     rm <path>...
#include "../libc/sys.h"
#include "../libc/stdio.h"

#define MAX_ARGS 32

void _start(void) {
    char* argv[MAX_ARGS];
    int argc = get_argv(argv, MAX_ARGS);
    if (argc == 0) {
        dprintf(STDERR_FD, "usage: rm <file or empty folder>...\n");
        exit(2);
    }
    int status = 0;
    for (int i = 0; i < argc; i++) {
        if (fs_remove(argv[i]) < 0) {
            dprintf(STDERR_FD, "rm: %s: %s\n", argv[i], last_error());
            status = 1;
        }
    }
    exit(status);
}
