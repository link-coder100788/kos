// grep: prints the lines of files that contain a piece of text.
//
//     grep [-i] [-n] <text> <file>...
//
// -i ignores case, -n puts each line's number in front of it. With more
// than one file, each line starts with the file's name.
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

// Prints the matching lines of one file; returns how many there were.
static int grep_file(const char* path, const char* pattern, int show_name) {
    long len;
    char* data = read_file(path, &len);
    if (!data) {
        printf("grep: %s: can't read it\n", path);
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
                printf("grep: unknown option -%c\n", *f);
                exit();
            }
        }
    }
    if (argc - i < 2) {
        writeln("usage: grep [-i] [-n] <text> <file>...");
        exit();
    }
    const char* pattern = argv[i++];
    int files = argc - i;
    for (; i < argc; i++) {
        grep_file(argv[i], pattern, files > 1);
    }
    exit();
}
