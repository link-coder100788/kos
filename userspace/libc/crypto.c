#include "crypto.h"
#include "string.h"
#include "sys.h"
#include "syscall.h"

#include <stdbool.h>

#define SVC_CRYPTO(sub, a0, a1, a2, a3, a4) ((long)KOS_SVC(0xf, sub, a0, a1, a2, a3, a4))

enum { SHA256, SHA512, HMAC_SHA256, AES_GCM_SEAL, AES_GCM_OPEN, ARGON2ID_HASH, ARGON2ID_VERIFY };

long sha256(const void* data, size_t len, uint8_t out[SHA256_LEN]) {
    return SVC_CRYPTO(SHA256, data, len, out, 0, 0);
}

long sha512(const void* data, size_t len, uint8_t out[SHA512_LEN]) {
    return SVC_CRYPTO(SHA512, data, len, out, 0, 0);
}

long hmac_sha256(const void* key, size_t key_len, const void* data, size_t len, uint8_t out[SHA256_LEN]) {
    return SVC_CRYPTO(HMAC_SHA256, key, key_len, data, len, out);
}

int crypto_equal(const void* a, const void* b, size_t len) {
    const volatile uint8_t* x = a;
    const volatile uint8_t* y = b;
    uint8_t diff = 0;
    for (size_t i = 0; i < len; i++) diff |= x[i] ^ y[i];
    return diff == 0;
}

long aes_gcm_seal(const aead_t* a) {
    return SVC_CRYPTO(AES_GCM_SEAL, a, 0, 0, 0, 0);
}

long aes_gcm_open(const aead_t* a) {
    return SVC_CRYPTO(AES_GCM_OPEN, a, 0, 0, 0, 0);
}

long argon2id_hash(const char* password, size_t len, char* out, size_t max) {
    return SVC_CRYPTO(ARGON2ID_HASH, password, len, out, max, 0);
}

int argon2id_verify(const char* password, size_t len, const char* phc) {
    return SVC_CRYPTO(ARGON2ID_VERIFY, password, len, phc, 0, 0) == 1;
}

// ---- X25519 and Ed25519 ------------------------------------------------------------
// Curve25519 arithmetic in the style of TweetNaCl (public domain): a field
// element is 16 limbs of 16 bits. X25519, Ed25519 signing and the scalar
// multiplications run in constant time; checking a signature only handles
// public data.

typedef int64_t gf[16];

static const gf gf0;
static const gf gf1 = {1};
static const gf c121665 = {0xDB41, 1};
static const gf ed_d = {0x78a3, 0x1359, 0x4dca, 0x75eb, 0xd8ab, 0x4141, 0x0a4d, 0x0070,
                        0xe898, 0x7779, 0x4079, 0x8cc7, 0xfe73, 0x2b6f, 0x6cee, 0x5203};
static const gf ed_d2 = {0xf159, 0x26b2, 0x9b94, 0xebd6, 0xb156, 0x8283, 0x149a, 0x00e0,
                         0xd130, 0xeef3, 0x80f2, 0x198e, 0xfce7, 0x56df, 0xd9dc, 0x2406};
static const gf ed_x = {0xd51a, 0x8f25, 0x2d60, 0xc956, 0xa7b2, 0x9525, 0xc760, 0x692c,
                        0xdc5c, 0xfdd6, 0xe231, 0xc0a4, 0x53fe, 0xcd6e, 0x36d3, 0x2169};
static const gf ed_y = {0x6658, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666,
                        0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666};
/// The square root of -1.
static const gf sqrtm1 = {0xa0b0, 0x4a0e, 0x1b27, 0xc4ee, 0xe478, 0xad2f, 0x1806, 0x2f43,
                          0xd7a7, 0x3dfb, 0x0099, 0x2b4d, 0xdf0b, 0x4fc1, 0x2480, 0x2b83};

static void set25519(gf r, const gf a) {
    for (int i = 0; i < 16; i++) r[i] = a[i];
}

static void car25519(gf o) {
    for (int i = 0; i < 16; i++) {
        o[i] += (1LL << 16);
        int64_t c = o[i] >> 16;
        o[(i + 1) * (i < 15)] += c - 1 + 37 * (c - 1) * (i == 15);
        o[i] -= c * (1LL << 16);
    }
}

/// Swaps p and q if b is 1, in constant time.
static void sel25519(gf p, gf q, int b) {
    int64_t c = ~(b - 1);
    for (int i = 0; i < 16; i++) {
        int64_t t = c & (p[i] ^ q[i]);
        p[i] ^= t;
        q[i] ^= t;
    }
}

