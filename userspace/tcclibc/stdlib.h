#ifndef C_STDLIB_H
#define C_STDLIB_H

// Memory, number parsing, sorting and random numbers. (exit() is in
// sys.h.)

#include <stddef.h>
#include "sys.h"

// The standard names for sys.h's alloc functions. free() is in sys.h.
void* malloc(size_t size);
// Zeroed memory for n things of `size` bytes, or NULL (also if n * size
// overflows).
void* calloc(size_t n, size_t size);
// Resizes an allocation, moving it if need be. realloc(NULL, n) is
// malloc(n); realloc(p, 0) frees p and returns NULL.
void* realloc(void* ptr, size_t size);

int abs(int x);
long labs(long x);
long long llabs(long long x);

// Parse a number at the start of s, after any spaces. `end`, if not NULL,
// is set to just past it. base 0 accepts 0x... (hex) and 0... (octal).
long strtol(const char* s, char** end, int base);
unsigned long strtoul(const char* s, char** end, int base);
long long strtoll(const char* s, char** end, int base);
unsigned long long strtoull(const char* s, char** end, int base);
double strtod(const char* s, char** end);
// strtod, then made a float / long double (so only double's precision).
float strtof(const char* s, char** end);
long double strtold(const char* s, char** end);
int atoi(const char* s);
long atol(const char* s);
double atof(const char* s);

// KOS has no environment variables: always NULL.
char* getenv(const char* name);
// KOS paths have no links to resolve, so this copies `path` into
// `resolved` (PATH_MAX bytes), or into fresh memory if that's NULL.
char* realpath(const char* path, char* resolved);

// Sorts n things of `size` bytes with compare (negative, 0, positive).
void qsort(void* base, size_t n, size_t size, int (*compare)(const void*, const void*));
// Finds key in a sorted array, or returns NULL.
void* bsearch(const void* key, const void* base, size_t n, size_t size, int (*compare)(const void*, const void*));

// Quick pseudo-random numbers, 0..RAND_MAX. They start from a hardware
// random seed, so each run differs; srand(n) makes them repeatable.
#define RAND_MAX 0x7fffffff
int rand(void);
void srand(unsigned int seed);
// A random number in lo..hi, both included.
int rand_range(int lo, int hi);

#endif //C_STDLIB_H
