#ifndef C_SYS_TYPES_H
#define C_SYS_TYPES_H

// POSIX's types for sizes, offsets and ids.

#include <stddef.h>

#ifndef C_TIME_T
#define C_TIME_T
typedef long time_t;
#endif

typedef long ssize_t;
typedef long off_t;
typedef unsigned int mode_t;
typedef unsigned long dev_t;
typedef unsigned long ino_t;
typedef unsigned long nlink_t;
typedef long blksize_t;
typedef long blkcnt_t;
typedef int pid_t;
typedef unsigned int uid_t;
typedef unsigned int gid_t;

#endif //C_SYS_TYPES_H
