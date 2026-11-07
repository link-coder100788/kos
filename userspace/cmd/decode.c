// decode: turns base64 text back into the file it was made from, e.g. a
// program kept as .elf.b64 into a .elf.
//
//     decode [in [out]]
//
// Without `in` it reads stdin; without `out` it writes the bytes to stdout.
#include "../libc/sys.h"
#include "../libc/stdio.h"
#include "../libc/crypto.h"

#define MAX_ARGS 4

void _start(void) {
    char* argv[MAX_ARGS];
    int argc = get_argv(argv, MAX_ARGS);
    if (argc > 2) {
        dprintf(STDERR_FD, "usage: decode [in [out]]\n");
        exit(2);
    }
    size_t len;
    char* text = argc > 0 ? fs_read_all(argv[0], &len) : fd_read_all(STDIN_FD, &len);
    if (!text) {
        dprintf(STDERR_FD, "decode: %s: %s\n", argc > 0 ? argv[0] : "stdin", last_error());
        exit(1);
    }
    uint8_t* data = alloc(len / 4 * 3 + 3);
    if (!data) {
        dprintf(STDERR_FD, "decode: out of memory\n");
        exit(1);
    }
    long n = base64_decode(data, text, len);
    if (n < 0) {
        dprintf(STDERR_FD, "decode: %s isn't base64\n", argc > 0 ? argv[0] : "stdin");
        exit(1);
    }
    if (argc == 2) {
        if (fs_write(argv[1], data, n) < 0) {
            dprintf(STDERR_FD, "decode: %s: %s\n", argv[1], last_error());
            exit(1);
        }
    } else {
        fd_write(STDOUT_FD, data, n);
    }
    exit(0);
}
