// sha256: prints the SHA-256 hash of each file, or of stdin if none is
// given.
#include "../libc/sys.h"
#include "../libc/stdio.h"
#include "../libc/crypto.h"

#define MAX_ARGS 16

void _start(void) {
    char* argv[MAX_ARGS];
    int argc = get_argv(argv, MAX_ARGS);
    int status = 0;
    for (int i = 0; i < argc || (argc == 0 && i == 0); i++) {
        size_t len;
        char* data = argc ? fs_read_all(argv[i], &len) : fd_read_all(STDIN_FD, &len);
        if (!data) { dprintf(STDERR_FD, "sha256: %s\n", last_error()); status = 1; continue; }
        uint8_t hash[SHA256_LEN];
        if (sha256(data, len, hash) == 0) {
            for (int j = 0; j < SHA256_LEN; j++) printf("%02x", hash[j]);
            printf("  %s\n", argc ? argv[i] : "-");
        } else {
            dprintf(STDERR_FD, "sha256: %s: %s\n", argc ? argv[i] : "stdin", last_error());
            status = 1;
        }
        free(data);
    }
    exit(status);
}
