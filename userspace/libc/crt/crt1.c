// The start of a program that has a main(): turns the command line into
// argc and argv, calls main, and exits with what it returns. /bin/tcc
// links it into every program it builds (unless -nostartfiles), and clang
// builds it into every program with the rest of the library.
//
// _start is weak, so a program with its own _start (everything in
// userspace/prog and userspace/cmd) gets that one instead, and main is a
// weak reference, so such a program needn't have a main at all.
//
// KOS doesn't pass a program its own name, so argv[0] is "program".

#include "../sys.h"

#define MAX_ARGS 64

__attribute__((weak)) int main(int argc, char** argv);

__attribute__((weak)) void _start(void) {
    static char* argv[MAX_ARGS + 1];
    argv[0] = "program";
    int argc = 1 + get_argv(argv + 1, MAX_ARGS - 1);
    argv[argc] = NULL;
    exit(main(argc, argv));
}
