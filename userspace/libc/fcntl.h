#ifndef C_FCNTL_H
#define C_FCNTL_H

// open(). KOS reads and writes whole files, so an open file is a copy in
// memory: opening reads it all, and it's written back once its last
// descriptor is closed, or the program ends.

#define O_RDONLY 0
#define O_WRONLY 1
#define O_RDWR 2
#define O_ACCMODE 3
#define O_CREAT 0100
#define O_EXCL 0200
#define O_TRUNC 01000
#define O_APPEND 02000
// No text/binary difference on KOS.
#define O_BINARY 0

// Opens `path`, returning a file descriptor or -1 (and errno). A mode
// argument after O_CREAT is accepted and ignored: KOS files have none.
int open(const char* path, int flags, ...);

#endif //C_FCNTL_H
