#ifndef C_MATH_H
#define C_MATH_H

// Maths functions, written from scratch (there's no libm): see math.c for
// how each one works. Accuracy is close to a full double (about 15-16
// significant digits) for ordinary arguments. sin and cos lose accuracy
// for huge angles (beyond about 1e9 radians).

#define M_PI 3.14159265358979323846
#define M_E  2.71828182845904523536

// e^x.
double exp(double x);

// Natural logarithm (base e). NaN for x < 0, -infinity for 0.
double log(double x);

// Base-10 and base-2 logarithms (log2 of a power of two is exact).
double log10(double x);
double log2(double x);

// 2^x.
double exp2(double x);

// x^y. NaN for a negative x with a non-integer y.
double pow(double x, double y);

// Sine and cosine, x in radians.
double sin(double x);
double cos(double x);
double tan(double x);

// Inverses, in radians: asin and acos take -1..1 (NaN outside it); atan2
// is the angle of the point (x, y), in -pi..pi.
double asin(double x);
double acos(double x);
double atan(double x);
double atan2(double y, double x);

// Square root (NaN for x < 0), and sqrt(x*x + y*y).
double sqrt(double x);
double hypot(double x, double y);

// Rounding to a whole number: down, up, to the nearest (halves away from
// zero), and towards zero.
double floor(double x);
double ceil(double x);
double round(double x);
double trunc(double x);

// x * 2^e.
double ldexp(double x, int e);
// The other way: x = m * 2^*e with m in [0.5, 1) (or 0), returning m.
double frexp(double x, int* e);
// Splits x into a whole part (*whole) and a fraction, both with x's sign.
double modf(double x, double* whole);
// ldexp, in double precision.
long double ldexpl(long double x, int e);

// The remainder of x / y, with x's sign.
double fmod(double x, double y);

double fabs(double x);
double fmin(double a, double b);
double fmax(double a, double b);

// (float.h may have defined these already.)
#ifndef INFINITY
#ifdef __TINYC__
// tcc has no builtins for these: a too-big constant is infinity, 0/0 is NaN.
#define INFINITY (1e500)
#else
#define INFINITY (__builtin_inf())
#endif
#endif
#ifndef NAN
#ifdef __TINYC__
#define NAN (0.0/0.0)
#else
#define NAN (__builtin_nan(""))
#endif
#endif
#define HUGE_VAL ((double)INFINITY)
#define HUGE_VALF ((float)INFINITY)
#define HUGE_VALL ((long double)INFINITY)
static inline int isnan(double x) { return x != x; }
static inline int isinf(double x) { return x == INFINITY || x == -INFINITY; }
static inline int isfinite(double x) { return !isnan(x) && !isinf(x); }
static inline int signbit(double x) {
    union { double d; unsigned long long u; } b = { x };
    return (int)(b.u >> 63);
}

#endif //C_MATH_H
