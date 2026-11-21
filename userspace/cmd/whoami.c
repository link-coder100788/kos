// whoami: prints the name of the user this runs as.
#include "../libc/sys.h"
#include "../libc/stdio.h"

void _start(void) {
    char name[32];
    if (user_name(getuid(), name, sizeof name) < 0) {
        dprintf(STDERR_FD, "whoami: %s\n", last_error());
        exit(1);
    }
    printf("%s\n", name);
    exit(0);
}
