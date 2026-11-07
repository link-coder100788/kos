#ifndef C_SYS_H
#define C_SYS_H

// The KOS C library: thin wrappers around the kernel's syscalls.
//
// Programs are freestanding (-ffreestanding -nostdlib), start at
// `void _start(void)`, and must end with exit(status). Build them by dropping a
// .c file in userspace/prog/ and running `make disk`; run them with
// `run <name>.elf.b64` in the shell (which waits for them), `bg` (in the
// background, without the console) or from the Programs window.
//
// System commands (grep, ...) go in userspace/cmd/ instead: they end up in
// /bin on the disk, out of the Programs window, and the shell runs one
// when you type its name: `grep main notes.txt`.
//
// This header is the kernel's side of things. The rest of the library is
// ordinary C on top of it: string.h, ctype.h, stdlib.h (malloc, strtol,
// qsort, rand...), stdio.h (printf, snprintf), math.h, time.h, net.h
// (HTTP and TCP), crypto.h (hashes and encryption) and canvas.h (pixel
// windows).
//
// Calls that can fail return -1 (or NULL) and keep the reason, which
// last_error() returns.
//
// Syscalls: `svc #n`, arguments in x0-x4, a sub-command in x16 for 4-7,
// 12-13 and 15-17, the result in x0 (-1 for failure). The README's "Syscalls"
// table has every sub-command and its arguments.
//
//   svc  what           libc
//    0   exit           exit
//    1   write a byte   write_byte (to stdout)
//    2   write bytes    write_bytes, write, printf (to stdout)
//    3   read a line    readln (from stdin)
//    4   memory         alloc*, free, mem_free, mem_total, mem_size
//    5   files          fs_write, fs_read, fs_list, fs_remove, fs_mkdir, fs_cwd
//    6   windows        ui_*, canvas_*
//    7   time           sleep_ms, sleep_s, uptime_ms, unix_time, utc_offset
//    8   dump           dump
//    9   yield          yield
//   10   arguments      args
//   11   random         random_bytes
//   12   network        http_*, ping, net_address, tcp_*, udp_* (net.h)
//   13   processes      spawn, proc_*, last_error
//   14   system info    uname
//   15   crypto         sha256, sha512, hmac_sha256, aes_gcm_* (crypto.h)
//   16   sound          snd, beep, play, play_file, snd_stop, snd_busy
//   17   descriptors    fd_open, fd_read, fd_write, fd_close, fd_seek, fd_dup,
//                       fd_dup2, fd_pipe, fd_isatty
//
// Each program runs in user mode in its own address space: it sees only
// its own code, data, heap and stack (up to 8 MiB, growing as it's used), so a
// bug can't hurt the kernel
// or other programs; it just gets the program killed.

#include <stdint.h>
#include <stddef.h>

// ---- process -------------------------------------------------------------

// Ends the program with an exit status: 0 for success, anything else
// (up to 255) for failure, as a shell's && and || see it. Its open files
// are closed (and written back) first. Returning from _start is exit(0).
__attribute__((noreturn)) void exit(int status);

// Gives the rest of this time slice to other tasks.
void yield(void);

// Sleeps for at least `ms` milliseconds (rounded up to 10 ms).
void sleep_ms(uint64_t ms);

// Sleeps for at least `s` seconds.
void sleep_s(uint64_t s);

// Prints the CPU registers to the console.
void dump(void);

// Copies up to max_len bytes of the program's command-line arguments
// (everything typed after its name, e.g. "-n main notes.txt") into buf,
// without a terminating NUL. Returns their full length.
long args(char* buf, size_t max_len);

// Reads the arguments and splits them at spaces: fills argv with up to
// max words (pointing into a buffer of its own) and returns how many.
int get_argv(char** argv, int max);

// Why the last call that returned -1 or NULL failed (files, network,
// processes, random numbers), as a NUL-terminated string. It stays the
// same until another call fails.
const char* last_error(void);

// ---- other programs --------------------------------------------------------
// Programs are identified by their task id, as `ps` and the Task Manager
// show it. A program started by another never gets keyboard input from
// the console, but can be given a pipe or file to read (spawn_io).

// Starts the program at `path` (an .elf, .elf.b64 or .kbin) with `args` as
// its arguments (may be NULL). It gets this program's stdin, stdout and
// stderr. Returns its id, or -1.
long spawn(const char* path, const char* args);

// spawn, but the program's stdin, stdout and stderr are this program's
// descriptors fds[0], fds[1] and fds[2] (-1 for the same as ours): e.g.
// { -1, pipe_fds[1], -1 } to read what it prints from pipe_fds[0]. Close
// our copy of the write end afterwards, or reading never ends.
long spawn_io(const char* path, const char* args, const int fds[3]);

// Waits until program `id` has ended, and returns its exit status: what it
// passed to exit(), or 130 if Ctrl+C ended it, 137 if it was killed, 139
// if it crashed. -1 if there's no such program. Once this returns, what it
// wrote to files is on the disk.
long proc_wait(long id);

