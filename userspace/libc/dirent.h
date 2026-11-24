#ifndef C_DIRENT_H
#define C_DIRENT_H

// Reading folders: opendir(), then readdir() until it returns NULL. The
// listing is taken when the folder is opened, and doesn't include "." or
// "..".

#include "sys/types.h"

#define DT_UNKNOWN 0
#define DT_DIR 4
#define DT_REG 8

struct dirent {
    ino_t d_ino;
    unsigned char d_type;
    char d_name[256];
};

typedef struct DIR DIR;

DIR* opendir(const char* path);
// The next entry (overwritten by the next call), or NULL after the last.
struct dirent* readdir(DIR* dir);
void rewinddir(DIR* dir);
int closedir(DIR* dir);

#endif //C_DIRENT_H
