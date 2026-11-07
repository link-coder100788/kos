#ifndef C_STDIO_H
#define C_STDIO_H

// Formatted output. Conversions: %d %i %u %x %X %o %c %s %p %f %F %e %E
// %g %G %%, with the flags - + space 0 #, a width and a precision (either
// may be *), and the length modifiers hh h l ll z j t. Files are in
// sys.h (fs_read, fs_write...).

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

#endif //C_STDIO_H
