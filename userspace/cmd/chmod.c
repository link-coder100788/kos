// chmod: sets what others may do with files and folders. Their owner
// always may do anything, and so may root; only they can change it.
//
//     chmod <+|-><r|w|rw> <path>...
//
// r: others may read it (a folder: look inside). w: others may write it
// (a folder: add and remove things in it). `chmod -rw ~/diary` makes it
// private; `chmod +r notes.txt` lets everyone read it.
#include "../libc/sys.h"
#include "../libc/stdio.h"

#define MAX_ARGS 32

static void usage(void) {
    dprintf(STDERR_FD, "chmod: usage: chmod <+|-><r|w|rw> <path>...\n");
    exit(2);
}

void _start(void) {
    char* argv[MAX_ARGS];
    int argc = get_argv(argv, MAX_ARGS);
    if (argc < 2 || (argv[0][0] != '+' && argv[0][0] != '-') || !argv[0][1]) usage();
    int on = argv[0][0] == '+', read = -1, write = -1;
    for (const char* f = argv[0] + 1; *f; f++) {
        if (*f == 'r') read = on;
        else if (*f == 'w') write = on;
        else usage();
    }
    int status = 0;
    for (int i = 1; i < argc; i++) {
        if (fs_chmod(argv[i], read, write) < 0) {
            dprintf(STDERR_FD, "chmod: %s: %s\n", argv[i], last_error());
            status = 1;
        }
    }
    exit(status);
}
