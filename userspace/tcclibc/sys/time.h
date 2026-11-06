#ifndef C_SYS_TIME_H
#define C_SYS_TIME_H

// The time, to the millisecond: the clock chip's time at the first call,
// moved on by the uptime counter since, so it never goes backwards.

#include "../time.h"

struct timeval {
    time_t tv_sec;
    long tv_usec;
};

// `tz` must be NULL. Returns 0, or -1 if the clock can't be read.
int gettimeofday(struct timeval* tv, void* tz);

#endif //C_SYS_TIME_H
