// No include guard: each include sees the NDEBUG of the moment, as the
// standard says.

#undef assert
#ifdef NDEBUG
#define assert(e) ((void)0)
#else
// Prints the failed condition and where it is, then exits.
__attribute__((noreturn)) void __assert_fail(const char* expr, const char* file, int line);
#define assert(e) ((e) ? (void)0 : __assert_fail(#e, __FILE__, __LINE__))
#endif
