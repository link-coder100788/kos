#ifndef C_UNISTD_H
#define C_UNISTD_H

// File descriptors (see fcntl.h for open). 0, 1 and 2 are stdin, stdout and
// stderr; stdout and stderr both go to the console, and stdin reads nothing.
// (POSIX write() isn't here: sys.h's write(str) has the name.)

#include <stddef.h>

typedef long ssize_t;
typedef long off_t;

#ifndef SEEK_SET
#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2
#endif

ssize_t read(int fd, void* buf, size_t n);
off_t lseek(int fd, off_t offset, int whence);
int close(int fd);
int unlink(const char* path);
// The shell's current directory, or NULL if it doesn't fit in `size`.
char* getcwd(char* buf, size_t size);

#endif //C_UNISTD_H
