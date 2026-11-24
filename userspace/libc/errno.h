#ifndef C_ERRNO_H
#define C_ERRNO_H

// Why the last failed call failed (set by the file functions in stdio.h,
// fcntl.h and unistd.h). strerror(errno) (string.h) says it in words.

extern int errno;

// Linux's numbers.
#define EPERM 1         // only root (or the owner) may
#define ENOENT 2        // no such file
#define ESRCH 3         // no such process
#define EINTR 4         // interrupted
#define EIO 5           // the disk said no
#define ENXIO 6         // no such device
#define E2BIG 7         // too many arguments
#define ENOEXEC 8       // not a program
#define EBADF 9         // not an open file
#define ECHILD 10       // no such child process
#define EAGAIN 11       // nothing yet: try again
#define EWOULDBLOCK EAGAIN
#define ENOMEM 12       // out of memory
#define EACCES 13       // not allowed (e.g. writing a file opened to read)
#define EFAULT 14       // a bad pointer
#define EBUSY 16        // in use
#define EEXIST 17       // already exists
#define EXDEV 18        // across devices
#define ENODEV 19       // no such device
#define ENOTDIR 20      // not a folder
#define EISDIR 21       // is a folder
#define EINVAL 22       // a bad argument
#define ENFILE 23       // too many open files in the system
#define EMFILE 24       // too many open files
#define ENOTTY 25       // not a terminal
#define EFBIG 27        // file too big
#define ENOSPC 28       // the disk is full
#define ESPIPE 29       // can't seek (a pipe or the console)
#define EROFS 30        // read-only
#define EMLINK 31       // too many links
#define EPIPE 32        // nothing reads from the pipe any more
#define EDOM 33         // outside a maths function's domain
#define ERANGE 34       // a number too big to hold
#define EDEADLK 35      // would deadlock
#define ENAMETOOLONG 36 // a name too long
#define ENOSYS 38       // KOS can't do that
#define ENOTEMPTY 39    // the folder isn't empty
#define ELOOP 40        // too many links to follow
#define ENOTSUP 95      // not supported
#define EOPNOTSUPP ENOTSUP
#define EADDRINUSE 98   // the address is in use
#define ECONNRESET 104  // the connection was reset
#define ETIMEDOUT 110   // timed out
#define ECONNREFUSED 111 // the connection was refused

#endif //C_ERRNO_H
