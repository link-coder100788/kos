#include "../libc/sys.h"
#include "../libc/time.h"
#include "../libc/stdio.h"

#define MAX_ARGS 8
#define MAX_ARG_LEN 1024

void _start(void) {
    char* argv[MAX_ARGS];
    int argc = get_argv(argv, MAX_ARGS);
    long sec = time(0);
    if (argc == 0) { printf("%ld\n", sec); exit(0); }
    char args[MAX_ARG_LEN];
    int pos = 0;
    for (int i = 1; i < 8 && argv[i] != NULL; i++) {
        if (i > 1) args[pos++] = ' ';
        for (int j = 0; argv[i][j] != '\0'; j++) args[pos++] = argv[i][j];
    }
    args[pos] = '\0';
    long id = spawn(argv[0], args);
    long stat = proc_wait(id);
    long sec_f = time(0);
    printf("time %s %ld s stat %ld\n", argv[0], sec_f-sec, stat);
    exit(0);
}
