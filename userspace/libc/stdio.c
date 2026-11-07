#include "stdio.h"
#include "string.h"
#include "sys.h"

// ---- where formatted text goes ----------------------------------------------

typedef struct {
    // Into a buffer (snprintf)...
    char* buf;
    size_t size;
    // ...or to stdout (or descriptor fd - 1, for dprintf), a block at a
    // time (printf).
    int console;
    long fd;
    char pending[256];
    size_t npending;
    // Everything produced, written or not.
    size_t len;
} out_t;

static void flush(out_t* o) {
    if (o->npending && o->fd) fd_write(o->fd - 1, o->pending, o->npending);
    else if (o->npending) write_bytes((const uint8_t*)o->pending, o->npending);
    o->npending = 0;
}

static void put(out_t* o, char c) {
    if (o->console) {
        if (o->npending == sizeof(o->pending)) flush(o);
        o->pending[o->npending++] = c;
    } else if (o->len + 1 < o->size) {
        o->buf[o->len] = c;
    }
    o->len++;
}

static void put_n(out_t* o, char c, int n) {
    while (n-- > 0) put(o, c);
}

// ---- the formatter ------------------------------------------------------------

typedef struct {
    int left, plus, space, zero, alt;
    int width;
    int precision;  // -1: none given
} spec_t;

// Writes `body` (len bytes) with its sign/prefix, padded to the width.
static void emit(out_t* o, const spec_t* sp, const char* prefix, const char* body, int len, int zeros) {
    int plen = (int)strlen(prefix);
    int pad = sp->width - plen - zeros - len;
    if (!sp->left && !sp->zero) put_n(o, ' ', pad);
    for (int i = 0; i < plen; i++) put(o, prefix[i]);
    if (!sp->left && sp->zero) put_n(o, '0', pad);
    put_n(o, '0', zeros);
    for (int i = 0; i < len; i++) put(o, body[i]);
    if (sp->left) put_n(o, ' ', pad);
}

static void format_int(out_t* o, spec_t sp, unsigned long long v, int negative, int base, int upper) {
    char digits[66];
    int n = 0;
    const char* set = upper ? "0123456789ABCDEF" : "0123456789abcdef";
    // A precision of 0 prints nothing for 0.
    if (!(v == 0 && sp.precision == 0)) {
        do { digits[n++] = set[v % base]; v /= base; } while (v);
    }
    char body[66];
    for (int i = 0; i < n; i++) body[i] = digits[n - 1 - i];
    const char* prefix = negative ? "-" : sp.plus ? "+" : sp.space ? " " : "";
    if (sp.alt && base == 16 && n > 0) prefix = upper ? "0X" : "0x";
    int zeros = sp.precision > n ? sp.precision - n : 0;
    if (sp.alt && base == 8 && zeros == 0 && (n == 0 || body[0] != '0')) zeros = 1;
    if (sp.precision >= 0) sp.zero = 0;
    emit(o, &sp, prefix, body, n, zeros);
}

// The decimal digits of v (0 <= v < 1e19, already rounded), into out.
static int uint_digits(char* out, unsigned long long v) {
    char tmp[24];
    int n = 0;
    do { tmp[n++] = '0' + v % 10; v /= 10; } while (v);
    for (int i = 0; i < n; i++) out[i] = tmp[n - 1 - i];
    return n;
}

static double pow10i(int e) {
    double r = 1, b = 10;
    int neg = e < 0;
    if (neg) e = -e;
    for (; e; e >>= 1, b *= b) if (e & 1) r *= b;
    return neg ? 1 / r : r;
}

// v >= 0 in %f style with `prec` decimals. Returns the length.
static int fixed(char* out, double v, int prec) {
    if (prec > 17) prec = 17;
    double scale = pow10i(prec);
    double whole_f = __builtin_floor(v);
    double frac = (v - whole_f) * scale + 0.5;
    unsigned long long frac_i = (unsigned long long)frac;
    if (frac_i >= (unsigned long long)scale) {
        frac_i -= (unsigned long long)scale;
        whole_f += 1;
    }
    int n = 0;
    if (whole_f >= 1e19) {
        // Too big for an integer: print its leading digits, then zeros.
        int e = 0;
        while (whole_f >= 1e18) whole_f /= 10, e++;
        n = uint_digits(out, (unsigned long long)whole_f);
        while (e--) out[n++] = '0';
    } else {
        n = uint_digits(out, (unsigned long long)whole_f);
    }
    if (prec > 0) {
        out[n++] = '.';
        char f[24];
        int fl = uint_digits(f, frac_i);
        for (int i = fl; i < prec; i++) out[n++] = '0';
        for (int i = 0; i < fl; i++) out[n++] = f[i];
    }
    return n;
}

