// encode: turns a file into base64 text, as programs are kept on the disk
// (.elf.b64).
//
//     encode [in [out]]
//
// Without `in` it reads stdin; without `out` it prints the text.
#include "../libc/sys.h"
#include "../libc/stdio.h"
#include "../libc/crypto.h"

#define MAX_ARGS 4

void _start(void) {
    char* argv[MAX_ARGS];
    int argc = get_argv(argv, MAX_ARGS);
    if (argc > 2) {
        dprintf(STDERR_FD, "usage: encode [in [out]]\n");
        exit(2);
    }
    size_t len;
    char* data = argc > 0 ? fs_read_all(argv[0], &len) : fd_read_all(STDIN_FD, &len);
    if (!data) {
        dprintf(STDERR_FD, "encode: %s: %s\n", argc > 0 ? argv[0] : "stdin", last_error());
        exit(1);
    }
    char* text = alloc(BASE64_LEN(len) + 1);
    if (!text) {
        dprintf(STDERR_FD, "encode: out of memory\n");
        exit(1);
    }
    size_t n = base64_encode(text, data, len);
    if (argc == 2) {
        if (fs_write(argv[1], text, n) < 0) {
            dprintf(STDERR_FD, "encode: %s: %s\n", argv[1], last_error());
            exit(1);
        }
    } else {
        text[n++] = '\n';
        fd_write(STDOUT_FD, text, n);
    }
    exit(0);
}
