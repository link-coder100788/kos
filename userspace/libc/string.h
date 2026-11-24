#ifndef C_STRING_H
#define C_STRING_H

// The usual C string and memory functions.

#include <stddef.h>

void* memcpy(void* dst, const void* src, size_t n);
// Like memcpy, but the two may overlap.
void* memmove(void* dst, const void* src, size_t n);
void* memset(void* dst, int c, size_t n);
int memcmp(const void* a, const void* b, size_t n);
void* memchr(const void* s, int c, size_t n);

size_t strlen(const char* s);
// strlen, but looks at no more than max bytes.
size_t strnlen(const char* s, size_t max);
char* strcpy(char* dst, const char* src);
// Copies at most n bytes, padding with NULs; doesn't NUL-terminate if src
// is n bytes or longer.
char* strncpy(char* dst, const char* src, size_t n);
// Copies src into a buffer of `size` bytes, always NUL-terminating it.
// Returns strlen(src), so a result >= size means it was cut short.
size_t strlcpy(char* dst, const char* src, size_t size);
char* strcat(char* dst, const char* src);
char* strncat(char* dst, const char* src, size_t n);

int strcmp(const char* a, const char* b);
int strncmp(const char* a, const char* b, size_t n);
// Ignoring the case of ASCII letters.
int strcasecmp(const char* a, const char* b);
int strncasecmp(const char* a, const char* b, size_t n);

char* strchr(const char* s, int c);
char* strrchr(const char* s, int c);
char* strstr(const char* haystack, const char* needle);
// strstr, ignoring case.
char* strcasestr(const char* haystack, const char* needle);
size_t strspn(const char* s, const char* accept);
size_t strcspn(const char* s, const char* reject);
char* strpbrk(const char* s, const char* accept);

// What an errno value (errno.h) means, in words.
char* strerror(int e);
// memset(dst, 0, n).
void bzero(void* dst, size_t n);

// A copy in fresh memory (free() it), or NULL.
char* strdup(const char* s);
char* strndup(const char* s, size_t n);

// Splits a string into tokens separated by any of `delim`, writing NULs
// into it. strtok_r keeps its place in *save; strtok in a static.
char* strtok_r(char* s, const char* delim, char** save);
char* strtok(char* s, const char* delim);

#endif //C_STRING_H
