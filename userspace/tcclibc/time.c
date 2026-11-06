#include "time.h"
#include "stdio.h"
#include "sys.h"
#include "sys/time.h"

static const char* const DAYS[] = { "Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday" };
static const char* const MONTHS[] = { "January", "February", "March", "April", "May", "June", "July",
                                      "August", "September", "October", "November", "December" };

time_t time(time_t* t) {
    time_t now = unix_time();
    if (t) *t = now;
    return now;
}

int gettimeofday(struct timeval* tv, void* tz) {
    (void)tz;
    static long start_s = -1;
    static uint64_t start_ms;
    if (start_s < 0) {
        start_s = unix_time();
        if (start_s < 0) return -1;
        start_ms = uptime_ms();
    }
    uint64_t ms = (uint64_t)start_s * 1000 + (uptime_ms() - start_ms);
    tv->tv_sec = (time_t)(ms / 1000);
    tv->tv_usec = (long)(ms % 1000) * 1000;
    return 0;
}

// Days since 1970-01-01 for a date (Howard Hinnant's days_from_civil):
// counting years from March makes February, with its leap day, the last
// month, so every year is 365 days plus a leap day at its very end.
static long days_from_civil(long y, int m, int d) {
    y -= m <= 2;
    long era = (y >= 0 ? y : y - 399) / 400;
    long yoe = y - era * 400;
    long doy = (153 * (m + (m > 2 ? -3 : 9)) + 2) / 5 + d - 1;
    long doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    return era * 146097 + doe - 719468;
}

struct tm* gmtime_r(const time_t* t, struct tm* out) {
    long secs = *t;
    long days = secs / 86400, rem = secs % 86400;
    if (rem < 0) rem += 86400, days--;
    out->tm_hour = rem / 3600;
    out->tm_min = rem / 60 % 60;
    out->tm_sec = rem % 60;
    out->tm_wday = (int)((days % 7 + 11) % 7);  // 1970-01-01 was a Thursday
    // The inverse of days_from_civil.
    long z = days + 719468;
    long era = (z >= 0 ? z : z - 146096) / 146097;
    long doe = z - era * 146097;
    long yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    long doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    long mp = (5 * doy + 2) / 153;
    int d = (int)(doy - (153 * mp + 2) / 5 + 1);
    int m = (int)(mp < 10 ? mp + 3 : mp - 9);
    long y = yoe + era * 400 + (m <= 2);
    out->tm_year = (int)(y - 1900);
    out->tm_mon = m - 1;
    out->tm_mday = d;
    out->tm_yday = (int)(days - days_from_civil(y, 1, 1));
    return out;
}

struct tm* localtime_r(const time_t* t, struct tm* out) {
    time_t local = *t + utc_offset();
    return gmtime_r(&local, out);
}

struct tm* gmtime(const time_t* t) {
    static struct tm shared;
    return gmtime_r(t, &shared);
}

struct tm* localtime(const time_t* t) {
    static struct tm shared;
    return localtime_r(t, &shared);
}

time_t timegm(const struct tm* tm) {
    // Roll months over into years first, so days_from_civil gets 1-12.
    long y = tm->tm_year + 1900L + tm->tm_mon / 12;
    int m = tm->tm_mon % 12;
    if (m < 0) m += 12, y--;
    long days = days_from_civil(y, m + 1, 1) + tm->tm_mday - 1;
    return days * 86400 + tm->tm_hour * 3600L + tm->tm_min * 60L + tm->tm_sec;
}

time_t mktime(const struct tm* tm) {
    return timegm(tm) - utc_offset();
}

size_t strftime(char* buf, size_t size, const char* f, const struct tm* tm) {
    size_t n = 0;
    char part[32];
    for (; *f; f++) {
        const char* text = part;
        if (*f != '%') {
            part[0] = *f;
            part[1] = '\0';
        } else {
            f++;
            int hour12 = tm->tm_hour % 12 == 0 ? 12 : tm->tm_hour % 12;
            switch (*f) {
                case 'Y': snprintf(part, sizeof(part), "%d", tm->tm_year + 1900); break;
                case 'y': snprintf(part, sizeof(part), "%02d", (tm->tm_year + 1900) % 100); break;
                case 'm': snprintf(part, sizeof(part), "%02d", tm->tm_mon + 1); break;
                case 'd': snprintf(part, sizeof(part), "%02d", tm->tm_mday); break;
                case 'e': snprintf(part, sizeof(part), "%2d", tm->tm_mday); break;
                case 'H': snprintf(part, sizeof(part), "%02d", tm->tm_hour); break;
                case 'I': snprintf(part, sizeof(part), "%02d", hour12); break;
                case 'M': snprintf(part, sizeof(part), "%02d", tm->tm_min); break;
                case 'S': snprintf(part, sizeof(part), "%02d", tm->tm_sec); break;
                case 'p': text = tm->tm_hour < 12 ? "AM" : "PM"; break;
                case 'a': snprintf(part, sizeof(part), "%.3s", DAYS[tm->tm_wday % 7]); break;
                case 'A': text = DAYS[tm->tm_wday % 7]; break;
                case 'b': snprintf(part, sizeof(part), "%.3s", MONTHS[tm->tm_mon % 12]); break;
                case 'B': text = MONTHS[tm->tm_mon % 12]; break;
                case 'j': snprintf(part, sizeof(part), "%03d", tm->tm_yday + 1); break;
                case 'F': snprintf(part, sizeof(part), "%d-%02d-%02d", tm->tm_year + 1900, tm->tm_mon + 1, tm->tm_mday); break;
                case 'T': snprintf(part, sizeof(part), "%02d:%02d:%02d", tm->tm_hour, tm->tm_min, tm->tm_sec); break;
                case 'R': snprintf(part, sizeof(part), "%02d:%02d", tm->tm_hour, tm->tm_min); break;
                case 'n': text = "\n"; break;
                case 't': text = "\t"; break;
                case '%': text = "%"; break;
                case '\0': f--; text = ""; break;
                default: snprintf(part, sizeof(part), "%%%c", *f); break;
            }
        }
        for (; *text; text++) {
            if (n + 1 >= size) {
                if (size > 0) buf[0] = '\0';
                return 0;
            }
            buf[n++] = *text;
        }
    }
    if (size > 0) buf[n] = '\0';
    return n;
}
