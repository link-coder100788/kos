#ifndef C_SYS_STAT_H
#define C_SYS_STAT_H

// What a file is: stat(), fstat(), and mkdir() and chmod() with modes.
//
// KOS's permissions are simpler than Unix's (see `chmod` in the README): an
// owner, who may always read and write, and whether everyone else may
// read and write. There are no groups and no execute bit (anything that
// can be read can be run), so st_mode has the owner's rwx, and r-x / -w-
// for group and others alike from the two flags. KOS doesn't keep file
// times yet, so the st_*time fields are 0.

#include "types.h"

struct stat {
    dev_t st_dev;
    ino_t st_ino;
    mode_t st_mode;
    nlink_t st_nlink;
    uid_t st_uid;
    gid_t st_gid;
    dev_t st_rdev;
    off_t st_size;
    blksize_t st_blksize;
    blkcnt_t st_blocks;
    time_t st_atime;
    time_t st_mtime;
    time_t st_ctime;
};

#define S_IFMT 0170000
#define S_IFIFO 0010000
#define S_IFCHR 0020000
#define S_IFDIR 0040000
#define S_IFBLK 0060000
#define S_IFREG 0100000
#define S_IFLNK 0120000
#define S_IFSOCK 0140000

#define S_ISFIFO(m) (((m) & S_IFMT) == S_IFIFO)
#define S_ISCHR(m) (((m) & S_IFMT) == S_IFCHR)
#define S_ISDIR(m) (((m) & S_IFMT) == S_IFDIR)
#define S_ISBLK(m) (((m) & S_IFMT) == S_IFBLK)
#define S_ISREG(m) (((m) & S_IFMT) == S_IFREG)
#define S_ISLNK(m) (((m) & S_IFMT) == S_IFLNK)
#define S_ISSOCK(m) (((m) & S_IFMT) == S_IFSOCK)

#define S_IRWXU 0700
#define S_IRUSR 0400
#define S_IWUSR 0200
#define S_IXUSR 0100
#define S_IRWXG 0070
#define S_IRGRP 0040
#define S_IWGRP 0020
#define S_IXGRP 0010
#define S_IRWXO 0007
#define S_IROTH 0004
#define S_IWOTH 0002
#define S_IXOTH 0001
#define S_ISUID 04000
#define S_ISGID 02000
#define S_ISVTX 01000

// 0, or -1 with errno set. KOS has no links, so lstat is stat.
int stat(const char* path, struct stat* st);
int lstat(const char* path, struct stat* st);
int fstat(int fd, struct stat* st);
// The mode is ignored: a new folder gets KOS's usual permissions.
int mkdir(const char* path, mode_t mode);
// Others may read if the mode has S_IROTH or S_IRGRP, and write if it has
// S_IWOTH or S_IWGRP; the owner's bits can't be taken away.
int chmod(const char* path, mode_t mode);
// KOS has no umask: returns 022 and changes nothing.
mode_t umask(mode_t mask);

#endif //C_SYS_STAT_H
