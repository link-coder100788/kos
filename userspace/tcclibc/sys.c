#include "sys.h"
#include "syscall.h"

__attribute__((noreturn)) void exit(int status) {
    KOS_SVC(0x0, 0, status, 0, 0, 0, 0);
    __builtin_unreachable();
}

void write_byte(const uint8_t* byte) {
    KOS_SVC(0x1, 0, *byte, 0, 0, 0, 0);
}

void write_bytes(const uint8_t* bytes, size_t len) {
    KOS_SVC(0x2, 0, bytes, len, 0, 0, 0);
}

void write(const char* str) {
    size_t len = 0;
    while (str[len] != '\0') len++;
    write_bytes((const uint8_t*)str, len);
}

void writeln(const char* str) {
    write(str);
    write("\n");
}

void yield(void) {
    KOS_SVC(0x9, 0, 0, 0, 0, 0, 0);
}

long args(char* buf, size_t max_len) {
    return (long)KOS_SVC(0xa, 0, buf, max_len, 0, 0, 0);
}

int get_argv(char** argv, int max) {
    long len = args(NULL, 0);
    if (len <= 0) return 0;
    char* buf = alloc(len + 1);
    if (!buf) return 0;
    args(buf, len);
    buf[len] = '\0';
    int n = 0;
    char* p = buf;
    while (*p && n < max) {
        while (*p == ' ') *p++ = '\0';
        if (!*p) break;
        argv[n++] = p;
        while (*p && *p != ' ') p++;
    }
    return n;
}

long readln(char* buf, size_t max_len) {
    return (long)KOS_SVC(0x3, 0, buf, max_len, 0, 0, 0);
}

// Syscall 4 is memory; the sub-command picks what it does.
void* alloc(size_t size) {
    return (void*)KOS_SVC(0x4, 0x0, size, 0, 0, 0, 0);
}

void* alloc_n(size_t size, int n) {
    return (void*)KOS_SVC(0x4, 0x1, size, n, 0, 0, 0);
}

void* alloc_z(size_t size) {
    return (void*)KOS_SVC(0x4, 0x2, size, 0, 0, 0, 0);
}

void* alloc_n_z(size_t size, int n) {
    return (void*)KOS_SVC(0x4, 0x3, size, n, 0, 0, 0);
}

void free(void* ptr) {
    KOS_SVC(0x4, 0x6, ptr, 0, 0, 0, 0);
}

size_t mem_free() {
    return (size_t)KOS_SVC(0x4, 0x4, 0, 0, 0, 0, 0);
}

size_t mem_total() {
    return (size_t)KOS_SVC(0x4, 0x5, 0, 0, 0, 0, 0);
}

// Calls a syscall that takes a sub-command in x16 and up to four
// arguments, and returns x0.
#define SVC(n, sub, a0, a1, a2, a3) KOS_SVC(n, sub, a0, a1, a2, a3, 0)

static size_t c_strlen(const char* s) {
    size_t len = 0;
    while (s[len] != '\0') len++;
    return len;
}

void sleep_ms(uint64_t ms) {
    SVC(0x7, 0, ms, 0, 0, 0);
}

void sleep_s(uint64_t s) {
    SVC(0x7, 1, s, 0, 0, 0);
}

void dump(void) {
    SVC(0x8, 0, 0, 0, 0, 0);
}

long fs_write(const char* path, const void* data, size_t len) {
    return (long)SVC(0x5, 0, path, c_strlen(path), data, len);
}

long fs_read(const char* path, void* buf, size_t max_len) {
    return (long)SVC(0x5, 1, path, c_strlen(path), buf, max_len);
}

long ui_create_window(const char* title, const ui_element_t* elements, size_t count) {
    return (long)SVC(0x6, 0, title, elements, count, 0);
}

void ui_close_window(long id) {
    SVC(0x6, 1, id, 0, 0, 0);
}

long ui_test_window(void) {
    return (long)SVC(0x6, 2, 0, 0, 0, 0);
}

