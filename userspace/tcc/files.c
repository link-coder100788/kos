// Checks what tcclibc gained for tcc: file descriptors and FILE streams
// over whole-file syscalls, errno, setjmp/longjmp, and the number and time
// functions. Prints a PASS or FAIL line for each, then how many failed.
#include "../tcclibc/stdio.h"
#include "../tcclibc/stdlib.h"
#include "../tcclibc/string.h"
#include "../tcclibc/math.h"
#include "../tcclibc/errno.h"
#include "../tcclibc/fcntl.h"
#include "../tcclibc/unistd.h"
#include "../tcclibc/setjmp.h"
#include "../tcclibc/sys/time.h"
#include "../tcclibc/sys.h"

static int failed;

static void check(const char* what, int ok) {
    printf("%s %s\n", ok ? "PASS" : "FAIL", what);
    if (!ok) failed++;
}

static jmp_buf jump;

static void jump_back(int v) {
    longjmp(jump, v);
}

// Kept in d8-d15 across setjmp if the compiler puts it there; checked after.
static double after_jump(volatile double keep) {
    if (setjmp(jump) == 0) jump_back(0);
    return keep;
}

void _start(void) {
    const char* path = "/tcc-files-test.txt";
    char buf[128];

    // File descriptors.
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    check("open for writing", fd >= 3);
    check("close writes the file", close(fd) == 0 && fs_size(path) == 0);
    fd = open(path, O_RDWR);
    FILE* w = fdopen(fd, "r+");
    fputs("hello ", w);
    fprintf(w, "%d %s\n", 42, "world");
    fputc('!', w);
    check("fprintf, fputs, fputc, ftell", ftell(w) == 16);
    fseek(w, 0, SEEK_SET);
    check("fgets after fseek", fgets(buf, sizeof(buf), w) && strcmp(buf, "hello 42 world\n") == 0);
    check("fgetc", fgetc(w) == '!' && fgetc(w) == EOF && feof(w));
    check("nothing on disk before fclose", fs_size(path) == 0);
    check("fclose writes it back", fclose(w) == 0 && fs_size(path) == 16);

    FILE* a = fopen(path, "a");
    fwrite("abc", 1, 3, a);
    fclose(a);
    FILE* r = fopen(path, "rb");
    size_t n = fread(buf, 1, sizeof(buf), r);
    check("append, then fread to the end", n == 19 && memcmp(buf + 16, "abc", 3) == 0 && feof(r));
    check("writing a read-only file fails", fwrite("x", 1, 1, r) == 0 && ferror(r));
    fclose(r);

    fd = open(path, O_RDONLY);
    check("lseek SEEK_END", lseek(fd, -3, SEEK_END) == 16 && read(fd, buf, 3) == 3 && memcmp(buf, "abc", 3) == 0);
    check("read at the end gives 0", read(fd, buf, 10) == 0);
    close(fd);

    fd = open(path, O_WRONLY);
    lseek(fd, 25, SEEK_SET);
    FILE* gap = fdopen(fd, "w");
    fputc('z', gap);
    fclose(gap);
    char* all = fs_read_all(path, &n);
    check("writing past the end leaves zeros", all && n == 26 && all[20] == 0 && all[24] == 0 && all[25] == 'z');
    free(all);

    check("remove", remove(path) == 0 && fs_size(path) < 0);
    errno = 0;
    check("open a missing file: ENOENT", open(path, O_RDONLY) == -1 && errno == ENOENT);
    check("strerror", strcmp(strerror(ENOENT), "no such file or directory") == 0);
    check("fopen a missing file: NULL", fopen(path, "r") == NULL);
    check("bad descriptor: EBADF", close(99) == -1 && errno == EBADF);
    check("getcwd", getcwd(buf, sizeof(buf)) && buf[0] == '/');
    check("getcwd too small: NULL", getcwd(buf, 1) == NULL);

    // setjmp / longjmp.
    volatile int count = 0;
    int v = setjmp(jump);
    count++;
    if (v == 0) jump_back(7);
    check("longjmp returns its value from setjmp", v == 7 && count == 2);
    check("longjmp(j, 0) returns 1, keeps registers", after_jump(2.5) == 2.5);

    // Numbers and time.
    check("strtof, strtold", strtof("0.5", NULL) == 0.5f && strtold("1.25", NULL) == 1.25L);
    check("ldexp", ldexp(3.0, 4) == 48.0 && ldexp(1.0, -1) == 0.5 && ldexp(1.0, 1024) == INFINITY && ldexp(1.0, -1074) > 0);
    struct timeval t1, t2;
    gettimeofday(&t1, NULL);
    sleep_ms(30);
    gettimeofday(&t2, NULL);
    long ms = (t2.tv_sec - t1.tv_sec) * 1000 + (t2.tv_usec - t1.tv_usec) / 1000;
    check("gettimeofday", t1.tv_sec > 1704067200L && ms >= 30 && ms < 1000);
    check("getenv", getenv("HOME") == NULL);

    printf("%d failed\n", failed);
    exit(0);
}
