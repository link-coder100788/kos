#ifndef C_ERRNO_H
#define C_ERRNO_H

// Why the last failed call failed (set by the file functions in stdio.h,
// fcntl.h and unistd.h). strerror(errno) (string.h) says it in words.

extern int errno;

#define ENOENT 2   // no such file
#define EIO 5      // the disk said no
#define EBADF 9    // not an open file
#define ENOMEM 12  // out of memory
#define EACCES 13  // not allowed (e.g. writing a file opened to read)
#define EEXIST 17  // already exists
#define EINVAL 22  // a bad argument
#define EMFILE 24  // too many open files
#define ERANGE 34  // a number too big to hold
#define ENOSYS 38  // KOS can't do that

#endif //C_ERRNO_H