long ui_poll_event(long id) {
    return (long)SVC(0x6, 3, id, 0, 0, 0);
}

size_t mem_size(void* ptr) {
    return (size_t)SVC(0x4, 7, ptr, 0, 0, 0);
}

// ---- errors ----------------------------------------------------------------------

const char* last_error(void) {
    static char why[256];
    long n = (long)SVC(0xd, 5, why, sizeof(why) - 1, 0, 0);
    if (n < 0) n = 0;
    if (n > (long)sizeof(why) - 1) n = sizeof(why) - 1;
    why[n] = '\0';
    return why;
}

// ---- other programs ----------------------------------------------------------------

long spawn(const char* path, const char* args) {
    return (long)SVC(0xd, 0, path, args, 0, 0);
}

long spawn_io(const char* path, const char* args, const int fds[3]) {
    return (long)SVC(0xd, 6, path, args, fds, 0);
}

long proc_wait(long id) {
    return (long)SVC(0xd, 1, id, 0, 0, 0);
}

long run_program(const char* path, const char* args) {
    long id = spawn(path, args);
    if (id < 0) return -1;
    return proc_wait(id);
}

long proc_kill(long id) {
    return (long)SVC(0xd, 2, id, 0, 0, 0);
}

int proc_running(long id) {
    return (int)SVC(0xd, 4, id, 0, 0, 0);
}

long proc_self(void) {
    return (long)SVC(0xd, 3, 0, 0, 0, 0);
}

// ---- file descriptors --------------------------------------------------------------

long fd_open(const char* path, int flags) {
    return (long)SVC(0x11, 0, path, c_strlen(path), flags, 0);
}

long fd_read(long fd, void* buf, size_t max) {
    return (long)SVC(0x11, 1, fd, buf, max, 0);
}

long fd_read_now(long fd, void* buf, size_t max) {
    return (long)SVC(0x11, 1, fd, buf, max, 1);
}

long fd_write(long fd, const void* data, size_t len) {
    return (long)SVC(0x11, 2, fd, data, len, 0);
}

long fd_write_now(long fd, const void* data, size_t len) {
    return (long)SVC(0x11, 2, fd, data, len, 1);
}

long fd_close(long fd) {
    return (long)SVC(0x11, 3, fd, 0, 0, 0);
}

long fd_seek(long fd, long offset, int whence) {
    return (long)SVC(0x11, 4, fd, offset, whence, 0);
}

long fd_dup2(long fd, long to) {
    return (long)SVC(0x11, 5, fd, to, 0, 0);
}

long fd_dup(long fd) {
    return (long)SVC(0x11, 6, fd, 0, 0, 0);
}

long fd_pipe(int fds[2]) {
    return (long)SVC(0x11, 7, fds, 0, 0, 0);
}

int fd_isatty(long fd) {
    return (int)SVC(0x11, 8, fd, 0, 0, 0);
}

char* fd_read_all(long fd, size_t* len) {
    size_t size = 0, cap = 4096;
    char* data = alloc(cap + 1);
    if (!data) return NULL;
    for (;;) {
        if (size == cap) {
            char* bigger = alloc(cap * 2 + 1);
            if (!bigger) {
                free(data);
                return NULL;
            }
            for (size_t i = 0; i < size; i++) bigger[i] = data[i];
            free(data);
            data = bigger;
            cap *= 2;
        }
        long n = fd_read(fd, data + size, cap - size);
        if (n < 0) {
            free(data);
            return NULL;
        }
        if (n == 0) break;
        size += n;
    }
    data[size] = '\0';
    if (len) *len = size;
    return data;
}

// ---- time --------------------------------------------------------------------------

uint64_t uptime_ms(void) {
    return SVC(0x7, 2, 0, 0, 0, 0);
}

long unix_time(void) {
    return (long)SVC(0x7, 3, 0, 0, 0, 0);
}

long utc_offset(void) {
    return (long)SVC(0x7, 4, 0, 0, 0, 0);
}

// ---- sound -------------------------------------------------------------------------