static void pack25519(uint8_t* o, const gf n) {
    gf m, t;
    set25519(t, n);
    car25519(t);
    car25519(t);
    car25519(t);
    for (int j = 0; j < 2; j++) {
        m[0] = t[0] - 0xffed;
        for (int i = 1; i < 15; i++) {
            m[i] = t[i] - 0xffff - ((m[i - 1] >> 16) & 1);
            m[i - 1] &= 0xffff;
        }
        m[15] = t[15] - 0x7fff - ((m[14] >> 16) & 1);
        int b = (m[15] >> 16) & 1;
        m[14] &= 0xffff;
        sel25519(t, m, 1 - b);
    }
    for (int i = 0; i < 16; i++) {
        o[2 * i] = t[i] & 0xff;
        o[2 * i + 1] = t[i] >> 8;
    }
}

static bool eq25519(const gf a, const gf b) {
    uint8_t c[32], d[32];
    pack25519(c, a);
    pack25519(d, b);
    return memcmp(c, d, 32) == 0;
}

static uint8_t par25519(const gf a) {
    uint8_t d[32];
    pack25519(d, a);
    return d[0] & 1;
}

static void unpack25519(gf o, const uint8_t* n) {
    for (int i = 0; i < 16; i++) o[i] = n[2 * i] + ((int64_t)n[2 * i + 1] << 8);
    o[15] &= 0x7fff;
}

static void fadd(gf o, const gf a, const gf b) {
    for (int i = 0; i < 16; i++) o[i] = a[i] + b[i];
}

static void fsub(gf o, const gf a, const gf b) {
    for (int i = 0; i < 16; i++) o[i] = a[i] - b[i];
}

static void fmul(gf o, const gf a, const gf b) {
    int64_t t[31] = {0};
    for (int i = 0; i < 16; i++)
        for (int j = 0; j < 16; j++) t[i + j] += a[i] * b[j];
    for (int i = 0; i < 15; i++) t[i] += 38 * t[i + 16];
    for (int i = 0; i < 16; i++) o[i] = t[i];
    car25519(o);
    car25519(o);
}

static void fsq(gf o, const gf a) {
    fmul(o, a, a);
}

static void inv25519(gf o, const gf in) {
    gf c;
    set25519(c, in);
    for (int a = 253; a >= 0; a--) {
        fsq(c, c);
        if (a != 2 && a != 4) fmul(c, c, in);
    }
    set25519(o, c);
}

static void pow2523(gf o, const gf in) {
    gf c;
    set25519(c, in);
    for (int a = 250; a >= 0; a--) {
        fsq(c, c);
        if (a != 1) fmul(c, c, in);
    }
    set25519(o, c);
}

void x25519(uint8_t q[32], const uint8_t n[32], const uint8_t* p) {
    static const uint8_t base[32] = {9};
    uint8_t z[32];
    gf x, a, b, c, d, e, f;
    memcpy(z, n, 32);
    z[31] = (n[31] & 127) | 64;
    z[0] &= 248;
    unpack25519(x, p ? p : base);
    for (int i = 0; i < 16; i++) {
        b[i] = x[i];
        d[i] = a[i] = c[i] = 0;
    }
    a[0] = d[0] = 1;
    for (int i = 254; i >= 0; --i) {
        int r = (z[i >> 3] >> (i & 7)) & 1;
        sel25519(a, b, r);
        sel25519(c, d, r);
        fadd(e, a, c);
        fsub(a, a, c);
        fadd(c, b, d);
        fsub(b, b, d);
        fsq(d, e);
        fsq(f, a);
        fmul(a, c, a);
        fmul(c, b, e);
        fadd(e, a, c);
        fsub(a, a, c);
        fsq(b, a);
        fsub(c, d, f);
        fmul(a, c, c121665);
        fadd(a, a, d);
        fmul(c, c, a);
        fmul(a, d, f);
        fmul(d, b, x);
        fsq(b, e);
        sel25519(a, b, r);
        sel25519(c, d, r);
    }
    inv25519(c, c);
    fmul(a, a, c);
    pack25519(q, a);
}

// Ed25519 points in extended coordinates (X, Y, Z, T).

