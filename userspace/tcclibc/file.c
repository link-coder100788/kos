// File descriptors and FILE streams (fcntl.h, unistd.h, stdio.h) over KOS's
// whole-file syscalls: opening a file reads all of it into memory, reads,
// writes and seeks work on that copy, and closing writes it back if it
// changed. Descriptors 0-2 are the console.

#include "stdio.h"
#include "stdlib.h"
#include "string.h"
#include "sys.h"
#include "errno.h"
#include "fcntl.h"
#include "unistd.h"

int errno;

// ---- file descriptors ---------------------------------------------------------

#define MAX_FILES 32

typedef struct {
    int open;
    int access;   // O_RDONLY, O_WRONLY or O_RDWR
    int append;   // every write goes at the end
    int changed;  // write it back on close
    char* path;
    char* data;
    size_t len, cap, pos;
} file_t;

static file_t files[MAX_FILES];

static file_t* get(int fd) {
    if (fd < 3 || fd >= MAX_FILES || !files[fd].open) {
        errno = EBADF;
        return NULL;
    }
    return &files[fd];
}

int open(const char* path, int flags, ...) {
    int fd = 3;
    while (fd < MAX_FILES && files[fd].open) fd++;
    if (fd == MAX_FILES) {
        errno = EMFILE;
        return -1;
    }
    int access = flags & O_ACCMODE;
    int exists = fs_size(path) >= 0;
    if (!exists && !(flags & O_CREAT)) {
        errno = ENOENT;
        return -1;
    }
    if (exists && (flags & O_CREAT) && (flags & O_EXCL)) {
        errno = EEXIST;
        return -1;
    }
    char* data = NULL;
    size_t len = 0;
    int truncate = (flags & O_TRUNC) && access != O_RDONLY;
    if (exists && !truncate) {
        data = fs_read_all(path, &len);
        if (!data) {
            errno = EIO;
            return -1;
        }
    }
    char* copy = strdup(path);
    if (!copy) {
        free(data);
        errno = ENOMEM;
        return -1;
    }
    file_t* f = &files[fd];
    f->open = 1;
    f->access = access;
    f->append = (flags & O_APPEND) != 0;
    // A new or emptied file is written even if nothing is put in it.
    f->changed = access != O_RDONLY && (!exists || truncate);
    f->path = copy;
    f->data = data;
    f->len = f->cap = len;
    f->pos = 0;
    return fd;
}

ssize_t read(int fd, void* buf, size_t n) {
    if (fd == 0) return 0;
    file_t* f = get(fd);
    if (!f) return -1;
    if (f->access == O_WRONLY) {
        errno = EBADF;
        return -1;
    }
    if (f->pos >= f->len) return 0;
    if (n > f->len - f->pos) n = f->len - f->pos;
    memcpy(buf, f->data + f->pos, n);
    f->pos += n;
    return (ssize_t)n;
}

// write() for descriptors; POSIX's name is taken by sys.h's write(str).
static ssize_t fd_write(int fd, const void* buf, size_t n) {
    if (fd == 1 || fd == 2) {
        write_bytes((const uint8_t*)buf, n);
        return (ssize_t)n;
    }
    file_t* f = get(fd);
    if (!f) return -1;
    if (f->access == O_RDONLY) {
        errno = EBADF;
        return -1;
    }
    if (f->append) f->pos = f->len;
    size_t end = f->pos + n;
    if (end > f->cap) {
        size_t cap = f->cap ? f->cap : 4096;
        while (cap < end) cap *= 2;
        char* data = realloc(f->data, cap);
        if (!data) {
            errno = ENOMEM;
            return -1;
        }
        f->data = data;
        f->cap = cap;
    }
    // Writing past the end (after a seek) leaves zeros in the gap.
    if (f->pos > f->len) memset(f->data + f->len, 0, f->pos - f->len);
    memcpy(f->data + f->pos, buf, n);
    f->pos = end;
    if (end > f->len) f->len = end;
    f->changed = 1;
    return (ssize_t)n;
}

off_t lseek(int fd, off_t offset, int whence) {
    file_t* f = get(fd);
    if (!f) return -1;
    off_t base = whence == SEEK_SET ? 0 : whence == SEEK_CUR ? (off_t)f->pos : whence == SEEK_END ? (off_t)f->len : -1;
    if (base < 0 || base + offset < 0) {
        errno = EINVAL;
        return -1;
    }
    f->pos = (size_t)(base + offset);
    return (off_t)f->pos;
}

int close(int fd) {
    if (fd >= 0 && fd < 3) return 0;
    file_t* f = get(fd);
    if (!f) return -1;
    int result = 0;
    // fs_write needs a real pointer even for an empty file.
    if (f->changed && fs_write(f->path, f->data ? f->data : "", f->len) < 0) {
        errno = EIO;
        result = -1;
    }
    free(f->data);
    free(f->path);
    memset(f, 0, sizeof(*f));
    return result;
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
    int result = close(f->fd);
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
    if (fd_write(f->fd, buf, size * n) < 0) {
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
    if (f->fd == 1 || f->fd == 2) return vprintf(format, args);
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
