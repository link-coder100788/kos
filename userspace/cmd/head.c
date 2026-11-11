// head: the first lines of a file, or of stdin if none is given
// (`ls /bin | head -n 3`).
//
//     head [-n lines] [file]
//
// It stops reading once it has them, so `yes | head` ends.
#include "../libc/sys.h"
#include "../libc/stdio.h"
#include "../libc/stdlib.h"
#include "../libc/string.h"

#define MAX_ARGS 16

/// Writes up to `*left` lines of data[0..len) and counts them off.
static void show(const char* data, size_t len, long* left) {
    size_t end = 0;
    while (end < len && *left > 0)
        if (data[end++] == '\n') (*left)--;
    fd_write(STDOUT_FD, data, end);
}

void _start(void) {
    char* argv[MAX_ARGS];
    int argc = get_argv(argv, MAX_ARGS);
    long lines = 10;
    int i = 0;
    if (i + 1 < argc && strcmp(argv[i], "-n") == 0) {
        lines = strtol(argv[i + 1], NULL, 10);
        i += 2;
    }
    if (argc - i > 1 || lines < 0) { dprintf(STDERR_FD, "head: usage: head [-n lines] [file]\n"); exit(2); }
    if (i < argc) {
        size_t len;
        char* data = fs_read_all(argv[i], &len);
        if (!data) { dprintf(STDERR_FD, "head: %s: %s\n", argv[i], last_error()); exit(1); }
        show(data, len, &lines);
        free(data);
        exit(0);
    }
    char buf[4096];
    while (lines > 0) {
        long n = fd_read(STDIN_FD, buf, sizeof buf);
        if (n <= 0) break;
        show(buf, n, &lines);
    }
    exit(0);
}