// v > 0 in %e style: d.ddde+XX.
static int scientific(char* out, double v, int prec, int upper) {
    int e = 0;
    if (v != 0) {
        while (v >= 10) v /= 10, e++;
        while (v < 1) v *= 10, e--;
    }
    int n = fixed(out, v, prec);
    // Rounding can make it 10.0...
    if (out[0] == '1' && out[1] == '0' && (n == 2 || out[2] == '.')) {
        n = fixed(out, v / 10, prec);
        e++;
    }
    out[n++] = upper ? 'E' : 'e';
    out[n++] = e < 0 ? '-' : '+';
    if (e < 0) e = -e;
    if (e >= 100) out[n++] = '0' + e / 100;
    out[n++] = '0' + e / 10 % 10;
    out[n++] = '0' + e % 10;
    return n;
}

static void format_double(out_t* o, spec_t sp, double v, char conv) {
    int upper = conv == 'F' || conv == 'E' || conv == 'G';
    char body[400];
    int n;
    int negative = __builtin_signbit(v);
    if (negative) v = -v;
    const char* prefix = negative ? "-" : sp.plus ? "+" : sp.space ? " " : "";
    if (v != v || v == __builtin_inf()) {
        const char* word = v != v ? (upper ? "NAN" : "nan") : (upper ? "INF" : "inf");
        sp.zero = 0;
        emit(o, &sp, prefix, word, 3, 0);
        return;
    }
    int prec = sp.precision < 0 ? 6 : sp.precision;
    if (conv == 'f' || conv == 'F') {
        n = fixed(body, v, prec);
    } else if (conv == 'e' || conv == 'E') {
        n = scientific(body, v, prec, upper);
    } else {
        // %g: %e for very big or small numbers, %f otherwise, with
        // `prec` significant digits and no trailing zeros.
        if (prec == 0) prec = 1;
        int e = 0;
        double t = v;
        if (t != 0) {
            while (t >= 10) t /= 10, e++;
            while (t < 1) t *= 10, e--;
        }
        if (e < -4 || e >= prec) n = scientific(body, v, prec - 1, upper);
        else n = fixed(body, v, prec - 1 - e);
        if (!sp.alt) {
            // Drop trailing zeros in the fraction (before any exponent).
            int exp_at = n;
            for (int i = 0; i < n; i++) if (body[i] == 'e' || body[i] == 'E') exp_at = i;
            int dot = -1;
            for (int i = 0; i < exp_at; i++) if (body[i] == '.') dot = i;
            if (dot >= 0) {
                int cut = exp_at;
                while (cut > dot + 1 && body[cut - 1] == '0') cut--;
                if (cut == dot + 1) cut = dot;
                memmove(body + cut, body + exp_at, n - exp_at);
                n -= exp_at - cut;
            }
        }
    }
    emit(o, &sp, prefix, body, n, 0);
}

