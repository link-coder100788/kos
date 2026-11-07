// cat: prints files one after another, or stdin if none are given (so it
// works in a pipe, and on the console until Ctrl+D).
//
//     cat [file]...
#include "../libc/sys.h"
#include "../libc/stdio.h"

#define MAX_ARGS 32

void _start(void) {
    char* argv[MAX_ARGS];
    int argc = get_argv(argv, MAX_ARGS);
    int status = 0;
    if (argc == 0) {
        char buf[4096];
        long n;
        while ((n = fd_read(STDIN_FD, buf, sizeof buf)) > 0) fd_write(STDOUT_FD, buf, n);
        exit(n < 0 ? 1 : 0);
    }
    for (int i = 0; i < argc; i++) {
        size_t len;
        char* data = fs_read_all(argv[i], &len);
        if (!data) {
            dprintf(STDERR_FD, "cat: %s: %s\n", argv[i], last_error());
            status = 1;
            continue;
        }
        fd_write(STDOUT_FD, data, len);
        free(data);
    }
    exit(status);
}
