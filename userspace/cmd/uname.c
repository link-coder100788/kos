#include "../libc/sys.h"
#include "../libc/stdio.h"

#define MAX_ARGS 16

void _start(void) {
    char* argv[MAX_ARGS];
    int argc = get_argv(argv, MAX_ARGS);
    utsname_t info;
    if (uname(&info) < 0) { writeln("uname: failed"); exit(); }
    if (argc > 1) { writeln("uname: too many args"); exit(); }

    char flag = argc == 0 ? 's' : (argv[0][0] == '-' ? argv[0][1] : 0);
    switch (flag) {
        case 's': printf("%s\n", info.sysname); break;
        case 'n': printf("%s\n", info.nodename); break;
        case 'r': printf("%s\n", info.release); break;
        case 'v': printf("%s\n", info.version); break;
        case 'm': printf("%s\n", info.machine); break;
        case 'a': printf("%s %s %s %s %s\n", info.sysname, info.nodename, info.release, info.version, info.machine); break;
        default: writeln("uname: usage: uname [-s|-n|-r|-v|-m|-a]");
    }
    exit();
}
