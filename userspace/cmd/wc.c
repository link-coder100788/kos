#include "../libc/sys.h"
#include "../libc/stdio.h"

#define MAX_ARGS 16

void _start(void) {
    char* argv[MAX_ARGS];
    int argc = get_argv(argv, MAX_ARGS);
    if (argc != 1) { writeln("wc: usage: <file>"); exit(); }
    size_t len;
    char* data = fs_read_all(argv[0], &len);
    if (!data) { writeln("wc: failed to read file"); exit(); }
    long lines = 0; long words = 0; long bytes = (long)len;
    int in_word = 0;
    for (size_t i = 0; i < len; i++) {
        char c = data[i];
        if (c == '\n') { lines++; }
        if (c == ' ' || c == '\n' || c == '\t' || c == '\r' || c == '\v' || c == '\f') { in_word = 0; }
        else if (!in_word) { words++; in_word = 1; }
    }
    printf("%ld lines %ld words %ld bytes %s\n", lines, words, bytes, argv[0]);
    free(data);
    exit();
}
