#ifndef C_STDIO_H
#define C_STDIO_H

// Formatted output. Conversions: %d %i %u %x %X %o %c %s %p %f %F %e %E
// %g %G %%, with the flags - + space 0 #, a width and a precision (either
// may be *), and the length modifiers hh h l ll z j t. Then FILE streams
// (fopen...), and sys.h has whole-file helpers (fs_read, fs_write...).

#include <stdarg.h>
#include <stddef.h>

// To stdout.
int printf(const char* format, ...) __attribute__((format(printf, 1, 2)));
int vprintf(const char* format, va_list args);

// To file descriptor fd (sys.h), e.g. dprintf(STDERR_FD, ...) for errors.
int dprintf(int fd, const char* format, ...) __attribute__((format(printf, 2, 3)));
int vdprintf(int fd, const char* format, va_list args);

// Format into buf (at most size bytes, always NUL-terminated if size > 0).
// Returns the length the whole result would have had.
int snprintf(char* buf, size_t size, const char* format, ...) __attribute__((format(printf, 3, 4)));
int vsnprintf(char* buf, size_t size, const char* format, va_list args);
// snprintf without a limit: buf must be big enough.
int sprintf(char* buf, const char* format, ...) __attribute__((format(printf, 2, 3)));

// Formats into fresh memory (free() it). Returns the length, or -1.
int asprintf(char** out, const char* format, ...) __attribute__((format(printf, 2, 3)));

// Writes s and a newline.
int puts(const char* s);
int putchar(int c);

// ---- FILE streams -------------------------------------------------------------
// Over the file descriptors of fcntl.h / unistd.h, so an open file is a
// copy in memory, written back by fclose(): see fcntl.h. stdin, stdout and
// stderr are descriptors 0-2 (the console, unless a shell gave the program
// a pipe or a file); fclose() leaves those open.

typedef struct FILE FILE;
extern FILE* stdin;
extern FILE* stdout;
extern FILE* stderr;

#define EOF (-1)
#define BUFSIZ 4096
#define FILENAME_MAX 4096
#define FOPEN_MAX 64
#define L_tmpnam 32
#define TMP_MAX 10000
// setvbuf() modes. Streams aren't buffered, so there's nothing to set.
#define _IOFBF 0
#define _IOLBF 1
#define _IONBF 2
#ifndef SEEK_SET
#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2
#endif

// mode: "r", "w" or "a", maybe with "+" (read and write) and "b" (ignored).
FILE* fopen(const char* path, const char* mode);
// A stream over an open file descriptor; fclose() closes that too.
FILE* fdopen(int fd, const char* mode);
// Writes the file back if it changed. 0, or EOF if that failed.
int fclose(FILE* f);
// Closes f's file and opens `path` in its place (with path NULL, keeps
// the file: KOS can't change a mode). Returns f, or NULL.
FILE* freopen(const char* path, const char* mode, FILE* f);
// A new empty file (in /tmp), removed when closed... (KOS can't remove an
// open file, so it's left there; tmpnam names are unique).
FILE* tmpfile(void);
// A name for a file nothing has yet: into buf (L_tmpnam bytes), or a
// static buffer if that's NULL.
char* tmpnam(char* buf);
int setvbuf(FILE* f, char* buf, int mode, size_t size);
void setbuf(FILE* f, char* buf);
// Nothing waits in a buffer, so there's nothing to do.
int fflush(FILE* f);

size_t fread(void* buf, size_t size, size_t n, FILE* f);
size_t fwrite(const void* buf, size_t size, size_t n, FILE* f);
int fgetc(FILE* f);
#define getc fgetc
int getchar(void);
// Puts c back, for the next read to return. One at a time.
int ungetc(int c, FILE* f);
// Reads a line (keeping its newline) into buf, at most size - 1 bytes.
char* fgets(char* buf, int size, FILE* f);
int fputc(int c, FILE* f);
#define putc fputc
int fputs(const char* s, FILE* f);
int fprintf(FILE* f, const char* format, ...) __attribute__((format(printf, 2, 3)));
int vfprintf(FILE* f, const char* format, va_list args);

int fseek(FILE* f, long offset, int whence);
long ftell(FILE* f);
void rewind(FILE* f);
// Whether a read hit the end, or anything failed; clearerr forgets both.
int feof(FILE* f);
int ferror(FILE* f);
void clearerr(FILE* f);
// Prints s, ": " and strerror(errno) to stderr.
void perror(const char* s);
int fileno(FILE* f);

// Deletes a file, or an empty folder.
int remove(const char* path);
// Moves or renames `from` to `to`, replacing a file (or an empty folder,
// for a folder) already there.
int rename(const char* from, const char* to);

#endif //C_STDIO_H