// spawn, then proc_wait: the exit status, or -1 if it couldn't start.
long run_program(const char* path, const char* args);

// Ends program `id`. Returns 0, or -1.
long proc_kill(long id);

// Whether program `id` is still running (1 or 0).
int proc_running(long id);

// This program's own id.
long proc_self(void);

// ---- time ------------------------------------------------------------------
// See time.h for dates.

// Milliseconds since KOS started: for timing and animation.
uint64_t uptime_ms(void);

// Seconds since 1970-01-01 00:00 UTC, or -1 if there's no clock.
long unix_time(void);

// How many seconds the local time zone is ahead of UTC.
long utc_offset(void);

// ---- sound -------------------------------------------------------------------
// One voice, like a PC speaker: each call replaces whatever was playing,
// and a program's sound stops when it ends. These return 0, or -1 if
// there's no sound device (or bad notes; last_error() says what).

// Plays a steady square-wave tone of `hz` until something else plays;
// snd(0) is silence.
long snd(uint32_t hz);

// A short beep; returns once it's over.
long beep(void);

// Plays a tune and returns once it's over (or something else replaced it),
// e.g. play("T140 O4 C D E8 F#8 G2"):
// notes C-B (# sharp, - flat) with a length (4 = quarter, 8 = eighth, a `.`
// for dotted), R for a rest, On/</> for the octave, Tn tempo, Ln default
// length. The README's Sound section has the details.
long play(const char* notes);

// Plays a WAV file (uncompressed PCM, up to 3 minutes) and returns once
// it's over (or something else replaced it).
long play_file(const char* path);

// Stops whatever is playing.
long snd_stop(void);

// 1 while something is playing (e.g. a song from play()), else 0.
int snd_busy(void);

// ---- random numbers ------------------------------------------------------------

// Fills buf with len random bytes from the kernel's generator (ChaCha20,
// seeded by the CPU's hardware one). Returns 0, or -1 if the CPU has no
// hardware generator. (stdlib.h's rand() is quicker for
// games; this is for keys and seeds.)
long random_bytes(void* buf, size_t len);

// 64 random bits (0 if there's no generator).
uint64_t random_u64(void);

// ---- console ---------------------------------------------------------------
// These write to stdout, which is the serial console and the Terminal
// unless whoever started the program gave it a pipe or a file (ksh does
// for `|` and `>`). For programs started with `bg` it's thrown away.

void write_byte(const uint8_t* byte);

void write_bytes(const uint8_t* bytes, size_t len);

// Writes a NUL-terminated string.
void write(const char* str);

// Writes a NUL-terminated string and a newline.
void writeln(const char* str);

// Reads the next line from stdin and copies up to max_len bytes of it
// into buf, without the newline and without a terminating NUL. Returns the
// number of bytes copied, or -1 at the end of the input. On the console
// (the usual stdin) that waits for a line to be typed, and only programs
// started with `run` get input; for any other, this returns 0 straight
// away. A pipe or file gives -1 once it has no more lines.
long readln(char* buf, size_t max_len);

// ---- memory ----------------------------------------------------------------
// The heap is the program's own memory, after its image. It grows as needed,
// as far as free RAM allows, and freeing gives whole pages back to the
// system. Memory is 16-byte aligned. Whatever a program hasn't freed goes
// away with it when it exits.

void* alloc(size_t size);

void* alloc_n(size_t size, int n);

// Like alloc / alloc_n, but zeroed.
void* alloc_z(size_t size);

void* alloc_n_z(size_t size, int n);

// Gives back memory from any of the alloc functions. free(NULL) does nothing.
void free(void* ptr);

// Free and total bytes of this program's heap, counting the free RAM it
// could still grow into.
size_t mem_free(void);

size_t mem_total(void);

// How many bytes the allocation at ptr has room for (at least what was
// asked for), or 0 if ptr isn't one.
size_t mem_size(void* ptr);

// ---- files -------------------------------------------------------------------
// Paths are relative to the shell's current directory; "/" starts at the
// top of the disk.

// Creates or replaces a file. Returns 0, or -1 on error.
long fs_write(const char* path, const void* data, size_t len);

// Reads up to max_len bytes of a file into buf. Returns the file's full
// size (which may be more than max_len), or -1 on error.
long fs_read(const char* path, void* buf, size_t max_len);

// A file's size, or -1 if it isn't there (or is a folder).
long fs_size(const char* path);

// Reads a whole file into memory, with a NUL after it. Returns it (free()
// it when done) and its size in *len (len may be NULL), or NULL.
char* fs_read_all(const char* path, size_t* len);

// Adds data to the end of a file, creating it if need be. Returns 0 or -1.
long fs_append(const char* path, const void* data, size_t len);

