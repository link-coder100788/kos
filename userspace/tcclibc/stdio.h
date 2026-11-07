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
// Nothing waits in a buffer, so there's nothing to do.
int fflush(FILE* f);

size_t fread(void* buf, size_t size, size_t n, FILE* f);
size_t fwrite(const void* buf, size_t size, size_t n, FILE* f);
int fgetc(FILE* f);
#define getc fgetc
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
// Whether a read hit the end, or anything failed.
int feof(FILE* f);
int ferror(FILE* f);
int fileno(FILE* f);

// Deletes a file.
int remove(const char* path);

#endif //C_STDIO_H