long snd(uint32_t hz) {
    return (long)SVC(0x10, 0, hz, 0, 0, 0);
}

long beep(void) {
    return (long)SVC(0x10, 1, 0, 0, 0, 0);
}

long play(const char* notes) {
    return (long)SVC(0x10, 2, notes, c_strlen(notes), 0, 0);
}

long snd_stop(void) {
    return (long)SVC(0x10, 3, 0, 0, 0, 0);
}

int snd_busy(void) {
    return (int)SVC(0x10, 4, 0, 0, 0, 0);
}

long play_file(const char* path) {
    return (long)SVC(0x10, 5, path, c_strlen(path), 0, 0);
}

// ---- random numbers ----------------------------------------------------------------

long random_bytes(void* buf, size_t len) {
    return (long)SVC(0xb, 0, buf, len, 0, 0);
}

uint64_t random_u64(void) {
    uint64_t v = 0;
    random_bytes(&v, sizeof(v));
    return v;
}

// ---- files -------------------------------------------------------------------------

long fs_size(const char* path) {
    return fs_read(path, NULL, 0);
}

char* fs_read_all(const char* path, size_t* len) {
    long size = fs_size(path);
    if (size < 0) return NULL;
    char* data = alloc(size + 1);
    if (!data) return NULL;
    long got = fs_read(path, data, size);
    if (got < 0) {
        free(data);
        return NULL;
    }
    // It may have shrunk in between.
    if (got < size) size = got;
    data[size] = '\0';
    if (len) *len = size;
    return data;
}

long fs_append(const char* path, const void* data, size_t len) {
    size_t old_len = 0;
    char* old = fs_read_all(path, &old_len);
    char* both = alloc(old_len + len + 1);
    if (!both) {
        free(old);
        return -1;
    }
    for (size_t i = 0; i < old_len; i++) both[i] = old[i];
    for (size_t i = 0; i < len; i++) both[old_len + i] = ((const char*)data)[i];
    long result = fs_write(path, both, old_len + len);
    free(old);
    free(both);
    return result;
}

long fs_remove(const char* path) {
    return (long)SVC(0x5, 3, path, c_strlen(path), 0, 0);
}

long fs_mkdir(const char* path) {
    return (long)SVC(0x5, 4, path, c_strlen(path), 0, 0);
}

long fs_cwd(char* buf, size_t max_len) {
    long n = (long)SVC(0x5, 5, buf, max_len, 0, 0);
    if (n < 0 || (size_t)n >= max_len) return -1;
    buf[n] = '\0';
    return n;
}

fs_entry_t* fs_list(const char* path, int* count) {
    *count = 0;
    long len = (long)SVC(0x5, 2, path, c_strlen(path), NULL, 0);
    if (len < 0) return NULL;
    char* text = alloc(len + 1);
    if (!text) return NULL;
    len = (long)SVC(0x5, 2, path, c_strlen(path), text, len);
    if (len < 0) {
        free(text);
        return NULL;
    }
    text[len] = '\0';
    int lines = 0;
    for (long i = 0; i < len; i++) if (text[i] == '\n') lines++;
    fs_entry_t* entries = alloc_z(sizeof(fs_entry_t) * (lines ? lines : 1));
    if (!entries) {
        free(text);
        return NULL;
    }
    // Each line is "d|f <tab> size <tab> name".
    int n = 0;
    char* p = text;
    while (*p && n < lines) {
        fs_entry_t* e = &entries[n];
        e->is_dir = *p == 'd';
        while (*p && *p != '\t') p++;
        if (*p) p++;
        while (*p >= '0' && *p <= '9') e->size = e->size * 10 + (*p++ - '0');
        if (*p) p++;
        size_t k = 0;
        while (*p && *p != '\n') {
            if (k < sizeof(e->name) - 1) e->name[k++] = *p;
            p++;
        }
        if (*p) p++;
        n++;
    }
    free(text);
    *count = n;
    return entries;
}

long uname(utsname_t* buf) {
    return (long)KOS_SVC(0xe, 0, buf, 0, 0, 0, 0);
}
