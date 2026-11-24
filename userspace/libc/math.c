// Maths functions from scratch. The CPU only does + - * / and square
// roots in hardware; everything else is built from those.
//
// (sqrt and the rounding functions are single instructions.)
//
// Each other function uses the same two-step recipe:
//   1. Range reduction: use an exact identity to turn any input into an
//      equivalent small one (sin repeats every 2*pi; e^x = 2^k * e^r).
//   2. A polynomial: on a small range, a few terms of the function's
//      Taylor series are accurate to the last bit of a double.

#include <stdint.h>
#include "math.h"

// Doubles as their bits, for building and taking apart numbers directly:
// 1 sign bit, 11 exponent bits (biased by 1023), 52 fraction bits.
typedef union {
    double d;
    uint64_t u;
} bits_t;

static const double INF = 1.0 / 0.0;
#define NAN_VALUE (0.0 / 0.0)

// ln(2) split in two: HI has trailing zero bits, so k * HI is exact for
// the k we use, and LO holds the rest. Subtracting them separately keeps
// precision that x - k * ln2 in one step would round away (Cody and Waite).
static const double LN2_HI = 6.93147180369123816490e-01;
static const double LN2_LO = 1.90821492927058770002e-10;
static const double INV_LN2 = 1.44269504088896338700e+00;
static const double INV_LN10 = 4.34294481903251827651e-01;

// pi/2 in three parts, for the same reason.
static const double PIO2_1 = 1.57079632673412561417e+00;
static const double PIO2_2 = 6.07710050630396597660e-11;
static const double PIO2_3 = 2.02226624879595063154e-21;
static const double TWO_OVER_PI = 6.36619772367581382433e-01;

static double round_nearest(double x) {
    return (double)(int64_t)(x >= 0 ? x + 0.5 : x - 0.5);
}

// 2^k as a double, built from its bits: the exponent field is k + 1023.
static double pow2(int k) {
    bits_t b;
    b.u = (uint64_t)(k + 1023) << 52;
    return b.d;
}

// ---- exp ------------------------------------------------------------------------
// e^x = e^(k*ln2 + r) = 2^k * e^r, choosing k so that |r| <= ln2/2 = 0.35.
// 2^k is free (it's just the exponent bits), and on |r| <= 0.35 the Taylor
// series 1 + r + r^2/2! + ... + r^13/13! is accurate to about 1e-18.

double exp(double x) {
    if (x != x) return x;            // NaN
    if (x > 709.78) return INF;       // beyond the largest double
    if (x < -745.14) return 0.0;      // below the smallest
    int k = (int)round_nearest(x * INV_LN2);
    double r = (x - k * LN2_HI) - k * LN2_LO;
    // Horner's method: 1 + r(1 + r/2(1 + r/3(...))), from the inside out.
    double sum = 1.0;
    for (int n = 13; n >= 1; n--) sum = 1.0 + sum * r / n;
    // Apply 2^k in two halves, so neither factor overflows on its own
    // (k can reach about +-1075 near the ends of the range).
    int k1 = k / 2;
    return sum * pow2(k1) * pow2(k - k1);
}

// ---- log --------------------------------------------------------------------------
// Split x = m * 2^e (the exponent bits give e), so ln x = e*ln2 + ln m,
// with m kept in [0.71, 1.41]. Then with s = (m-1)/(m+1), which is at most
// 0.172 there:  ln m = 2(s + s^3/3 + s^5/5 + ...)  converges very fast.

double log(double x) {
    if (x != x) return x;
    if (x < 0) return NAN_VALUE;
    if (x == 0) return -INF;
    if (x == INF) return INF;
    bits_t b = { .d = x };
    int e = (int)((b.u >> 52) & 0x7ff) - 1023;
    if (e == -1023) {
        // A subnormal (tiny) number: scale it up into the normal range.
        b.d = x * 18014398509481984.0;  // 2^54
        e = (int)((b.u >> 52) & 0x7ff) - 1023 - 54;
    }
    // Keep the fraction bits, set the exponent to 0: m is in [1, 2).
    b.u = (b.u & 0x000fffffffffffffULL) | 0x3ff0000000000000ULL;
    double m = b.d;
    if (m > 1.41421356237309504880) {
        m /= 2;
        e++;
    }
    double s = (m - 1) / (m + 1), s2 = s * s;
    double sum = 0.0;
    for (int n = 21; n >= 3; n -= 2) sum = (sum + 1.0 / n) * s2;
    double ln_m = 2 * s * (1.0 + sum);
    return e * LN2_HI + (ln_m + e * LN2_LO);
}

double log10(double x) {
    return log(x) * INV_LN10;
}

// ---- pow ----------------------------------------------------------------------------

double pow(double x, double y) {
    // Whole powers by repeated squaring: exact where possible, and works
    // for negative x (-2^3 = -8).
    if (y == round_nearest(y) && y >= -2147483648.0 && y <= 2147483647.0) {
        int64_t n = (int64_t)y;
        uint64_t left = n < 0 ? (uint64_t)(-n) : (uint64_t)n;
        double result = 1.0, base = x;
        while (left) {
            if (left & 1) result *= base;
            base *= base;
            left >>= 1;
        }
        return n < 0 ? 1.0 / result : result;
    }
    if (x < 0) return NAN_VALUE;  // e.g. (-8)^0.5 isn't a real number
    if (x == 0) return y > 0 ? 0.0 : INF;
    return exp(y * log(x));
}

