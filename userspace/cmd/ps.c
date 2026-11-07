// ps: lists every task, kernel and program, with the core it runs on, its
// state and how much of its core's time it has had. The ids are what
// `kill` takes.
#include "../libc/sys.h"
#include "../libc/stdio.h"
#include "../libc/string.h"

void _start(void) {
    long len = proc_tasks(NULL, 0);
    char* text = len >= 0 ? alloc(len + 1) : NULL;
    if (!text || proc_tasks(text, len) < 0) {
        dprintf(STDERR_FD, "ps: %s\n", last_error());
        exit(1);
    }
    text[len] = '\0';
    printf("  ID  CORE  STATE      CPU  KIND          NAME\n");
    char* save;
    for (char* line = strtok_r(text, "\n", &save); line; line = strtok_r(NULL, "\n", &save)) {
        char* f[6];
        int n = 0;
        char* field_save;
        for (char* p = strtok_r(line, "\t", &field_save); p && n < 6; p = strtok_r(NULL, "\t", &field_save)) f[n++] = p;
        if (n == 3 && !strcmp(f[0], "core")) printf("core %s idle %s%% of the time since boot\n", f[1], f[2]);
        else if (n == 6) printf("%4s  %4s  %-9s %3s%%  %-13s %s\n", f[0], f[1], f[2], f[3], f[4], f[5]);
    }
    exit(0);
}
