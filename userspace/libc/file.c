// File descriptors and FILE streams (fcntl.h, unistd.h, stdio.h) over
// the kernel's file descriptors (sys.h's fd_*): files, pipes and the
// console. The kernel keeps an open file in memory and writes it back when
// its last descriptor is closed. Descriptors 0-2 are stdin, stdout and
// stderr, as whoever started the program set them up. Then the rest of
// POSIX's files: sys/stat.h, dirent.h, rename().

#include "stdio.h"
#include "stdlib.h"
#include "string.h"
#include "sys.h"
#include "errno.h"
#include "fcntl.h"
#include "unistd.h"
#include "sys/stat.h"
#include "dirent.h"
#include <stdarg.h>

int errno;

// ---- file descriptors ---------------------------------------------------------

// Sets errno from why the kernel said the last call failed (FsError's
// and fd.rs's words), and returns -1.
static int failed(void) {
    const char* why = last_error();
    if (strstr(why, "no such file")) errno = ENOENT;
    else if (strstr(why, "already exists")) errno = EEXIST;
    else if (strstr(why, "too many open files")) errno = EMFILE;
    else if (strstr(why, "bad file descriptor") || strstr(why, "not open for")) errno = EBADF;
    else if (strstr(why, "is a folder")) errno = EISDIR;
    else if (strstr(why, "not a folder")) errno = ENOTDIR;
    else if (strstr(why, "isn't empty")) errno = ENOTEMPTY;
    else if (strstr(why, "disk is full")) errno = ENOSPC;
    else if (strstr(why, "permission denied")) errno = EACCES;
    else if (strstr(why, "bad path") || strstr(why, "bad whence") || strstr(why, "bad offset")) errno = EINVAL;
    else if (strstr(why, "bad buffer")) errno = EFAULT;
    else if (strstr(why, "only files can seek")) errno = ESPIPE;
    else if (strstr(why, "nothing reads from that pipe")) errno = EPIPE;
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

ssize_t write(int fd, const void* buf, size_t n) {
    long done = fd_write(fd, buf, n);
    return done < 0 ? failed() : (ssize_t)done;
}

int creat(const char* path, int mode) {
    (void)mode;
    return open(path, O_WRONLY | O_CREAT | O_TRUNC);
}

int fcntl(int fd, int cmd, ...) {
    kos_stat_t st;
    if (fd_stat(fd, &st) < 0) return failed();
    switch (cmd) {
        case F_DUPFD:
        case F_DUPFD_CLOEXEC: {
            va_list args;
            va_start(args, cmd);
            int from = va_arg(args, int);
            va_end(args);
            // The first descriptor from `from` up that isn't open.
            for (int to = from < 0 ? 0 : from; to < 256; to++) {
                if (fd_stat(to, &st) < 0) return dup2(fd, to);
            }
            errno = EMFILE;
            return -1;
        }
        case F_GETFD: return 0;
        case F_GETFL: return O_RDWR;
        case F_SETFD:
        case F_SETFL: return 0;
        default: errno = EINVAL; return -1;
    }
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
    if (fd_isatty(fd)) return 1;
    errno = ENOTTY;
    return 0;
}

int unlink(const char* path) {
    kos_stat_t st;
    if (fs_stat(path, &st) < 0) return failed();
    if (st.kind == KOS_FOLDER) {
        errno = EISDIR;
        return -1;
    }
    return fs_remove(path) < 0 ? failed() : 0;
}

int rmdir(const char* path) {
    kos_stat_t st;
    if (fs_stat(path, &st) < 0) return failed();
    if (st.kind != KOS_FOLDER) {
        errno = ENOTDIR;
        return -1;
    }
    fs_entry_t* entries;
    int count;
    if ((entries = fs_list(path, &count)) != NULL) {
        free(entries);
        if (count > 0) {
            errno = ENOTEMPTY;
            return -1;
        }
    }
    return fs_remove(path) < 0 ? failed() : 0;
}

int access(const char* path, int mode) {
    kos_stat_t st;
    if (fs_stat(path, &st) < 0) return failed();
    long me = getuid();
    int mine = me == 0 || (long)st.owner == me;
    if (((mode & (R_OK | X_OK)) && !mine && !(st.flags & 1)) || ((mode & W_OK) && !mine && !(st.flags & 2))) {
        errno = EACCES;
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

int chdir(const char* path) {
    return fs_chdir(path) < 0 ? failed() : 0;
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
    if (write(f->fd, buf, size * n) < 0) {
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
    return fs_remove(path) < 0 ? failed() : 0;
}

int rename(const char* from, const char* to) {
    return fs_rename(from, to) < 0 ? failed() : 0;
}

// ---- sys/stat.h ---------------------------------------------------------------

static void to_stat(const kos_stat_t* k, struct stat* st) {
    memset(st, 0, sizeof *st);
    switch (k->kind) {
        case KOS_FOLDER: st->st_mode = S_IFDIR; break;
        case KOS_CONSOLE:
        case KOS_NULL: st->st_mode = S_IFCHR; break;
        case KOS_PIPE: st->st_mode = S_IFIFO; break;
        default: st->st_mode = S_IFREG; break;
    }
    // The owner may do anything; everyone else what the flags say, with
    // reading counting as running too.
    st->st_mode |= S_IRWXU;
    if (k->flags & 1) st->st_mode |= S_IRGRP | S_IXGRP | S_IROTH | S_IXOTH;
    if (k->flags & 2) st->st_mode |= S_IWGRP | S_IWOTH;
    st->st_nlink = 1;
    st->st_uid = k->owner;
    st->st_size = (off_t)k->size;
    st->st_blksize = 512;
    st->st_blocks = (blkcnt_t)((k->size + 511) / 512);
}

int stat(const char* path, struct stat* st) {
    kos_stat_t k;
    if (fs_stat(path, &k) < 0) return failed();
    to_stat(&k, st);
    return 0;
}

int lstat(const char* path, struct stat* st) {
    return stat(path, st);
}

int fstat(int fd, struct stat* st) {
    kos_stat_t k;
    if (fd_stat(fd, &k) < 0) return failed();
    to_stat(&k, st);
    return 0;
}

int mkdir(const char* path, mode_t mode) {
    (void)mode;
    kos_stat_t st;
    if (fs_stat(path, &st) == 0) {
        errno = EEXIST;
        return -1;
    }
    return fs_mkdir(path) < 0 ? failed() : 0;
}

int chmod(const char* path, mode_t mode) {
    return fs_chmod(path, (mode & (S_IROTH | S_IRGRP)) != 0, (mode & (S_IWOTH | S_IWGRP)) != 0) < 0 ? failed() : 0;
}

mode_t umask(mode_t mask) {
    (void)mask;
    return 022;
}

// ---- dirent.h -----------------------------------------------------------------

struct DIR {
    fs_entry_t* entries;
    int count;
    int next;
    struct dirent current;
};

DIR* opendir(const char* path) {
    kos_stat_t st;
    if (fs_stat(path, &st) < 0) {
        failed();
        return NULL;
    }
    if (st.kind != KOS_FOLDER) {
        errno = ENOTDIR;
        return NULL;
    }
    DIR* dir = alloc_z(sizeof(DIR));
    if (!dir) {
        errno = ENOMEM;
        return NULL;
    }
    dir->entries = fs_list(path, &dir->count);
    if (!dir->entries) {
        free(dir);
        failed();
        return NULL;
    }
    return dir;
}

struct dirent* readdir(DIR* dir) {
    if (dir->next >= dir->count) return NULL;
    fs_entry_t* e = &dir->entries[dir->next++];
    dir->current.d_ino = (ino_t)dir->next;
    dir->current.d_type = e->is_dir ? DT_DIR : DT_REG;
    strlcpy(dir->current.d_name, e->name, sizeof dir->current.d_name);
    return &dir->current;
}

void rewinddir(DIR* dir) {
    dir->next = 0;
}

int closedir(DIR* dir) {
    free(dir->entries);
    free(dir);
    return 0;
}

// ---- assert.h -----------------------------------------------------------------

void __assert_fail(const char* expr, const char* file, int line) {
    printf("%s:%d: assertion failed: %s\n", file, line, expr);
    exit(1);
}