static void ed_add(gf p[4], gf q[4]) {
    gf a, b, c, d, t, e, f, g, h;
    fsub(a, p[1], p[0]);
    fsub(t, q[1], q[0]);
    fmul(a, a, t);
    fadd(b, p[0], p[1]);
    fadd(t, q[0], q[1]);
    fmul(b, b, t);
    fmul(c, p[3], q[3]);
    fmul(c, c, ed_d2);
    fmul(d, p[2], q[2]);
    fadd(d, d, d);
    fsub(e, b, a);
    fsub(f, d, c);
    fadd(g, d, c);
    fadd(h, b, a);
    fmul(p[0], e, f);
    fmul(p[1], h, g);
    fmul(p[2], g, f);
    fmul(p[3], e, h);
}

static void ed_pack(uint8_t* r, gf p[4]) {
    gf tx, ty, zi;
    inv25519(zi, p[2]);
    fmul(tx, p[0], zi);
    fmul(ty, p[1], zi);
    pack25519(r, ty);
    r[31] ^= par25519(tx) << 7;
}

static void ed_scalarmult(gf p[4], gf q[4], const uint8_t* s) {
    set25519(p[0], gf0);
    set25519(p[1], gf1);
    set25519(p[2], gf1);
    set25519(p[3], gf0);
    for (int i = 255; i >= 0; --i) {
        int b = (s[i / 8] >> (i & 7)) & 1;
        for (int k = 0; k < 4; k++) sel25519(p[k], q[k], b);
        ed_add(q, p);
        ed_add(p, p);
        for (int k = 0; k < 4; k++) sel25519(p[k], q[k], b);
    }
}

static void ed_scalarbase(gf p[4], const uint8_t* s) {
    gf q[4];
    set25519(q[0], ed_x);
    set25519(q[1], ed_y);
    set25519(q[2], gf1);
    fmul(q[3], ed_x, ed_y);
    ed_scalarmult(p, q, s);
}

/// The order of the base point, little-endian.
static const int64_t ed_l[32] = {0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2,
                                 0xde, 0xf9, 0xde, 0x14, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x10};

/// r = x mod L, for a 64-byte little-endian x.
static void ed_mod_l(uint8_t* r, int64_t x[64]) {
    int64_t carry;
    for (int i = 63; i >= 32; --i) {
        carry = 0;
        int j;
        for (j = i - 32; j < i - 12; ++j) {
            x[j] += carry - 16 * x[i] * ed_l[j - (i - 32)];
            carry = (x[j] + 128) >> 8;
            x[j] -= carry * 256;
        }
        x[j] += carry;
        x[i] = 0;
    }
    carry = 0;
    for (int j = 0; j < 32; j++) {
        x[j] += carry - (x[31] >> 4) * ed_l[j];
        carry = x[j] >> 8;
        x[j] &= 255;
    }
    for (int j = 0; j < 32; j++) x[j] -= carry * ed_l[j];
    for (int i = 0; i < 32; i++) {
        x[i + 1] += x[i] >> 8;
        r[i] = x[i] & 255;
    }
}

/// Decodes point p and negates it. Returns false if it isn't on the curve.
static bool ed_unpack_neg(gf r[4], const uint8_t p[32]) {
    gf t, chk, num, den, den2, den4, den6;
    set25519(r[2], gf1);
    unpack25519(r[1], p);
    fsq(num, r[1]);
    fmul(den, num, ed_d);
    fsub(num, num, r[2]);
    fadd(den, r[2], den);
    fsq(den2, den);
    fsq(den4, den2);
    fmul(den6, den4, den2);
    fmul(t, den6, num);
    fmul(t, t, den);
    pow2523(t, t);
    fmul(t, t, num);
    fmul(t, t, den);
    fmul(t, t, den);
    fmul(r[0], t, den);
    fsq(chk, r[0]);
    fmul(chk, chk, den);
    if (!eq25519(chk, num)) fmul(r[0], r[0], sqrtm1);
    fsq(chk, r[0]);
    fmul(chk, chk, den);
    if (!eq25519(chk, num)) return false;
    if (par25519(r[0]) == (p[31] >> 7)) fsub(r[0], gf0, r[0]);
    fmul(r[3], r[0], r[1]);
    return true;
}

int ed25519_verify(const uint8_t pk[32], const void* msg, size_t len, const uint8_t sig[64]) {
    gf p[4], q[4];
    if (!ed_unpack_neg(q, pk)) return false;
    // h = SHA-512(R || A || msg) mod L
    uint8_t* m = alloc(64 + len);
    if (!m) return false;
    memcpy(m, sig, 32);
    memcpy(m + 32, pk, 32);
    memcpy(m + 64, msg, len);
    uint8_t h[64];
    long hashed = sha512(m, 64 + len, h);
    free(m);
    if (hashed != 0) return false;
    int64_t x[64];
    for (int i = 0; i < 64; i++) x[i] = h[i];
    ed_mod_l(h, x);
    // Check [S]B - [h]A == R.
    ed_scalarmult(p, q, h);
    ed_scalarbase(q, sig + 32);
    ed_add(p, q);
    uint8_t t[32];
    ed_pack(t, p);
    return memcmp(sig, t, 32) == 0;
}

