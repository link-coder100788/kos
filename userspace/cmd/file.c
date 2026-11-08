#include "../libc/sys.h"
#include "../libc/stdio.h"

#define MAX_ARGS 8
#define MAX_MAGIC 12

unsigned char elf[]  = {0x7F, 0x45, 0x4C, 0x46};
unsigned char png[]  = {0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A};
unsigned char bmp[]  = {0x42, 0x4D};
unsigned char jpeg[] = {0xFF, 0xD8, 0xFF};
unsigned char heic[] = {0x66, 0x74, 0x79, 0x70, 0x68, 0x65, 0x69, 0x63};
unsigned char wav[]  = {0x52, 0x49, 0x46, 0x46};
unsigned char mov[]  = {0x66, 0x74, 0x79, 0x70, 0x71, 0x74, 0x20, 0x20};

char* get_extension(char* str) {
    while (*str) {
        if (*str == '.') return str + 1;
        str++;
    }
    return 0;
}

int streq(char* a, char* b) {
    while (*a && *b) {
        if (*a != *b) return 0;
        a++;
        b++;
    }
    return *a == *b;
}

int has_magic(unsigned char* data, unsigned char* magic, int len) {
    for (int i = 0; i < len; i++) {
        if (data[i] != magic[i]) return 0;
    }
    return 1;
}

void _start(void) {
    char* argv[MAX_ARGS];
    int argc = get_argv(argv, MAX_ARGS);
    if (argc == 0) { printf("no file specified\n"); exit(1); }
    char* path = argv[0];
    char* ext = get_extension(path);
    if (fs_size(path) == -1) { printf("file does not exist or is a directory\n"); exit(1); }
    if (ext) {
        if (streq(ext, "elf")) { printf("%s: ELF executable (ext)\n", path); exit(0); }
        else if (streq(ext, "txt")) { printf("%s: ASCII text (ext)\n", path); exit(0); }
        else if (streq(ext, "b64")) { printf("%s: ASCII Base-64 encoded text (ext)\n", path); exit(0); }
        else if (streq(ext, "c")) { printf("%s: C source file (ext)\n", path); exit(0); }
        else if (streq(ext, "h")) { printf("%s: C header file (ext)\n", path); exit(0); }
        else if (streq(ext, "png")) { printf("%s: PNG image (ext)\n", path); exit(0); }
        else if (streq(ext, "bmp")) { printf("%s: BMP image (ext)\n", path); exit(0); }
        else if (streq(ext, "jpg")) { printf("%s: JPEG image (ext)\n", path); exit(0); }
        else if (streq(ext, "jpeg")) { printf("%s: JPEG image (ext)\n", path); exit(0); }
        else if (streq(ext, "heic")) { printf("%s: HEIC image (ext)\n", path); exit(0); }
        else if (streq(ext, "wav")) { printf("%s: WAV audio (ext)\n", path); exit(0); }
        else if (streq(ext, "mov")) { printf("%s: MOV Video (ext)\n", path); exit(0); }
        else printf("%s: unknown file type (ext)\n", path);
    }
    unsigned char data[MAX_MAGIC];
    long size = fs_read(path, data, MAX_MAGIC);
    if (size >= 4 && has_magic(data, elf, 4)) printf("%s: ELF executable (magic)\n", path);
    else if (size >= 8 && has_magic(data, png, 8)) printf("%s: PNG image (magic)\n", path);
    else if (size >= 2 && has_magic(data, bmp, 2)) printf("%s: BMP image (magic)\n", path);
    else if (size >= 3 && has_magic(data, jpeg, 3)) printf("%s: JPEG image (magic)\n", path);
    else if (size >= 12 && has_magic(data + 4, heic, 4) && has_magic(data + 8, heic + 4, 4)) printf("%s: HEIC image (magic)\n", path);
    else if (size >= 12 && has_magic(data, wav, 4) && data[8] == 'W' && data[9] == 'A' && data[10] == 'V' && data[11] == 'E') printf("%s: WAV audio (magic)\n", path);
    else if (size >= 12 && has_magic(data + 4, mov, 8)) printf("%s: QuickTime MOV video (magic)\n", path);
    else { printf("%s: unknown file type (magic)\n", path); exit(1); }
    exit(0);
}
