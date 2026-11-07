// kill: ends programs, by the ids `ps` and the Task Manager show.
//
//     kill <id>...
#include "../libc/sys.h"
#include "../libc/stdio.h"
#include "../libc/stdlib.h"

#define MAX_ARGS 32

void _start(void) {
    char* argv[MAX_ARGS];
    int argc = get_argv(argv, MAX_ARGS);
    if (argc == 0) {
        dprintf(STDERR_FD, "usage: kill <id>... (see ps)\n");
        exit(2);
    }
    int status = 0;
    for (int i = 0; i < argc; i++) {
        char* end;
        long id = strtol(argv[i], &end, 10);
        if (*end || id <= 0) {
            dprintf(STDERR_FD, "kill: %s: not an id\n", argv[i]);
            status = 1;
        } else if (proc_kill(id) < 0) {
            dprintf(STDERR_FD, "kill: %s: %s\n", argv[i], last_error());
            status = 1;
        }
    }
    exit(status);
}
