#ifndef C_SYS_H
#define C_SYS_H

// The KOS C library: thin wrappers around the kernel's syscalls.
//
// Programs are freestanding (-ffreestanding -nostdlib), start at
// `void _start(void)`, and must end with exit(). Build them by dropping a
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
// 12-13 and 15-16, the result in x0 (-1 for failure). The README's "Syscalls"
// table has every sub-command and its arguments.
//
//   svc  what           libc
//    0   exit           exit
//    1   write a byte   write_byte
//    2   write bytes    write_bytes, write, printf
//    3   read a line    readln
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
//   16   sound          snd, beep, play, snd_stop, snd_busy
//
// Each program runs in user mode in its own address space: it sees only
// its own code, data, heap and stack (up to 8 MiB, growing as it's used), so a
// bug can't hurt the kernel
// or other programs; it just gets the program killed.

#include <stdint.h>
#include <stddef.h>

// ---- process -------------------------------------------------------------

// Ends the program.
__attribute__((noreturn)) void exit(void);

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
// show it. A program started by another prints where its parent does, but
// never gets keyboard input.

// Starts the program at `path` (an .elf, .elf.b64 or .kbin) with `args` as
// its arguments (may be NULL). Returns its id, or -1.
long spawn(const char* path, const char* args);

// Waits until program `id` has ended. Returns 0.
long proc_wait(long id);

// spawn, then proc_wait. Returns 0, or -1 if it couldn't start.
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
// Output goes to the serial console and the Terminal, except for programs
// started with `bg`, whose output is thrown away.

void write_byte(const uint8_t* byte);

void write_bytes(const uint8_t* bytes, size_t len);

// Writes a NUL-terminated string.
void write(const char* str);

// Writes a NUL-terminated string and a newline.
void writeln(const char* str);

// Waits for the next line typed on the console and copies up to max_len
// bytes of it into buf, without the newline and without a terminating NUL.
// Returns the number of bytes copied. Only programs started with `run` get
// input; for any other, this returns 0 straight away.
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
