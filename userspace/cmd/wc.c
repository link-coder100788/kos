// wc: counts the lines, words and bytes of a file, or of stdin if none is
// given (`cat notes.txt | wc`).
#include "../libc/sys.h"
#include "../libc/stdio.h"

#define MAX_ARGS 16

void _start(void) {
    char* argv[MAX_ARGS];
    int argc = get_argv(argv, MAX_ARGS);
    if (argc > 1) { dprintf(STDERR_FD, "wc: usage: wc [file]\n"); exit(2); }
    size_t len;
    char* data = argc ? fs_read_all(argv[0], &len) : fd_read_all(STDIN_FD, &len);
    if (!data) { dprintf(STDERR_FD, "wc: failed to read %s\n", argc ? argv[0] : "stdin"); exit(1); }
    long lines = 0; long words = 0; long bytes = (long)len;
    int in_word = 0;
    for (size_t i = 0; i < len; i++) {
        char c = data[i];
        if (c == '\n') { lines++; }
        if (c == ' ' || c == '\n' || c == '\t' || c == '\r' || c == '\v' || c == '\f') { in_word = 0; }
        else if (!in_word) { words++; in_word = 1; }
    }
    printf("%ld lines %ld words %ld bytes%s%s\n", lines, words, bytes, argc ? " " : "", argc ? argv[0] : "");
    free(data);
    exit(0);
}
