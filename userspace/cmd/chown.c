// chown: gives files and folders to another user. Only root can.
//
//     chown <user> <path>...
#include "../libc/sys.h"
#include "../libc/stdio.h"

#define MAX_ARGS 32

void _start(void) {
    char* argv[MAX_ARGS];
    int argc = get_argv(argv, MAX_ARGS);
    if (argc < 2) {
        dprintf(STDERR_FD, "chown: usage: chown <user> <path>...\n");
        exit(2);
    }
    long uid = user_id(argv[0]);
    if (uid < 0) {
        dprintf(STDERR_FD, "chown: %s: no such user\n", argv[0]);
        exit(1);
    }
    int status = 0;
    for (int i = 1; i < argc; i++) {
        if (fs_chown(argv[i], uid) < 0) {
            dprintf(STDERR_FD, "chown: %s: %s\n", argv[i], last_error());
            status = 1;
        }
    }
    exit(status);
}