// Deletes a file or an empty folder. Returns 0 or -1.
long fs_remove(const char* path);

// Makes a folder. Returns 0 or -1.
long fs_mkdir(const char* path);

// Copies the shell's current directory into buf, NUL-terminated. Returns
// its length, or -1 if it doesn't fit.
long fs_cwd(char* buf, size_t max_len);

typedef struct {
    char name[256];
    int is_dir;
    uint32_t size;   // 0 for folders
} fs_entry_t;

// Lists a folder (not "." or ".."). Returns an array of *count entries
// (free() it when done), or NULL on error.
fs_entry_t* fs_list(const char* path, int* count);

// ---- file descriptors ----------------------------------------------------------
// Numbered handles on files, pipes and the console, like Unix's. 0, 1 and
// 2 are stdin, stdout and stderr; a program gets them from whoever started
// it. KOS reads and writes whole files, so an open file is a copy in
// memory: it's written back to the disk when its last descriptor is
// closed, or when the program ends. Calls return -1 on error
// (last_error() says why).

#define STDIN_FD 0
#define STDOUT_FD 1
#define STDERR_FD 2

// fd_open flags (Linux's values; combine one of the first three with |).
#define FD_READ 0         // O_RDONLY
#define FD_WRITE 1        // O_WRONLY
#define FD_READ_WRITE 2   // O_RDWR
#define FD_CREATE 0100    // O_CREAT: make it if it isn't there
#define FD_EXCLUSIVE 0200 // O_EXCL: with FD_CREATE, fail if it is
#define FD_TRUNCATE 01000 // O_TRUNC: empty it first
#define FD_APPEND 02000   // O_APPEND: every write goes at the end

// Opens `path` (relative to the shell's directory). Returns a descriptor.
long fd_open(const char* path, int flags);

// Reads up to max bytes. Returns how many (0 at the end of the input). A
// pipe waits for something to read; the console waits for a line.
long fd_read(long fd, void* buf, size_t max);

// fd_read, but returns -2 at once if a pipe has nothing to read yet.
long fd_read_now(long fd, void* buf, size_t max);

// Writes len bytes; returns how many. A full pipe waits for room.
long fd_write(long fd, const void* data, size_t len);

// fd_write, but writes only what fits in a pipe now: -2 if nothing does.
long fd_write_now(long fd, const void* data, size_t len);

// Closes a descriptor. Returns 0, or -1 if writing the file back failed.
long fd_close(long fd);

// Moves a file's position (whence: 0 from the start, 1 from here, 2 from
// the end). Returns the new position.
long fd_seek(long fd, long offset, int whence);

// Another descriptor for the same thing (and the same position).
long fd_dup(long fd);

// Makes descriptor `to` the same as `fd`, closing what `to` was. Returns to.
long fd_dup2(long fd, long to);

// A pipe: fds[1] writes, fds[0] reads it. Returns 0.
long fd_pipe(int fds[2]);

// 1 if fd is the console, else 0.
int fd_isatty(long fd);

// Reads everything from fd until the end of its input (e.g. stdin, when
// it's a pipe), with a NUL after it. Returns it (free() it when done) and
// its size in *len (len may be NULL), or NULL.
char* fd_read_all(long fd, size_t* len);

// ---- windows -----------------------------------------------------------------

// One element of a window. Build them with ui_label / ui_button.
// (For labels, h is 0 and w packs the color and scale.)
typedef struct {
    size_t x, y, w, h;
    const char* text;
} ui_element_t;

// Text at (x, y) in the given color; scale 1 is 8 pixels tall.
static inline ui_element_t ui_label(size_t x, size_t y, const char* text, uint8_t r, uint8_t g, uint8_t b, uint8_t scale) {
    ui_element_t e = { x, y, (size_t)r | ((size_t)g << 8) | ((size_t)b << 16) | ((size_t)scale << 24), 0, text };
    return e;
}

static inline ui_element_t ui_button(size_t x, size_t y, size_t w, size_t h, const char* text) {
    ui_element_t e = { x, y, w, h, text };
    return e;
}

// Opens a window on the desktop. Coordinates are relative to its content
// area; the window is sized to fit. Returns the window's id.
long ui_create_window(const char* title, const ui_element_t* elements, size_t count);

void ui_close_window(long id);

// Returns 0 if nothing happened, n + 1 if element n (a button) was
// clicked, or -1 if the user closed the window.
long ui_poll_event(long id);

// Opens the kernel's built-in test window.
long ui_test_window(void);

#define UTS_LEN 65

typedef struct {
    char sysname[UTS_LEN];
    char nodename[UTS_LEN];
    char release[UTS_LEN];
    char version[UTS_LEN];
    char machine[UTS_LEN];
} utsname_t;

// Fills *buf with the system's name, node, release, version and machine.
// Returns 0, or -1 for a bad buffer.
long uname(utsname_t* buf);

#endif //C_SYS_H
