// File descriptors and FILE streams (fcntl.h, unistd.h, stdio.h) over
// the kernel's file descriptors (sys.h's fd_*): files, pipes and the
// console. The kernel keeps an open file in memory and writes it back when
// its last descriptor is closed. Descriptors 0-2 are stdin, stdout and
// stderr, as whoever started the program set them up.

#include "stdio.h"
#include "stdlib.h"
#include "string.h"
#include "sys.h"
#include "errno.h"
#include "fcntl.h"
#include "unistd.h"

int errno;

// ---- file descriptors ---------------------------------------------------------

// Sets errno from why the kernel said the last call failed, and returns -1.
static int failed(void) {
    const char* why = last_error();
    if (strstr(why, "no such file")) errno = ENOENT;
    else if (strstr(why, "already exists")) errno = EEXIST;
    else if (strstr(why, "too many open files")) errno = EMFILE;
    else if (strstr(why, "bad file descriptor") || strstr(why, "not open for")) errno = EBADF;
    else if (strstr(why, "is a folder") || strstr(why, "not a folder")) errno = EACCES;
    else errno = EIO;
    return -1;
}

int open(const char* path, int flags, ...) {
    long fd = fd_open(path, flags);
    return fd < 0 ? failed() : (int)fd;
}

ssize_t read(int fd, void* buf, size_t n) {
    long got = fd_read(fd, buf, n);
    return got < 0 ? failed() : (ssize_t)got;
}

// write() for descriptors; POSIX's name is taken by sys.h's write(str).
static ssize_t write_fd(int fd, const void* buf, size_t n) {
    long done = fd_write(fd, buf, n);
    return done < 0 ? failed() : (ssize_t)done;
}

off_t lseek(int fd, off_t offset, int whence) {
    long pos = fd_seek(fd, offset, whence);
    return pos < 0 ? failed() : (off_t)pos;
}

int close(int fd) {
    return fd_close(fd) < 0 ? failed() : 0;
}

int dup(int fd) {
    long n = fd_dup(fd);
    return n < 0 ? failed() : (int)n;
}

int dup2(int fd, int to) {
    long n = fd_dup2(fd, to);
    return n < 0 ? failed() : (int)n;
}

int pipe(int fds[2]) {
    return fd_pipe(fds) < 0 ? failed() : 0;
}

int isatty(int fd) {
    return fd_isatty(fd);
}

int unlink(const char* path) {
    if (fs_remove(path) < 0) {
        errno = ENOENT;
        return -1;
    }
    return 0;
}

char* getcwd(char* buf, size_t size) {
    if (fs_cwd(buf, size) < 0) {
        errno = ERANGE;
        return NULL;
    }
    return buf;
}

// ---- FILE streams -------------------------------------------------------------

struct FILE {
    int fd;
    int eof;
    int err;
};

static FILE console[3] = { { 0 }, { 1 }, { 2 } };
FILE* stdin = &console[0];
FILE* stdout = &console[1];
FILE* stderr = &console[2];

FILE* fdopen(int fd, const char* mode) {
    (void)mode;
    FILE* f = alloc_z(sizeof(FILE));
    if (!f) {
        errno = ENOMEM;
        return NULL;
    }
    f->fd = fd;
    return f;
}

FILE* fopen(const char* path, const char* mode) {
    int plus = strchr(mode, '+') != NULL;
    int flags;
    switch (mode[0]) {
        case 'r': flags = plus ? O_RDWR : O_RDONLY; break;
        case 'w': flags = (plus ? O_RDWR : O_WRONLY) | O_CREAT | O_TRUNC; break;
        case 'a': flags = (plus ? O_RDWR : O_WRONLY) | O_CREAT | O_APPEND; break;
        default: errno = EINVAL; return NULL;
    }
    int fd = open(path, flags);
    if (fd < 0) return NULL;
    FILE* f = fdopen(fd, mode);
    if (!f) close(fd);
    return f;
}

int fclose(FILE* f) {
    int result = f >= console && f <= &console[2] ? 0 : close(f->fd);
    if (f < console || f > &console[2]) free(f);
    return result < 0 ? EOF : 0;
}

int fflush(FILE* f) {
    (void)f;
    return 0;
}

size_t fread(void* buf, size_t size, size_t n, FILE* f) {
    size_t want = size * n;
    if (want == 0) return 0;
    ssize_t got = read(f->fd, buf, want);
    if (got < 0) {
        f->err = 1;
        return 0;
    }
    if ((size_t)got < want) f->eof = 1;
    return (size_t)got / size;
}

size_t fwrite(const void* buf, size_t size, size_t n, FILE* f) {
    if (size * n == 0) return 0;
    if (write_fd(f->fd, buf, size * n) < 0) {
        f->err = 1;
        return 0;
    }
    return n;
}

int fgetc(FILE* f) {
    unsigned char c;
    return fread(&c, 1, 1, f) == 1 ? c : EOF;
}

char* fgets(char* buf, int size, FILE* f) {
    int n = 0;
    while (n < size - 1) {
        int c = fgetc(f);
        if (c == EOF) break;
        buf[n++] = (char)c;
        if (c == '\n') break;
    }
    if (n == 0) return NULL;
    buf[n] = '\0';
    return buf;
}

int fputc(int c, FILE* f) {
    unsigned char b = (unsigned char)c;
    return fwrite(&b, 1, 1, f) == 1 ? b : EOF;
}

int fputs(const char* s, FILE* f) {
    size_t len = strlen(s);
    return len == 0 || fwrite(s, len, 1, f) == 1 ? 0 : EOF;
}

int vfprintf(FILE* f, const char* format, va_list args) {
    if (f->fd == 1 || f->fd == 2) return vdprintf(f->fd, format, args);
    va_list again;
    va_copy(again, args);
    int n = vsnprintf(NULL, 0, format, args);
    char* text = alloc(n + 1);
    if (!text) {
        va_end(again);
        f->err = 1;
        return -1;
    }
    vsnprintf(text, n + 1, format, again);
    va_end(again);
    size_t written = n ? fwrite(text, n, 1, f) : 1;
    free(text);
    return written == 1 ? n : -1;
}

int fprintf(FILE* f, const char* format, ...) {
    va_list args;
    va_start(args, format);
    int n = vfprintf(f, format, args);
    va_end(args);
    return n;
}

int fseek(FILE* f, long offset, int whence) {
    if (lseek(f->fd, offset, whence) < 0) return -1;
    f->eof = 0;
    return 0;
}

long ftell(FILE* f) {
    return lseek(f->fd, 0, SEEK_CUR);
}

void rewind(FILE* f) {
    fseek(f, 0, SEEK_SET);
    f->err = 0;
}

int feof(FILE* f) { return f->eof; }
int ferror(FILE* f) { return f->err; }
int fileno(FILE* f) { return f->fd; }

int remove(const char* path) {
    return unlink(path);
}

// ---- assert.h -----------------------------------------------------------------

void __assert_fail(const char* expr, const char* file, int line) {
    printf("%s:%d: assertion failed: %s\n", file, line, expr);
    exit(1);
}
