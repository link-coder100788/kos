#include "stdlib.h"
#include "string.h"
#include "ctype.h"
#include "math.h"

void* malloc(size_t size) {
    return alloc(size);
}

void* calloc(size_t n, size_t size) {
    if (size != 0 && n > (size_t)-1 / size) return NULL;
    return alloc_z(n * size);
}

void* realloc(void* ptr, size_t size) {
    if (!ptr) return malloc(size);
    if (size == 0) {
        free(ptr);
        return NULL;
    }
    size_t old = mem_size(ptr);
    if (old >= size) return ptr;
    void* bigger = malloc(size);
    if (!bigger) return NULL;
    memcpy(bigger, ptr, old);
    free(ptr);
    return bigger;
}

int abs(int x) { return x < 0 ? -x : x; }
long labs(long x) { return x < 0 ? -x : x; }
long long llabs(long long x) { return x < 0 ? -x : x; }

// The digits of an unsigned number; *neg says if there was a '-'.
static unsigned long long parse_unsigned(const char* s, char** end, int base, int* neg) {
    const char* p = s;
    while (isspace((unsigned char)*p)) p++;
    *neg = 0;
    if (*p == '-' || *p == '+') *neg = *p++ == '-';
    if ((base == 0 || base == 16) && p[0] == '0' && (p[1] == 'x' || p[1] == 'X') && isxdigit((unsigned char)p[2])) {
        p += 2;
        base = 16;
    } else if (base == 0) {
        base = *p == '0' ? 8 : 10;
    }
    unsigned long long v = 0;
    const char* digits = p;
    for (;; p++) {
        int d;
        if (isdigit((unsigned char)*p)) d = *p - '0';
        else if (isalpha((unsigned char)*p)) d = tolower((unsigned char)*p) - 'a' + 10;
        else break;
        if (d >= base) break;
        v = v * base + d;
    }
    if (end) *end = (char*)(p == digits ? s : p);
    return v;
}

long long strtoll(const char* s, char** end, int base) {
    int neg;
    unsigned long long v = parse_unsigned(s, end, base, &neg);
    return neg ? -(long long)v : (long long)v;
}

unsigned long long strtoull(const char* s, char** end, int base) {
    int neg;
    unsigned long long v = parse_unsigned(s, end, base, &neg);
    return neg ? -v : v;
}

long strtol(const char* s, char** end, int base) { return (long)strtoll(s, end, base); }
unsigned long strtoul(const char* s, char** end, int base) { return (unsigned long)strtoull(s, end, base); }
int atoi(const char* s) { return (int)strtol(s, NULL, 10); }
long atol(const char* s) { return strtol(s, NULL, 10); }

double strtod(const char* s, char** end) {
    const char* p = s;
    while (isspace((unsigned char)*p)) p++;
    int neg = 0;
    if (*p == '-' || *p == '+') neg = *p++ == '-';
    if (strncasecmp(p, "inf", 3) == 0) {
        if (end) *end = (char*)p + (strncasecmp(p, "infinity", 8) == 0 ? 8 : 3);
        return neg ? -INFINITY : INFINITY;
    }
    if (strncasecmp(p, "nan", 3) == 0) {
        if (end) *end = (char*)p + 3;
        return NAN;
    }
    double v = 0;
    int any = 0;
    while (isdigit((unsigned char)*p)) v = v * 10 + (*p++ - '0'), any = 1;
    if (*p == '.') {
        p++;
        double scale = 0.1;
        while (isdigit((unsigned char)*p)) v += (*p++ - '0') * scale, scale /= 10, any = 1;
    }
    if (!any) {
        if (end) *end = (char*)s;
        return 0;
    }
    if ((*p == 'e' || *p == 'E') && (isdigit((unsigned char)p[1]) || ((p[1] == '-' || p[1] == '+') && isdigit((unsigned char)p[2])))) {
        p++;
        int eneg = 0;
        if (*p == '-' || *p == '+') eneg = *p++ == '-';
        int e = 0;
        while (isdigit((unsigned char)*p)) e = e * 10 + (*p++ - '0');
        double f = 1;
        double base = 10;
        // 10^e by squaring.
        for (; e > 0; e >>= 1, base *= base) if (e & 1) f *= base;
        v = eneg ? v / f : v * f;
    }
    if (end) *end = (char*)p;
    return neg ? -v : v;
}

double atof(const char* s) { return strtod(s, NULL); }

static void swap_bytes(char* a, char* b, size_t size) {
    while (size--) {
        char t = *a;
        *a++ = *b;
        *b++ = t;
    }
}

// Quicksort, with insertion sort for short runs.
static void sort(char* lo, size_t n, size_t size, int (*compare)(const void*, const void*)) {
    while (n > 12) {
        char* mid = lo + (n / 2) * size;
        char* hi = lo + (n - 1) * size;
        // The median of three goes to the front, as the pivot.
        if (compare(mid, lo) < 0) swap_bytes(mid, lo, size);
        if (compare(hi, lo) < 0) swap_bytes(hi, lo, size);
        if (compare(hi, mid) < 0) swap_bytes(hi, mid, size);
        swap_bytes(lo, mid, size);
        size_t i = 1, j = n - 1;
        for (;;) {
            while (i <= j && compare(lo + i * size, lo) < 0) i++;
            while (j >= i && compare(lo + j * size, lo) > 0) j--;
            if (i >= j) break;
            swap_bytes(lo + i * size, lo + j * size, size);
            i++, j--;
        }
        swap_bytes(lo, lo + j * size, size);
        // Recurse into the smaller side, loop on the bigger one.
        size_t left = j, right = n - j - 1;
        if (left < right) {
            sort(lo, left, size, compare);
            lo += (j + 1) * size;
            n = right;
        } else {
            sort(lo + (j + 1) * size, right, size, compare);
            n = left;
        }
    }
    for (size_t i = 1; i < n; i++) {
        for (size_t k = i; k > 0 && compare(lo + (k - 1) * size, lo + k * size) > 0; k--) {
            swap_bytes(lo + (k - 1) * size, lo + k * size, size);
        }
    }
}

void qsort(void* base, size_t n, size_t size, int (*compare)(const void*, const void*)) {
    if (n > 1 && size > 0) sort(base, n, size, compare);
}

void* bsearch(const void* key, const void* base, size_t n, size_t size, int (*compare)(const void*, const void*)) {
    const char* lo = base;
    while (n > 0) {
        const char* mid = lo + (n / 2) * size;
        int c = compare(key, mid);
        if (c == 0) return (void*)mid;
        if (c > 0) {
            lo = mid + size;
            n -= n / 2 + 1;
        } else {
            n /= 2;
        }
    }
    return NULL;
}

// xorshift64*; seeded from the hardware on first use.
static uint64_t rand_state;

void srand(unsigned int seed) {
    rand_state = seed * 0x9e3779b97f4a7c15ull + 1;
}

int rand(void) {
    if (rand_state == 0) {
        rand_state = random_u64() | 1;
    }
    rand_state ^= rand_state >> 12;
    rand_state ^= rand_state << 25;
    rand_state ^= rand_state >> 27;
    return (int)((rand_state * 0x2545f4914f6cdd1dull) >> 33);
}

int rand_range(int lo, int hi) {
    if (hi <= lo) return lo;
    return lo + (int)((unsigned)rand() % (unsigned)(hi - lo + 1));
}
