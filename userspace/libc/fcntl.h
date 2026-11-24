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
// Accepted and ignored: KOS has no controlling terminals, non-blocking
// files, or exec to close descriptors on.
#define O_NOCTTY 0400
#define O_NONBLOCK 04000
#define O_NDELAY O_NONBLOCK
#define O_SYNC 04010000
#define O_DIRECTORY 0200000
#define O_NOFOLLOW 0400000
#define O_CLOEXEC 02000000
// No text/binary difference on KOS.
#define O_BINARY 0

// Opens `path`, returning a file descriptor or -1 (and errno). A mode
// argument after O_CREAT is accepted and ignored: KOS files have none.
int open(const char* path, int flags, ...);
// open(path, O_WRONLY | O_CREAT | O_TRUNC).
int creat(const char* path, int mode);

// fcntl() commands. F_DUPFD gives the lowest free descriptor >= its
// argument; the flag commands are accepted, but there's nothing to set.
#define F_DUPFD 0
#define F_GETFD 1
#define F_SETFD 2
#define F_GETFL 3
#define F_SETFL 4
#define F_DUPFD_CLOEXEC 1030
#define FD_CLOEXEC 1

int fcntl(int fd, int cmd, ...);

#endif //C_FCNTL_H
