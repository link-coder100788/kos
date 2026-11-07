// grep: prints the lines of files that contain a piece of text.
//
//     grep [-i] [-n] <text> [file]...
//
// -i ignores case, -n puts each line's number in front of it. With more
// than one file, each line starts with the file's name; with none, it
// reads stdin (`ls | grep txt`). Exits with 0 if any line matched, 1 if
// none did, 2 for an error.
#include "../libc/sys.h"
#include "../libc/stdio.h"

#define MAX_ARGS 32

static int ignore_case = 0;
static int line_numbers = 0;

static char lower(char c) {
    return (c >= 'A' && c <= 'Z') ? c - 'A' + 'a' : c;
}

static int same(char a, char b) {
    return ignore_case ? lower(a) == lower(b) : a == b;
}

// Whether line[0..len] contains the NUL-terminated pattern.
static int contains(const char* line, long len, const char* pattern) {
    long plen = 0;
    while (pattern[plen]) plen++;
    for (long i = 0; i + plen <= len; i++) {
        long j = 0;
        while (j < plen && same(line[i + j], pattern[j])) j++;
        if (j == plen) return 1;
    }
    return 0;
}

// The whole file, or NULL. Its size goes in *len.
static char* read_file(const char* path, long* len) {
    long size = fs_read(path, NULL, 0);
    if (size < 0) return NULL;
    char* data = alloc(size + 1);
    if (!data) return NULL;
    *len = fs_read(path, data, size);
    return data;
}

static int failed = 0;

// Prints the matching lines of one file (stdin if path is NULL); returns
// how many there were.
static int grep_file(const char* path, const char* pattern, int show_name) {
    long len;
    size_t stdin_len;
    char* data = path ? read_file(path, &len) : fd_read_all(STDIN_FD, &stdin_len);
    if (!path) len = (long)stdin_len;
    if (!data) {
        dprintf(STDERR_FD, "grep: %s: can't read it\n", path ? path : "stdin");
        failed = 1;
        return 0;
    }
    int matches = 0;
    long start = 0, number = 1;
    while (start < len) {
        long end = start;
        while (end < len && data[end] != '\n') end++;
        if (contains(data + start, end - start, pattern)) {
            matches++;
            if (show_name) printf("%s:", path);
            if (line_numbers) printf("%d:", (int)number);
            write_bytes((const uint8_t*)data + start, end - start);
            write("\n");
        }
        start = end + 1;
        number++;
    }
    free(data);
    return matches;
}

void _start(void) {
    char* argv[MAX_ARGS];
    int argc = get_argv(argv, MAX_ARGS);
    int i = 0;
    for (; i < argc && argv[i][0] == '-' && argv[i][1]; i++) {
        for (const char* f = argv[i] + 1; *f; f++) {
            if (*f == 'i') ignore_case = 1;
            else if (*f == 'n') line_numbers = 1;
            else {
                dprintf(STDERR_FD, "grep: unknown option -%c\n", *f);
                exit(2);
            }
        }
    }
    if (argc - i < 1) {
        dprintf(STDERR_FD, "usage: grep [-i] [-n] <text> [file]...\n");
        exit(2);
    }
    const char* pattern = argv[i++];
    int files = argc - i, matches = 0;
    if (files == 0) matches = grep_file(NULL, pattern, 0);
    for (; i < argc; i++) {
        matches += grep_file(argv[i], pattern, files > 1);
    }
    exit(failed ? 2 : matches ? 0 : 1);
}