/// The secret scalar (first half, clamped) and nonce prefix (second half)
/// for `seed` (RFC 8032 5.1.5).
static void ed_expand(uint8_t d[64], const uint8_t seed[32]) {
    sha512(seed, 32, d);
    d[0] &= 248;
    d[31] &= 127;
    d[31] |= 64;
}

void ed25519_public_key(uint8_t pk[32], const uint8_t seed[32]) {
    uint8_t d[64];
    gf p[4];
    ed_expand(d, seed);
    ed_scalarbase(p, d);
    ed_pack(pk, p);
    memset(d, 0, sizeof d);
}

/// x = SHA-512(a || b || msg) mod L, a and b 32 bytes each (b may be NULL).
static long ed_hash_mod_l(uint8_t x[64], const uint8_t* a, const uint8_t* b, const void* msg, size_t len) {
    size_t head = b ? 64 : 32;
    uint8_t* m = alloc(head + len);
    if (!m) return -1;
    memcpy(m, a, 32);
    if (b) memcpy(m + 32, b, 32);
    memcpy(m + head, msg, len);
    long hashed = sha512(m, head + len, x);
    memset(m, 0, head + len);
    free(m);
    if (hashed != 0) return -1;
    int64_t t[64];
    for (int i = 0; i < 64; i++) t[i] = x[i];
    ed_mod_l(x, t);
    return 0;
}

long ed25519_sign(uint8_t sig[64], const uint8_t seed[32], const void* msg, size_t len) {
    uint8_t d[64], pk[32], r[64], h[64];
    gf p[4];
    ed_expand(d, seed);
    ed_scalarbase(p, d);
    ed_pack(pk, p);
    // r = H(prefix || msg), R = [r]B, h = H(R || A || msg), S = r + h * a.
    long ok = ed_hash_mod_l(r, d + 32, NULL, msg, len);
    if (ok == 0) {
        ed_scalarbase(p, r);
        ed_pack(sig, p);
        ok = ed_hash_mod_l(h, sig, pk, msg, len);
    }
    if (ok == 0) {
        int64_t x[64] = {0};
        for (int i = 0; i < 32; i++) x[i] = r[i];
        for (int i = 0; i < 32; i++)
            for (int j = 0; j < 32; j++) x[i + j] += (int64_t)h[i] * d[j];
        ed_mod_l(sig + 32, x);
    }
    memset(d, 0, sizeof d);
    memset(r, 0, sizeof r);
    return ok;
}

// ---- base64 -------------------------------------------------------------------

static const char base64_digits[] = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

size_t base64_encode(char* out, const void* data, size_t len) {
    const uint8_t* d = data;
    size_t o = 0;
    for (size_t i = 0; i < len; i += 3) {
        uint32_t v = (uint32_t)d[i] << 16;
        if (i + 1 < len) v |= (uint32_t)d[i + 1] << 8;
        if (i + 2 < len) v |= d[i + 2];
        out[o++] = base64_digits[v >> 18];
        out[o++] = base64_digits[(v >> 12) & 63];
        out[o++] = i + 1 < len ? base64_digits[(v >> 6) & 63] : '=';
        out[o++] = i + 2 < len ? base64_digits[v & 63] : '=';
    }
    out[o] = '\0';
    return o;
}

long base64_decode(uint8_t* out, const char* text, size_t len) {
    uint32_t v = 0;
    int bits = 0;
    long o = 0;
    for (size_t i = 0; i < len; i++) {
        char c = text[i];
        if (c == '=' || c == ' ' || c == '\n' || c == '\r' || c == '\t') continue;
        int n = c >= 'A' && c <= 'Z' ? c - 'A'
              : c >= 'a' && c <= 'z' ? c - 'a' + 26
              : c >= '0' && c <= '9' ? c - '0' + 52
              : c == '+' ? 62 : c == '/' ? 63 : -1;
        if (n < 0) return -1;
        v = v << 6 | (uint32_t)n;
        bits += 6;
        if (bits >= 8) {
            bits -= 8;
            out[o++] = (uint8_t)(v >> bits);
        }
    }
    return o;
}
