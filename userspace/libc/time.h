#ifndef C_TIME_H
#define C_TIME_H

// Dates and times, from the machine's clock. uptime_ms() (in sys.h) is
// better for timing things.

#include <stddef.h>

typedef long time_t;

struct tm {
    int tm_sec;    // 0-59
    int tm_min;    // 0-59
    int tm_hour;   // 0-23
    int tm_mday;   // 1-31
    int tm_mon;    // 0-11
    int tm_year;   // years since 1900
    int tm_wday;   // 0-6, Sunday is 0
    int tm_yday;   // 0-365
};

// Seconds since 1970-01-01 00:00 UTC (also stored in *t if t isn't NULL),
// or -1 if there's no clock.
time_t time(time_t* t);

// Splits t into a date and time: in UTC, or in the local time zone.
struct tm* gmtime_r(const time_t* t, struct tm* out);
struct tm* localtime_r(const time_t* t, struct tm* out);
// The same, into a static struct shared between calls.
struct tm* gmtime(const time_t* t);
struct tm* localtime(const time_t* t);

// The other way: a UTC date / a local date back to seconds since 1970.
// Out-of-range fields (a 32nd of January) roll over.
time_t timegm(const struct tm* tm);
time_t mktime(const struct tm* tm);

// Formats tm into buf (size bytes, NUL-terminated). Returns the length,
// or 0 if it didn't fit. Conversions: %Y %y %m %d %e %H %I %M %S %p %a %A
// %b %B %j %F (%Y-%m-%d) %T (%H:%M:%S) %R (%H:%M) %n %t %%.
size_t strftime(char* buf, size_t size, const char* format, const struct tm* tm);

#endif //C_TIME_H
