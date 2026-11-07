// ls: lists folders.
//
//     ls [-a] [-l] [path]...
//
// -a shows names starting with '.', -l puts each one's size in front.
// Folders end in '/'. On the console names go in columns; into a pipe or
// a file (`ls | grep txt`), one a line.
#include "../libc/sys.h"
#include "../libc/stdio.h"
#include "../libc/stdlib.h"
#include "../libc/string.h"

#define MAX_ARGS 32
#define WIDTH 80

static int all = 0, longform = 0;

static int compare(const void* a, const void* b) {
    return strcmp(((const fs_entry_t*)a)->name, ((const fs_entry_t*)b)->name);
}

static void size_text(uint32_t size, char* out, size_t n) {
    if (size < 1024) snprintf(out, n, "%u B", (unsigned)size);
    else if (size < 1024 * 1024) snprintf(out, n, "%u.%u K", (unsigned)(size / 1024), (unsigned)(size % 1024 * 10 / 1024));
    else snprintf(out, n, "%u.%u M", (unsigned)(size >> 20), (unsigned)((size & 0xfffff) * 10 >> 20));
}

static void show(fs_entry_t* entries, int count) {
    if (longform) {
        for (int i = 0; i < count; i++) {
            char size[16] = "-";
            if (!entries[i].is_dir) size_text(entries[i].size, size, sizeof size);
            printf("%9s  %s%s\n", size, entries[i].name, entries[i].is_dir ? "/" : "");
        }
        return;
    }
    if (!fd_isatty(STDOUT_FD)) {
        for (int i = 0; i < count; i++) printf("%s%s\n", entries[i].name, entries[i].is_dir ? "/" : "");
        return;
    }
    int width = 0;
    for (int i = 0; i < count; i++) {
        int w = (int)strlen(entries[i].name) + entries[i].is_dir;
        if (w > width) width = w;
    }
    width += 2;
    int columns = WIDTH / width > 0 ? WIDTH / width : 1;
    int rows = (count + columns - 1) / columns;
    for (int r = 0; r < rows; r++) {
        for (int c = 0; c < columns; c++) {
            int i = c * rows + r;
            if (i >= count) continue;
            printf("%s%s", entries[i].name, entries[i].is_dir ? "/" : "");
            if (c + 1 < columns && i + rows < count)
                printf("%*s", width - (int)strlen(entries[i].name) - entries[i].is_dir, "");
        }
        printf("\n");
    }
}

// Lists one folder (or names one file). Returns 0, or 1 if it isn't there.
static int list(const char* path, int heading) {
    long size = fs_size(path);
    if (size >= 0) {
        fs_entry_t e = { .is_dir = 0, .size = (uint32_t)size };
        snprintf(e.name, sizeof e.name, "%s", path);
        show(&e, 1);
        return 0;
    }
    int count;
    fs_entry_t* entries = fs_list(path, &count);
    if (!entries) {
        dprintf(STDERR_FD, "ls: %s: no such file or folder\n", path);
        return 1;
    }
    int shown = 0;
    for (int i = 0; i < count; i++)
        if (all || entries[i].name[0] != '.') entries[shown++] = entries[i];
    qsort(entries, shown, sizeof *entries, compare);
    if (heading) printf("%s:\n", path);
    show(entries, shown);
    free(entries);
    return 0;
}

void _start(void) {
    char* argv[MAX_ARGS];
    int argc = get_argv(argv, MAX_ARGS);
    int paths = 0, status = 0;
    for (int i = 0; i < argc; i++) {
        if (argv[i][0] != '-') {
            paths++;
            continue;
        }
        for (const char* f = argv[i] + 1; *f; f++) {
            if (*f == 'a') all = 1;
            else if (*f == 'l') longform = 1;
            else {
                dprintf(STDERR_FD, "ls: unknown option -%c (try -a, -l)\n", *f);
                exit(2);
            }
        }
    }
    if (paths == 0) exit(list(".", 0));
    for (int i = 0; i < argc; i++) {
        if (argv[i][0] == '-') continue;
        status |= list(argv[i], paths > 1);
    }
    exit(status);
}
