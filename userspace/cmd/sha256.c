#include "../libc/sys.h"
#include "../libc/stdio.h"
#include "../libc/crypto.h"

#define MAX_ARGS 16

void _start(void) {
    char* argv[MAX_ARGS];
    int argc = get_argv(argv, MAX_ARGS);
    if (argc == 0) { writeln("sha256: usage: <file>..."); exit(); }
    for (int i = 0; i < argc; i++) {
        size_t len;
        char* data = fs_read_all(argv[i], &len);
        if (!data) { printf("sha256: %s\n", last_error()); continue; }
        uint8_t hash[SHA256_LEN];
        if (sha256(data, len, hash) == 0) {
            for (int j = 0; j < SHA256_LEN; j++) printf("%02x", hash[j]);
            printf("  %s\n", argv[i]);
        } else {
            printf("sha256: %s: %s\n", argv[i], last_error());
        }
        free(data);
    }
    exit();
}