// ---- sin and cos ----------------------------------------------------------------------
// sin and cos repeat every 2*pi, and each quarter turn swaps or negates
// them:  sin(r + k*pi/2) is sin r, cos r, -sin r or -cos r for k = 0..3.
// So: write x = k*pi/2 + r with |r| <= pi/4, compute sin r and cos r with
// Taylor series (accurate to ~1e-19 on that range), and pick by k.

static double sin_small(double r) {
    // r - r^3/3! + r^5/5! - ... - r^19/19!, by Horner.
    double r2 = r * r, sum = 1.0;
    for (int n = 19; n >= 3; n -= 2) sum = 1.0 - sum * r2 / (n * (n - 1));
    return r * sum;
}

static double cos_small(double r) {
    // 1 - r^2/2! + r^4/4! - ... + r^20/20!.
    double r2 = r * r, sum = 1.0;
    for (int n = 20; n >= 2; n -= 2) sum = 1.0 - sum * r2 / (n * (n - 1));
    return sum;
}

// x = k*pi/2 + r. Returns r and sets *quadrant to k mod 4.
static double reduce(double x, int* quadrant) {
    double k = round_nearest(x * TWO_OVER_PI);
    *quadrant = (int)((int64_t)k & 3);
    return ((x - k * PIO2_1) - k * PIO2_2) - k * PIO2_3;
}

double sin(double x) {
    if (x != x || x == INF || x == -INF) return NAN_VALUE;
    int q;
    double r = reduce(x, &q);
    switch (q) {
    case 0: return sin_small(r);
    case 1: return cos_small(r);
    case 2: return -sin_small(r);
    default: return -cos_small(r);
    }
}

double cos(double x) {
    if (x != x || x == INF || x == -INF) return NAN_VALUE;
    int q;
    double r = reduce(x, &q);
    switch (q) {
    case 0: return cos_small(r);
    case 1: return -sin_small(r);
    case 2: return -cos_small(r);
    default: return sin_small(r);
    }
}

double tan(double x) {
    return sin(x) / cos(x);
}

// sqrt, floor, ceil, round and trunc are one instruction each: see fp.S.

double fabs(double x) {
    bits_t b = { x };
    b.u &= ~(1ull << 63);
    return b.d;
}

double fmin(double a, double b) { return a != a ? b : b != b ? a : a < b ? a : b; }
double fmax(double a, double b) { return a != a ? b : b != b ? a : a > b ? a : b; }

double ldexp(double x, int e) {
    // In steps that can't overflow on the way (2^1000 fits in a double).
    while (e > 1000) { x *= 0x1p1000; e -= 1000; }
    while (e < -1000) { x *= 0x1p-1000; e += 1000; }
    bits_t scale = { 0 };
    scale.u = (uint64_t)(e + 1023) << 52;
    // e + 1023 is 23..2023 here, a normal double's exponent.
    return x * scale.d;
}

long double ldexpl(long double x, int e) { return ldexp((double)x, e); }

double fmod(double x, double y) {
    if (y == 0 || x != x || y != y || x == INF || x == -INF) return NAN_VALUE;
    double r = x - trunc(x / y) * y;
    // x / y can round up by one; keep the result's sign x's and |r| < |y|.
    if (fabs(r) >= fabs(y)) r = 0;
    if (r != 0 && (r < 0) != (x < 0)) r += y < 0 ? -y : y;
    return r;
}

double hypot(double x, double y) {
    x = fabs(x);
    y = fabs(y);
    if (x < y) { double t = x; x = y; y = t; }
    if (x == 0) return 0;
    // Divide first, so x*x can't overflow.
    double r = y / x;
    return x * sqrt(1 + r * r);
}

// ---- inverse trigonometry ---------------------------------------------------------

// atan(r) for |r| <= tan(pi/12) ~ 0.268, by its Taylor series
// r - r^3/3 + r^5/5 - ...; 14 terms reach the last bit.
static double atan_small(double r) {
    double r2 = r * r, term = r, sum = 0;
    for (int k = 0; k < 14; k++) {
        sum += term / (2 * k + 1);
        term *= -r2;
    }
    return sum;
}

double atan(double x) {
    if (x != x) return x;
    int negative = x < 0;
    if (negative) x = -x;
    double result;
    // atan(x) = pi/2 - atan(1/x), which brings x down to 0..1; then
    // atan(x) = pi/6 + atan((x - 1/sqrt(3)) / (1 + x/sqrt(3))) brings
    // what's still above tan(pi/12) down too.
    int inverted = x > 1;
    if (inverted) x = 1 / x;
    if (x > 0.2679491924311227) {
        const double INV_SQRT3 = 0.57735026918962576451;
        result = M_PI / 6 + atan_small((x - INV_SQRT3) / (1 + x * INV_SQRT3));
    } else {
        result = atan_small(x);
    }
    if (inverted) result = M_PI / 2 - result;
    return negative ? -result : result;
}

double atan2(double y, double x) {
    if (x != x || y != y) return NAN_VALUE;
    if (x > 0) return atan(y / x);
    if (x < 0) return y >= 0 ? atan(y / x) + M_PI : atan(y / x) - M_PI;
    return y > 0 ? M_PI / 2 : y < 0 ? -M_PI / 2 : 0;
}

double asin(double x) {
    if (x < -1 || x > 1) return NAN_VALUE;
    return atan2(x, sqrt((1 - x) * (1 + x)));
}

double acos(double x) {
    if (x < -1 || x > 1) return NAN_VALUE;
    return atan2(sqrt((1 - x) * (1 + x)), x);
}