static void format(out_t* o, const char* f, va_list args) {
    for (; *f; f++) {
        if (*f != '%') {
            put(o, *f);
            continue;
        }
        f++;
        spec_t sp = { 0, 0, 0, 0, 0, 0, -1 };
        for (;; f++) {
            if (*f == '-') sp.left = 1;
            else if (*f == '+') sp.plus = 1;
            else if (*f == ' ') sp.space = 1;
            else if (*f == '0') sp.zero = 1;
            else if (*f == '#') sp.alt = 1;
            else break;
        }
        if (*f == '*') {
            sp.width = va_arg(args, int);
            if (sp.width < 0) sp.left = 1, sp.width = -sp.width;
            f++;
        } else {
            while (*f >= '0' && *f <= '9') sp.width = sp.width * 10 + (*f++ - '0');
        }
        if (*f == '.') {
            f++;
            sp.precision = 0;
            if (*f == '*') {
                sp.precision = va_arg(args, int);
                f++;
            } else {
                while (*f >= '0' && *f <= '9') sp.precision = sp.precision * 10 + (*f++ - '0');
            }
        }
        if (sp.left) sp.zero = 0;
        // Length: 0 int, 1 long, 2 long long, -1 short, -2 char.
        int length = 0;
        if (*f == 'h') { length = -1; f++; if (*f == 'h') { length = -2; f++; } }
        else if (*f == 'l') { length = 1; f++; if (*f == 'l') { length = 2; f++; } }
        else if (*f == 'z' || *f == 'j' || *f == 't') { length = 2; f++; }
        if (*f == '\0') break;
        switch (*f) {
            case 'd':
            case 'i': {
                long long v = length == 2 ? va_arg(args, long long)
                            : length == 1 ? va_arg(args, long)
                            : va_arg(args, int);
                if (length == -1) v = (short)v;
                if (length == -2) v = (signed char)v;
                format_int(o, sp, v < 0 ? -(unsigned long long)v : (unsigned long long)v, v < 0, 10, 0);
                break;
            }
            case 'u':
            case 'x':
            case 'X':
            case 'o': {
                unsigned long long v = length == 2 ? va_arg(args, unsigned long long)
                                     : length == 1 ? va_arg(args, unsigned long)
                                     : va_arg(args, unsigned int);
                if (length == -1) v = (unsigned short)v;
                if (length == -2) v = (unsigned char)v;
                int base = *f == 'u' ? 10 : *f == 'o' ? 8 : 16;
                sp.plus = sp.space = 0;
                format_int(o, sp, v, 0, base, *f == 'X');
                break;
            }
            case 'p': {
                void* p = va_arg(args, void*);
                sp.alt = 1;
                sp.plus = sp.space = 0;
                if (p) format_int(o, sp, (unsigned long long)p, 0, 16, 0);
                else emit(o, &sp, "", "(nil)", 5, 0);
                break;
            }
            case 'c': {
                char c = (char)va_arg(args, int);
                emit(o, &sp, "", &c, 1, 0);
                break;
            }
            case 's': {
                const char* s = va_arg(args, const char*);
                if (!s) s = "(null)";
                int n = sp.precision >= 0 ? (int)strnlen(s, sp.precision) : (int)strlen(s);
                sp.zero = 0;
                emit(o, &sp, "", s, n, 0);
                break;
            }
            case 'f': case 'F': case 'e': case 'E': case 'g': case 'G':
                format_double(o, sp, va_arg(args, double), *f);
                break;
            case '%':
                put(o, '%');
                break;
            default:
                put(o, '%');
                put(o, *f);
                break;
        }
    }
}

// ---- the functions ------------------------------------------------------------

int vprintf(const char* format_str, va_list args) {
    out_t o = { 0 };
    o.console = 1;
    format(&o, format_str, args);
    flush(&o);
    return (int)o.len;
}

int printf(const char* format_str, ...) {
    va_list args;
    va_start(args, format_str);
    int n = vprintf(format_str, args);
    va_end(args);
    return n;
}

int vdprintf(int fd, const char* format_str, va_list args) {
    out_t o = { 0 };
    o.console = 1;
    o.fd = fd + 1;
    format(&o, format_str, args);
    flush(&o);
    return (int)o.len;
}

int dprintf(int fd, const char* format_str, ...) {
    va_list args;
    va_start(args, format_str);
    int n = vdprintf(fd, format_str, args);
    va_end(args);
    return n;
}

int vsnprintf(char* buf, size_t size, const char* format_str, va_list args) {
    out_t o = { 0 };
    o.buf = buf;
    o.size = size;
    format(&o, format_str, args);
    if (size > 0) buf[o.len < size ? o.len : size - 1] = '\0';
    return (int)o.len;
}

int snprintf(char* buf, size_t size, const char* format_str, ...) {
    va_list args;
    va_start(args, format_str);
    int n = vsnprintf(buf, size, format_str, args);
    va_end(args);
    return n;
}

int sprintf(char* buf, const char* format_str, ...) {
    va_list args;
    va_start(args, format_str);
    int n = vsnprintf(buf, (size_t)-1 >> 1, format_str, args);
    va_end(args);
    return n;
}

int asprintf(char** out, const char* format_str, ...) {
    va_list args, again;
    va_start(args, format_str);
    va_copy(again, args);
    int n = vsnprintf(NULL, 0, format_str, args);
    va_end(args);
    *out = alloc(n + 1);
    if (!*out) {
        va_end(again);
        return -1;
    }
    vsnprintf(*out, n + 1, format_str, again);
    va_end(again);
    return n;
}

int puts(const char* s) {
    writeln(s);
    return 0;
}

int putchar(int c) {
    uint8_t b = (uint8_t)c;
    write_byte(&b);
    return c;
}
