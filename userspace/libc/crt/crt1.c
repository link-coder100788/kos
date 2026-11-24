// The start of a program /bin/tcc builds (unless -nostartfiles): turns the
// command line into argc and argv, calls main, and exits when it returns.
// KOS doesn't pass a program its own name, so argv[0] is "program".

#include "../sys.h"

#define MAX_ARGS 64

int main(int argc, char** argv);

void _start(void) {
    static char* argv[MAX_ARGS + 1];
    argv[0] = "program";
    int argc = 1 + get_argv(argv + 1, MAX_ARGS - 1);
    argv[argc] = NULL;
    exit(main(argc, argv));
}
