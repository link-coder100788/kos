#ifndef C_UNISTD_H
#define C_UNISTD_H

// File descriptors (see fcntl.h for open). 0, 1 and 2 are stdin, stdout and
// stderr: the console, or the pipes and files a shell gave the program.
// (POSIX write() isn't here: sys.h's write(str) has the name; use
// fd_write.)

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
int dup(int fd);
int dup2(int fd, int to);
// fds[1] writes into the pipe, fds[0] reads from it.
int pipe(int fds[2]);
int isatty(int fd);
int unlink(const char* path);
// This program's current folder, or NULL if it doesn't fit in `size`.
char* getcwd(char* buf, size_t size);
// Changes this program's current folder. 0, or -1 with errno set.
int chdir(const char* path);

#endif //C_UNISTD_H
