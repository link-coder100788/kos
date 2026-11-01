// ssh: log in to another computer over SSH 2, line by line.
//
//   ssh [-p port] user@host            a remote shell
//   ssh [-p port] user@host command    run one command and print its output
//
// One set of algorithms, which every OpenSSH server offers: key exchange
// curve25519-sha256, host key ssh-ed25519, cipher aes256-gcm@openssh.com.
// Logs in with a password (the "password" or "keyboard-interactive"
// method). Host keys are remembered in /.kos/ssh/known_hosts the first
// time, and a changed key is refused.
//
// The Terminal works a line at a time, so this does too: each line you type
// is sent with a newline, and what comes back is printed with the remote
// terminal's escape codes taken out. Full-screen programs (vim, top) don't
// work. Typing "~." on its own disconnects.

#include "../libc/sys.h"
#include "../libc/net.h"
#include "../libc/crypto.h"
#include "../libc/stdio.h"
#include "../libc/string.h"

#include <stdbool.h>
#include <stdint.h>

#define MAX_ARGS 64
#define INPUT_MAX 1024
#define KNOWN_HOSTS_DIR "/.kos/ssh"
#define KNOWN_HOSTS "/.kos/ssh/known_hosts"
#define VERSION "SSH-2.0-KOS_ssh_1.0"
/// The biggest packet either side may send (RFC 4253 says at least 35000).
#define MAX_PACKET 35000
/// Our receive window, and the most data per packet we ask for.
#define WINDOW (2 * 1024 * 1024)
#define MAX_DATA 32768

// Message numbers (RFC 4250).
enum {
    MSG_DISCONNECT = 1, MSG_IGNORE = 2, MSG_UNIMPLEMENTED = 3, MSG_DEBUG = 4,
    MSG_SERVICE_REQUEST = 5, MSG_SERVICE_ACCEPT = 6, MSG_EXT_INFO = 7,
    MSG_KEXINIT = 20, MSG_NEWKEYS = 21, MSG_KEX_ECDH_INIT = 30, MSG_KEX_ECDH_REPLY = 31,
    MSG_USERAUTH_REQUEST = 50, MSG_USERAUTH_FAILURE = 51, MSG_USERAUTH_SUCCESS = 52,
    MSG_USERAUTH_BANNER = 53, MSG_USERAUTH_INFO_REQUEST = 60, MSG_USERAUTH_INFO_RESPONSE = 61,
    MSG_GLOBAL_REQUEST = 80, MSG_REQUEST_SUCCESS = 81, MSG_REQUEST_FAILURE = 82,
    MSG_CHANNEL_OPEN = 90, MSG_CHANNEL_OPEN_CONFIRMATION = 91, MSG_CHANNEL_OPEN_FAILURE = 92,
    MSG_CHANNEL_WINDOW_ADJUST = 93, MSG_CHANNEL_DATA = 94, MSG_CHANNEL_EXTENDED_DATA = 95,
    MSG_CHANNEL_EOF = 96, MSG_CHANNEL_CLOSE = 97, MSG_CHANNEL_REQUEST = 98,
    MSG_CHANNEL_SUCCESS = 99, MSG_CHANNEL_FAILURE = 100,
};

static void fail(const char* what) {
    printf("ssh: %s\n", what);
    exit();
}

// ---- X25519 and Ed25519 ------------------------------------------------------------
// Curve25519 arithmetic in the style of TweetNaCl (public domain): a field
// element is 16 limbs of 16 bits. Used for the key exchange (X25519) and to
// check the server's signature (Ed25519). Only public data is checked with
// Ed25519 here; X25519 runs in constant time.

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

/// X25519 (RFC 7748): q = n * p, with p the base point (9) if NULL.
static void x25519(uint8_t q[32], const uint8_t n[32], const uint8_t* p) {
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

/// Whether sig is public key pk's Ed25519 signature (RFC 8032) of msg.
static bool ed25519_verify(const uint8_t pk[32], const uint8_t* msg, size_t len, const uint8_t sig[64]) {
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

// ---- end of X25519 and Ed25519 -------------------------------------------------

// ---- base64 --------------------------------------------------------------------------

/// Writes data as base64 (with padding) into out, NUL-terminated. out needs
/// 4 * ((len + 2) / 3) + 1 bytes.
static void base64(char* out, const uint8_t* data, size_t len) {
    static const char digits[] = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    size_t o = 0;
    for (size_t i = 0; i < len; i += 3) {
        uint32_t v = data[i] << 16;
        if (i + 1 < len) v |= data[i + 1] << 8;
        if (i + 2 < len) v |= data[i + 2];
        out[o++] = digits[v >> 18];
        out[o++] = digits[(v >> 12) & 63];
        out[o++] = i + 1 < len ? digits[(v >> 6) & 63] : '=';
        out[o++] = i + 2 < len ? digits[v & 63] : '=';
    }
    out[o] = '\0';
}

// ---- building and reading messages ------------------------------------------------------

typedef struct {
    uint8_t* data;
    size_t len;
    size_t cap;
} buf_t;

static void buf_reserve(buf_t* b, size_t more) {
    if (b->len + more <= b->cap) return;
    size_t cap = b->cap ? b->cap * 2 : 256;
    while (cap < b->len + more) cap *= 2;
    uint8_t* data = alloc(cap);
    if (!data) fail("out of memory");
    if (b->data) {
        memcpy(data, b->data, b->len);
        free(b->data);
    }
    b->data = data;
    b->cap = cap;
}

static void buf_free(buf_t* b) {
    free(b->data);
    *b = (buf_t){0};
}

static void put_bytes(buf_t* b, const void* data, size_t len) {
    buf_reserve(b, len);
    memcpy(b->data + b->len, data, len);
    b->len += len;
}

static void put_u8(buf_t* b, uint8_t v) {
    put_bytes(b, &v, 1);
}

static void put_u32(buf_t* b, uint32_t v) {
    uint8_t be[4] = {v >> 24, v >> 16, v >> 8, v};
    put_bytes(b, be, 4);
}

static void put_string(buf_t* b, const void* data, size_t len) {
    put_u32(b, (uint32_t)len);
    put_bytes(b, data, len);
}

static void put_cstr(buf_t* b, const char* s) {
    put_string(b, s, strlen(s));
}

/// An unsigned big-endian number as an mpint: no leading zeros, and a zero
/// byte in front if the top bit is set.
static void put_mpint(buf_t* b, const uint8_t* num, size_t len) {
    while (len > 0 && num[0] == 0) {
        num++;
        len--;
    }
    bool pad = len > 0 && (num[0] & 0x80);
    put_u32(b, (uint32_t)(len + pad));
    if (pad) put_u8(b, 0);
    put_bytes(b, num, len);
}

/// Reads a message: each get_ checks there's enough left, and once one
/// fails, `bad` stays set and the rest return zeros.
typedef struct {
    const uint8_t* data;
    size_t len;
    size_t pos;
    bool bad;
} reader_t;

static reader_t reader(const uint8_t* data, size_t len) {
    return (reader_t){data, len, 0, false};
}

static uint8_t get_u8(reader_t* r) {
    if (r->bad || r->pos + 1 > r->len) {
        r->bad = true;
        return 0;
    }
    return r->data[r->pos++];
}

static uint32_t get_u32(reader_t* r) {
    if (r->bad || r->pos + 4 > r->len) {
        r->bad = true;
        return 0;
    }
    const uint8_t* p = r->data + r->pos;
    r->pos += 4;
    return (uint32_t)p[0] << 24 | (uint32_t)p[1] << 16 | (uint32_t)p[2] << 8 | p[3];
}

/// A string's bytes (not NUL-terminated), and its length in *len.
static const uint8_t* get_string(reader_t* r, uint32_t* len) {
    uint32_t n = get_u32(r);
    if (r->bad || n > r->len - r->pos) {
        r->bad = true;
        *len = 0;
        return (const uint8_t*)"";
    }
    const uint8_t* s = r->data + r->pos;
    r->pos += n;
    *len = n;
    return s;
}

/// A string copied into out, NUL-terminated and cut short to fit.
static void get_cstr(reader_t* r, char* out, size_t max) {
    uint32_t n;
    const uint8_t* s = get_string(r, &n);
    if (n > max - 1) n = max - 1;
    memcpy(out, s, n);
    out[n] = '\0';
}

/// Whether comma-separated `list` (len bytes) contains `name`.
static bool list_has(const uint8_t* list, size_t len, const char* name) {
    size_t n = strlen(name);
    size_t i = 0;
    while (i <= len) {
        size_t j = i;
        while (j < len && list[j] != ',') j++;
        if (j - i == n && memcmp(list + i, name, n) == 0) return true;
        i = j + 1;
    }
    return false;
}

// ---- the connection --------------------------------------------------------------------

typedef struct {
    uint8_t key[32];
    uint8_t nonce[12];
} gcm_t;

static struct {
    long tcp;
    // Bytes read from the network and not used yet.
    uint8_t* in;
    size_t in_len, in_cap;
    bool encrypted;
    gcm_t send_key, recv_key;
    uint32_t send_seq, recv_seq;
    uint8_t session_id[32];
    bool have_session_id;
    char server_version[256];
    // The channel, once open.
    uint32_t remote_channel;
    uint64_t remote_window;
    uint32_t remote_max_packet;
    uint32_t local_window;
    bool remote_eof, closed;
    long exit_status;
} ssh = {.exit_status = -1};

/// Reads what the network has into ssh.in. With `wait`, waits for at
/// least one byte. Returns false once the connection has closed.
static bool pump(bool wait) {
    if (ssh.in_cap - ssh.in_len < 4096) {
        size_t cap = ssh.in_cap ? ssh.in_cap * 2 : 65536;
        uint8_t* in = alloc(cap);
        if (!in) fail("out of memory");
        if (ssh.in) {
            memcpy(in, ssh.in, ssh.in_len);
            free(ssh.in);
        }
        ssh.in = in;
        ssh.in_cap = cap;
    }
    long n = wait ? tcp_recv(ssh.tcp, ssh.in + ssh.in_len, ssh.in_cap - ssh.in_len)
                  : tcp_try_recv(ssh.tcp, ssh.in + ssh.in_len, ssh.in_cap - ssh.in_len);
    if (n == TCP_NOTHING_YET) return true;
    if (n <= 0) return false;
    ssh.in_len += n;
    return true;
}

/// Makes sure ssh.in holds `need` bytes. Without `wait`, returns false if
/// it doesn't yet; with it, fails if the connection closes first.
static bool have(size_t need, bool wait) {
    while (ssh.in_len < need) {
        size_t before = ssh.in_len;
        if (!pump(wait)) fail("the server closed the connection");
        if (!wait && ssh.in_len == before) return false;
    }
    return true;
}

static void consume(size_t n) {
    memmove(ssh.in, ssh.in + n, ssh.in_len - n);
    ssh.in_len -= n;
}

static void send_all(const void* data, size_t len) {
    if (tcp_send(ssh.tcp, data, len) < 0) fail("sending failed");
}

/// The next 12-byte nonce: the last 8 bytes count packets (RFC 5647).
static void next_nonce(uint8_t nonce[12]) {
    for (int i = 11; i >= 4; i--)
        if (++nonce[i] != 0) break;
}

static void send_packet(const buf_t* payload) {
    // padding_length, payload and padding fill whole blocks: 8 bytes plain
    // (counting the length field), 16 encrypted (not counting it).
    size_t block = ssh.encrypted ? 16 : 8;
    size_t counted = ssh.encrypted ? 1 + payload->len : 4 + 1 + payload->len;
    size_t pad = block - counted % block;
    if (pad < 4) pad += block;
    uint32_t packet_len = (uint32_t)(1 + payload->len + pad);
    buf_t p = {0};
    put_u32(&p, packet_len);
    put_u8(&p, (uint8_t)pad);
    put_bytes(&p, payload->data, payload->len);
    buf_reserve(&p, pad + AES_GCM_TAG_LEN);
    random_bytes(p.data + p.len, pad);
    p.len += pad;
    if (ssh.encrypted) {
        aead_t a = {ssh.send_key.key, ssh.send_key.nonce, p.data, 4, p.data + 4, packet_len, p.data + p.len};
        if (aes_gcm_seal(&a) < 0) fail("encryption failed");
        p.len += AES_GCM_TAG_LEN;
        next_nonce(ssh.send_key.nonce);
    }
    send_all(p.data, p.len);
    buf_free(&p);
    ssh.send_seq++;
}

/// Takes the next packet off the network into *out (its payload). Without
/// `wait`, returns false if a whole one hasn't arrived yet.
static bool recv_raw(buf_t* out, bool wait) {
    if (!have(4, wait)) return false;
    uint32_t packet_len = (uint32_t)ssh.in[0] << 24 | (uint32_t)ssh.in[1] << 16 | (uint32_t)ssh.in[2] << 8 | ssh.in[3];
    size_t block = ssh.encrypted ? 16 : 8;
    if (packet_len < 5 || packet_len > MAX_PACKET || (ssh.encrypted ? packet_len : packet_len + 4) % block != 0)
        fail("the server sent a garbled packet");
    size_t total = 4 + packet_len + (ssh.encrypted ? AES_GCM_TAG_LEN : 0);
    if (!have(total, wait)) return false;
    if (ssh.encrypted) {
        aead_t a = {ssh.recv_key.key, ssh.recv_key.nonce, ssh.in, 4, ssh.in + 4, packet_len, ssh.in + 4 + packet_len};
        if (aes_gcm_open(&a) < 0) fail("a packet failed its integrity check (corrupted or tampered with)");
        next_nonce(ssh.recv_key.nonce);
    }
    uint8_t pad = ssh.in[4];
    if (pad < 4 || pad > packet_len - 1) fail("the server sent a garbled packet");
    out->len = 0;
    put_bytes(out, ssh.in + 5, packet_len - 1 - pad);
    consume(total);
    ssh.recv_seq++;
    return true;
}

static void kex(const uint8_t* server_kexinit, size_t server_kexinit_len);
static void handle_channel_packet(const buf_t* p);

/// The next packet that isn't transport chatter: IGNORE, DEBUG and
/// UNIMPLEMENTED are dropped, DISCONNECT ends the program, and a KEXINIT
/// (the server re-keying) is dealt with here. Without `wait`, returns
/// false if none is waiting.
static bool recv_packet(buf_t* out, bool wait) {
    for (;;) {
        if (!recv_raw(out, wait)) return false;
        if (out->len == 0) fail("the server sent an empty packet");
        reader_t r = reader(out->data, out->len);
        switch (get_u8(&r)) {
        case MSG_IGNORE:
        case MSG_DEBUG:
        case MSG_UNIMPLEMENTED:
            continue;
        case MSG_DISCONNECT: {
            get_u32(&r);
            char why[256];
            get_cstr(&r, why, sizeof why);
            printf("ssh: the server disconnected: %s\n", why);
            exit();
        }
        case MSG_KEXINIT:
            if (ssh.have_session_id) {
                kex(out->data, out->len);
                continue;
            }
            return true;
        default:
            return true;
        }
    }
}

// ---- connecting: versions, key exchange, host key --------------------------------------

static const char* host_name;
static int host_port;

static void exchange_versions(void) {
    send_all(VERSION "\r\n", strlen(VERSION) + 2);
    // The server may send other lines first; its version line starts "SSH-".
    for (int lines = 0; lines < 50; lines++) {
        size_t n = 0;
        for (;;) {
            have(n + 1, true);
            if (ssh.in[n] == '\n') break;
            if (++n > 250) fail("the server's greeting is too long (is it an SSH server?)");
        }
        size_t len = n > 0 && ssh.in[n - 1] == '\r' ? n - 1 : n;
        bool is_version = len >= 4 && memcmp(ssh.in, "SSH-", 4) == 0;
        if (is_version) {
            memcpy(ssh.server_version, ssh.in, len);
            ssh.server_version[len] = '\0';
        }
        consume(n + 1);
        if (is_version) {
            if (strncmp(ssh.server_version, "SSH-2.0-", 8) != 0 && strncmp(ssh.server_version, "SSH-1.99-", 9) != 0) {
                printf("ssh: the server only speaks %s\n", ssh.server_version);
                exit();
            }
            return;
        }
    }
    fail("the server didn't say which SSH version it speaks");
}

/// Our KEXINIT payload.
static void make_kexinit(buf_t* b) {
    uint8_t cookie[16];
    random_bytes(cookie, sizeof cookie);
    put_u8(b, MSG_KEXINIT);
    put_bytes(b, cookie, sizeof cookie);
    put_cstr(b, "curve25519-sha256,curve25519-sha256@libssh.org");
    put_cstr(b, "ssh-ed25519");
    put_cstr(b, "aes256-gcm@openssh.com");
    put_cstr(b, "aes256-gcm@openssh.com");
    // GCM authenticates by itself, so the MAC isn't used; some list is needed.
    put_cstr(b, "hmac-sha2-256");
    put_cstr(b, "hmac-sha2-256");
    put_cstr(b, "none");
    put_cstr(b, "none");
    put_cstr(b, "");
    put_cstr(b, "");
    put_u8(b, 0);
    put_u32(b, 0);
}

/// Checks the server's KEXINIT offers what we need.
static void check_kexinit(const uint8_t* data, size_t len) {
    static const char* const needs[] = {"key exchange curve25519-sha256", "host key type ssh-ed25519",
                                        "cipher aes256-gcm@openssh.com", "cipher aes256-gcm@openssh.com"};
    reader_t r = reader(data, len);
    get_u8(&r);
    r.pos += 16;
    for (int i = 0; i < 4; i++) {
        uint32_t n;
        const uint8_t* list = get_string(&r, &n);
        bool ok = i == 0 ? list_has(list, n, "curve25519-sha256") || list_has(list, n, "curve25519-sha256@libssh.org")
            : i == 1     ? list_has(list, n, "ssh-ed25519")
                         : list_has(list, n, "aes256-gcm@openssh.com");
        if (!ok) {
            printf("ssh: the server doesn't offer %s; it offers: %.*s\n", needs[i], (int)n, list);
            exit();
        }
    }
    for (int i = 0; i < 2; i++) get_string(&r, &(uint32_t){0}); // MACs
    for (int i = 0; i < 2; i++) {
        uint32_t n;
        const uint8_t* list = get_string(&r, &n);
        if (!list_has(list, n, "none")) fail("the server insists on compression");
    }
    if (r.bad) fail("the server's KEXINIT is garbled");
}

/// SHA-256(K || H || letter || session_id): one of the keys (RFC 4253 7.2).
static void derive(uint8_t out[32], const buf_t* k_mpint, const uint8_t h[32], char letter) {
    buf_t b = {0};
    put_bytes(&b, k_mpint->data, k_mpint->len);
    put_bytes(&b, h, 32);
    put_u8(&b, (uint8_t)letter);
    put_bytes(&b, ssh.session_id, 32);
    sha256(b.data, b.len, out);
    buf_free(&b);
}

/// Checks the host key against known_hosts, asking the first time.
static void check_host_key(const uint8_t* blob, size_t blob_len) {
    uint8_t digest[32];
    sha256(blob, blob_len, digest);
    char fingerprint[48];
    base64(fingerprint, digest, 32);
    fingerprint[43] = '\0'; // OpenSSH leaves off the '='
    char key64[128];
    base64(key64, blob, blob_len);
    char who[300];
    snprintf(who, sizeof who, "%s:%d", host_name, host_port);

    size_t len = 0;
    char* known = fs_read_all(KNOWN_HOSTS, &len);
    for (char* line = known; line && *line;) {
        char* end = strchr(line, '\n');
        if (end) *end = '\0';
        char* sp = strchr(line, ' ');
        if (sp && (size_t)(sp - line) == strlen(who) && strncmp(line, who, sp - line) == 0) {
            char* key = strchr(sp + 1, ' ');
            if (key && strncmp(sp + 1, "ssh-ed25519", key - sp - 1) == 0) {
                if (strcmp(key + 1, key64) == 0) {
                    free(known);
                    return;
                }
                printf("ssh: WARNING: %s's host key has CHANGED!\n", who);
                printf("Someone could be pretending to be it. Its key is now %s ssh-ed25519 SHA256:%s.\n", who, fingerprint);
                printf("If you know why it changed, remove its line from %s and connect again.\n", KNOWN_HOSTS);
                exit();
            }
        }
        line = end ? end + 1 : line + strlen(line);
    }
    free(known);

    printf("The authenticity of host '%s' can't be established.\n", who);
    printf("ED25519 key fingerprint is SHA256:%s.\n", fingerprint);
    for (;;) {
        write("Are you sure you want to continue connecting (yes/no)? ");
        char answer[16];
        long n = readln(answer, sizeof answer - 1);
        answer[n < 0 ? 0 : n] = '\0';
        if (strcmp(answer, "yes") == 0) break;
        if (strcmp(answer, "no") == 0 || n <= 0) fail("host key not accepted");
        writeln("Please type 'yes' or 'no'.");
    }
    fs_mkdir("/.kos");
    fs_mkdir(KNOWN_HOSTS_DIR);
    char line[512];
    int n = snprintf(line, sizeof line, "%s ssh-ed25519 %s\n", who, key64);
    if (fs_append(KNOWN_HOSTS, line, n) < 0) printf("ssh: couldn't remember the key: %s\n", last_error());
    else printf("Added '%s' (ED25519) to %s.\n", who, KNOWN_HOSTS);
}

/// A key exchange: the first one, or a re-key the server started (its
/// KEXINIT given). Ends with both sides on new keys.
static void kex(const uint8_t* server_kexinit, size_t server_kexinit_len) {
    buf_t ours = {0}, theirs = {0}, p = {0};
    make_kexinit(&ours);
    send_packet(&ours);
    if (server_kexinit) {
        put_bytes(&theirs, server_kexinit, server_kexinit_len);
    } else {
        for (;;) {
            recv_packet(&theirs, true);
            if (theirs.data[0] == MSG_KEXINIT) break;
            printf("ssh: expected the server's KEXINIT, got message %d\n", theirs.data[0]);
            exit();
        }
    }
    check_kexinit(theirs.data, theirs.len);

    // Our ephemeral X25519 key.
    uint8_t secret[32], q_c[32];
    random_bytes(secret, sizeof secret);
    x25519(q_c, secret, NULL);
    buf_t init = {0};
    put_u8(&init, MSG_KEX_ECDH_INIT);
    put_string(&init, q_c, 32);
    send_packet(&init);
    buf_free(&init);

    // KEX_ECDH_REPLY: host key, the server's ephemeral key, its signature.
    for (;;) {
        recv_raw(&p, true);
        if (p.len > 0 && p.data[0] != MSG_IGNORE && p.data[0] != MSG_DEBUG) break;
    }
    if (p.data[0] != MSG_KEX_ECDH_REPLY) {
        printf("ssh: expected the key exchange reply, got message %d\n", p.data[0]);
        exit();
    }
    reader_t r = reader(p.data, p.len);
    get_u8(&r);
    uint32_t ks_len, qs_len, sig_len;
    const uint8_t* ks = get_string(&r, &ks_len);
    const uint8_t* qs = get_string(&r, &qs_len);
    const uint8_t* sig_blob = get_string(&r, &sig_len);
    reader_t kr = reader(ks, ks_len);
    char key_type[32];
    get_cstr(&kr, key_type, sizeof key_type);
    uint32_t pk_len;
    const uint8_t* pk = get_string(&kr, &pk_len);
    reader_t sr = reader(sig_blob, sig_len);
    char sig_type[32];
    get_cstr(&sr, sig_type, sizeof sig_type);
    uint32_t sig64_len;
    const uint8_t* sig = get_string(&sr, &sig64_len);
    if (r.bad || kr.bad || sr.bad || qs_len != 32 || pk_len != 32 || sig64_len != 64 ||
        strcmp(key_type, "ssh-ed25519") != 0 || strcmp(sig_type, "ssh-ed25519") != 0)
        fail("the server's key exchange reply is garbled");

    uint8_t shared[32];
    x25519(shared, secret, qs);
    memset(secret, 0, sizeof secret);
    uint8_t zero[32] = {0};
    if (memcmp(shared, zero, 32) == 0) fail("the server's key exchange key is bad");
    buf_t k = {0};
    put_mpint(&k, shared, 32);

    // The exchange hash H.
    buf_t hb = {0};
    put_cstr(&hb, VERSION);
    put_cstr(&hb, ssh.server_version);
    put_string(&hb, ours.data, ours.len);
    put_string(&hb, theirs.data, theirs.len);
    put_string(&hb, ks, ks_len);
    put_string(&hb, q_c, 32);
    put_string(&hb, qs, 32);
    put_bytes(&hb, k.data, k.len);
    uint8_t h[32];
    sha256(hb.data, hb.len, h);
    buf_free(&hb);

    if (!ed25519_verify(pk, h, 32, sig)) fail("the server's host key signature is wrong: someone may be in the middle");
    if (!ssh.have_session_id) {
        memcpy(ssh.session_id, h, 32);
        ssh.have_session_id = true;
        check_host_key(ks, ks_len);
    }

    uint8_t iv_cs[32], iv_sc[32];
    gcm_t send_key, recv_key;
    derive(iv_cs, &k, h, 'A');
    derive(iv_sc, &k, h, 'B');
    derive(send_key.key, &k, h, 'C');
    derive(recv_key.key, &k, h, 'D');
    memcpy(send_key.nonce, iv_cs, 12);
    memcpy(recv_key.nonce, iv_sc, 12);
    buf_free(&k);

    buf_t newkeys = {0};
    put_u8(&newkeys, MSG_NEWKEYS);
    send_packet(&newkeys);
    buf_free(&newkeys);
    ssh.send_key = send_key;
    for (;;) {
        recv_raw(&p, true);
        if (p.len > 0 && p.data[0] == MSG_NEWKEYS) break;
        if (p.len == 0 || (p.data[0] != MSG_IGNORE && p.data[0] != MSG_DEBUG)) fail("the server didn't finish the key exchange");
    }
    ssh.recv_key = recv_key;
    ssh.encrypted = true;
    buf_free(&ours);
    buf_free(&theirs);
    buf_free(&p);
}

// ---- logging in --------------------------------------------------------------------

/// Prints a banner or prompt text from the server.
static void print_text(const uint8_t* s, size_t len) {
    for (size_t i = 0; i < len; i++)
        if (s[i] != '\r') write_bytes(s + i, 1);
}

static void read_secret(const char* prompt, char* out, size_t max) {
    write(prompt);
    long n = readln(out, max - 1);
    out[n < 0 ? 0 : n] = '\0';
}

/// A USERAUTH reply. Returns 1 for success, 0 for failure (with the
/// methods the server still allows in `methods`), -1 for an INFO_REQUEST
/// (left in *p).
static int auth_reply(buf_t* p, char* methods, size_t max) {
    for (;;) {
        recv_packet(p, true);
        reader_t r = reader(p->data, p->len);
        uint8_t type = get_u8(&r);
        switch (type) {
        case MSG_USERAUTH_SUCCESS:
            return 1;
        case MSG_USERAUTH_FAILURE:
            get_cstr(&r, methods, max);
            return 0;
        case MSG_USERAUTH_BANNER: {
            uint32_t n;
            const uint8_t* text = get_string(&r, &n);
            print_text(text, n);
            continue;
        }
        case MSG_USERAUTH_INFO_REQUEST:
            return -1;
        case MSG_EXT_INFO:
            continue;
        default:
            printf("ssh: unexpected message %d while logging in\n", type);
            exit();
        }
    }
}

static void auth_request(const char* user, const char* method) {
    buf_t b = {0};
    put_u8(&b, MSG_USERAUTH_REQUEST);
    put_cstr(&b, user);
    put_cstr(&b, "ssh-connection");
    put_cstr(&b, method);
    if (strcmp(method, "keyboard-interactive") == 0) {
        put_cstr(&b, ""); // language
        put_cstr(&b, ""); // submethods
    }
    send_packet(&b);
    buf_free(&b);
}

static int try_password(const char* user, buf_t* p, char* methods, size_t max) {
    char password[256], prompt[300];
    snprintf(prompt, sizeof prompt, "%s@%s's password: ", user, host_name);
    read_secret(prompt, password, sizeof password);
    buf_t b = {0};
    put_u8(&b, MSG_USERAUTH_REQUEST);
    put_cstr(&b, user);
    put_cstr(&b, "ssh-connection");
    put_cstr(&b, "password");
    put_u8(&b, 0);
    put_cstr(&b, password);
    memset(password, 0, sizeof password);
    send_packet(&b);
    memset(b.data, 0, b.len);
    buf_free(&b);
    int result = auth_reply(p, methods, max);
    if (result == -1) {
        // In reply to "password", message 60 means the password has expired.
        writeln("ssh: the server says the password has expired; change it another way first");
        exit();
    }
    return result;
}

static int try_keyboard_interactive(const char* user, buf_t* p, char* methods, size_t max) {
    auth_request(user, "keyboard-interactive");
    for (;;) {
        int result = auth_reply(p, methods, max);
        if (result >= 0) return result;
        reader_t r = reader(p->data, p->len);
        get_u8(&r);
        uint32_t n;
        const uint8_t* name = get_string(&r, &n);
        if (n) { print_text(name, n); writeln(""); }
        const uint8_t* instruction = get_string(&r, &n);
        if (n) { print_text(instruction, n); writeln(""); }
        get_string(&r, &n); // language
        uint32_t count = get_u32(&r);
        if (r.bad || count > 16) fail("the server's login prompt is garbled");
        buf_t b = {0};
        put_u8(&b, MSG_USERAUTH_INFO_RESPONSE);
        put_u32(&b, count);
        for (uint32_t i = 0; i < count; i++) {
            char prompt[256], answer[256];
            get_cstr(&r, prompt, sizeof prompt);
            get_u8(&r); // echo: the Terminal shows what's typed either way
            read_secret(prompt, answer, sizeof answer);
            put_cstr(&b, answer);
            memset(answer, 0, sizeof answer);
        }
        send_packet(&b);
        memset(b.data, 0, b.len);
        buf_free(&b);
    }
}

static void log_in(const char* user) {
    buf_t p = {0};
    buf_t req = {0};
    put_u8(&req, MSG_SERVICE_REQUEST);
    put_cstr(&req, "ssh-userauth");
    send_packet(&req);
    buf_free(&req);
    for (;;) {
        recv_packet(&p, true);
        if (p.data[0] == MSG_SERVICE_ACCEPT) break;
        if (p.data[0] != MSG_EXT_INFO) fail("the server won't let us log in");
    }

    // "none" asks which methods the server takes.
    char methods[256] = "";
    auth_request(user, "none");
    if (auth_reply(&p, methods, sizeof methods) == 1) return;
    for (int attempt = 0; attempt < 3; attempt++) {
        int result;
        if (list_has((const uint8_t*)methods, strlen(methods), "password")) {
            result = try_password(user, &p, methods, sizeof methods);
        } else if (list_has((const uint8_t*)methods, strlen(methods), "keyboard-interactive")) {
            result = try_keyboard_interactive(user, &p, methods, sizeof methods);
        } else {
            printf("ssh: the server only takes: %s (KOS's ssh can only use a password)\n", methods);
            exit();
        }
        if (result == 1) {
            buf_free(&p);
            return;
        }
        writeln("Permission denied, please try again.");
    }
    printf("ssh: %s@%s: permission denied\n", user, host_name);
    exit();
}

// ---- the session -------------------------------------------------------------------

/// Escape codes are dropped from the output, across packets.
static enum { OUT_TEXT, OUT_ESC, OUT_CSI, OUT_OSC, OUT_OSC_ESC } out_state;

static void print_output(const uint8_t* data, size_t len) {
    uint8_t chunk[512];
    size_t n = 0;
    for (size_t i = 0; i < len; i++) {
        uint8_t c = data[i];
        switch (out_state) {
        case OUT_TEXT:
            if (c == 0x1b) out_state = OUT_ESC;
            else if (c == '\n' || c == '\t' || c == '\b' || (c >= 0x20 && c != 0x7f)) chunk[n++] = c;
            break;
        case OUT_ESC:
            out_state = c == '[' ? OUT_CSI : c == ']' ? OUT_OSC : OUT_TEXT;
            break;
        case OUT_CSI:
            if (c >= 0x40 && c <= 0x7e) out_state = OUT_TEXT;
            break;
        case OUT_OSC:
            if (c == 0x07) out_state = OUT_TEXT;
            else if (c == 0x1b) out_state = OUT_OSC_ESC;
            break;
        case OUT_OSC_ESC:
            out_state = c == '\\' ? OUT_TEXT : OUT_OSC;
            break;
        }
        if (n == sizeof chunk) {
            write_bytes(chunk, n);
            n = 0;
        }
    }
    if (n) write_bytes(chunk, n);
}

static void handle_channel_packet(const buf_t* p) {
    reader_t r = reader(p->data, p->len);
    uint8_t type = get_u8(&r);
    switch (type) {
    case MSG_CHANNEL_DATA:
    case MSG_CHANNEL_EXTENDED_DATA: {
        get_u32(&r);
        if (type == MSG_CHANNEL_EXTENDED_DATA) get_u32(&r);
        uint32_t n;
        const uint8_t* data = get_string(&r, &n);
        print_output(data, n);
        ssh.local_window -= n < ssh.local_window ? n : ssh.local_window;
        // Give the window back once half of it is used.
        if (ssh.local_window < WINDOW / 2) {
            buf_t b = {0};
            put_u8(&b, MSG_CHANNEL_WINDOW_ADJUST);
            put_u32(&b, ssh.remote_channel);
            put_u32(&b, WINDOW - ssh.local_window);
            send_packet(&b);
            buf_free(&b);
            ssh.local_window = WINDOW;
        }
        break;
    }
    case MSG_CHANNEL_WINDOW_ADJUST:
        get_u32(&r);
        ssh.remote_window += get_u32(&r);
        break;
    case MSG_CHANNEL_EOF:
        ssh.remote_eof = true;
        break;
    case MSG_CHANNEL_CLOSE:
        if (!ssh.closed) {
            buf_t b = {0};
            put_u8(&b, MSG_CHANNEL_CLOSE);
            put_u32(&b, ssh.remote_channel);
            send_packet(&b);
            buf_free(&b);
        }
        ssh.closed = true;
        break;
    case MSG_CHANNEL_REQUEST: {
        get_u32(&r);
        char name[64];
        get_cstr(&r, name, sizeof name);
        bool want_reply = get_u8(&r);
        if (strcmp(name, "exit-status") == 0) {
            ssh.exit_status = get_u32(&r);
        } else if (strcmp(name, "exit-signal") == 0) {
            char sig[32];
            get_cstr(&r, sig, sizeof sig);
            printf("ssh: the remote program was killed (SIG%s)\n", sig);
        }
        if (want_reply) {
            buf_t b = {0};
            put_u8(&b, MSG_CHANNEL_FAILURE);
            put_u32(&b, ssh.remote_channel);
            send_packet(&b);
            buf_free(&b);
        }
        break;
    }
    case MSG_GLOBAL_REQUEST: {
        char name[64];
        get_cstr(&r, name, sizeof name);
        if (get_u8(&r)) {
            buf_t b = {0};
            put_u8(&b, MSG_REQUEST_FAILURE);
            send_packet(&b);
            buf_free(&b);
        }
        break;
    }
    case MSG_CHANNEL_SUCCESS:
    case MSG_CHANNEL_FAILURE:
    case MSG_REQUEST_SUCCESS:
    case MSG_REQUEST_FAILURE:
        break;
    default: {
        // Anything else: tell the server we don't know it.
        buf_t b = {0};
        put_u8(&b, MSG_UNIMPLEMENTED);
        put_u32(&b, ssh.recv_seq - 1);
        send_packet(&b);
        buf_free(&b);
    }
    }
}

/// Handles packets until none has arrived for `quiet_ms` (or, before any
/// has, for `first_ms`), or the channel closes.
static void drain(uint64_t first_ms, uint64_t quiet_ms) {
    buf_t p = {0};
    uint64_t start = uptime_ms(), last = 0;
    while (!ssh.closed) {
        if (recv_packet(&p, false)) {
            handle_channel_packet(&p);
            last = uptime_ms();
            continue;
        }
        uint64_t now = uptime_ms();
        if (last ? now - last >= quiet_ms : now - start >= first_ms) break;
        sleep_ms(10);
    }
    buf_free(&p);
}

static void open_channel(void) {
    buf_t b = {0}, p = {0};
    put_u8(&b, MSG_CHANNEL_OPEN);
    put_cstr(&b, "session");
    put_u32(&b, 0); // our channel number
    put_u32(&b, WINDOW);
    put_u32(&b, MAX_DATA);
    send_packet(&b);
    buf_free(&b);
    for (;;) {
        recv_packet(&p, true);
        reader_t r = reader(p.data, p.len);
        uint8_t type = get_u8(&r);
        if (type == MSG_CHANNEL_OPEN_CONFIRMATION) {
            get_u32(&r);
            ssh.remote_channel = get_u32(&r);
            ssh.remote_window = get_u32(&r);
            ssh.remote_max_packet = get_u32(&r);
            ssh.local_window = WINDOW;
            break;
        }
        if (type == MSG_CHANNEL_OPEN_FAILURE) {
            get_u32(&r);
            get_u32(&r);
            char why[256];
            get_cstr(&r, why, sizeof why);
            printf("ssh: the server won't open a session: %s\n", why);
            exit();
        }
        if (type == MSG_GLOBAL_REQUEST || type == MSG_EXT_INFO) {
            if (type == MSG_GLOBAL_REQUEST) handle_channel_packet(&p);
            continue;
        }
        printf("ssh: unexpected message %d opening a session\n", type);
        exit();
    }
    buf_free(&p);
}

/// Sends a channel request and waits for the answer. Returns whether the
/// server agreed.
static bool channel_request(buf_t* b) {
    send_packet(b);
    buf_t p = {0};
    for (;;) {
        recv_packet(&p, true);
        uint8_t type = p.data[0];
        if (type == MSG_CHANNEL_SUCCESS || type == MSG_CHANNEL_FAILURE) {
            buf_free(&p);
            return type == MSG_CHANNEL_SUCCESS;
        }
        handle_channel_packet(&p);
        if (ssh.closed) fail("the server closed the session");
    }
}

static void start_request(buf_t* b, const char* name) {
    put_u8(b, MSG_CHANNEL_REQUEST);
    put_u32(b, ssh.remote_channel);
    put_cstr(b, name);
    put_u8(b, 1); // want reply
}

static void send_data(const char* data, size_t len) {
    while (len > 0) {
        // Wait for the server to make room if it has none.
        while (ssh.remote_window == 0 && !ssh.closed) drain(50, 10);
        if (ssh.closed) return;
        size_t n = len;
        if (n > ssh.remote_window) n = ssh.remote_window;
        if (n > ssh.remote_max_packet) n = ssh.remote_max_packet;
        if (n > MAX_DATA) n = MAX_DATA;
        buf_t b = {0};
        put_u8(&b, MSG_CHANNEL_DATA);
        put_u32(&b, ssh.remote_channel);
        put_string(&b, data, n);
        send_packet(&b);
        buf_free(&b);
        ssh.remote_window -= n;
        data += n;
        len -= n;
    }
}

static void run_shell(void) {
    buf_t b = {0};
    start_request(&b, "pty-req");
    put_cstr(&b, "dumb");
    put_u32(&b, 80);
    put_u32(&b, 24);
    put_u32(&b, 0);
    put_u32(&b, 0);
    // Terminal modes: no echo (the Terminal already shows what's typed).
    uint8_t modes[] = {53, 0, 0, 0, 0, 0};
    put_string(&b, modes, sizeof modes);
    bool pty = channel_request(&b);
    buf_free(&b);
    start_request(&b, "shell");
    if (!channel_request(&b)) fail("the server won't start a shell");
    buf_free(&b);
    if (!pty) writeln("(no terminal on the server: there may be no prompt)");

    drain(2000, 300);
    char line[INPUT_MAX + 1];
    while (!ssh.closed) {
        long n = readln(line, INPUT_MAX);
        if (n < 0) break;
        if (n == 2 && line[0] == '~' && line[1] == '.') break;
        line[n] = '\n';
        send_data(line, n + 1);
        // Show what comes back: wait up to a second for it to start, and
        // until it's been quiet for a moment.
        drain(1000, 250);
    }
}

static void run_command(const char* command) {
    buf_t b = {0};
    start_request(&b, "exec");
    put_cstr(&b, command);
    if (!channel_request(&b)) fail("the server won't run the command");
    buf_free(&b);
    buf_t eof = {0};
    put_u8(&eof, MSG_CHANNEL_EOF);
    put_u32(&eof, ssh.remote_channel);
    send_packet(&eof);
    buf_free(&eof);
    buf_t p = {0};
    while (!ssh.closed) {
        recv_packet(&p, true);
        handle_channel_packet(&p);
    }
    buf_free(&p);
}

static void disconnect(void) {
    buf_t b = {0};
    put_u8(&b, MSG_DISCONNECT);
    put_u32(&b, 11); // by application
    put_cstr(&b, "bye");
    put_cstr(&b, "");
    send_packet(&b);
    buf_free(&b);
}

// ---- main ----------------------------------------------------------------------------

static void print_help(void) {
    writeln("Usage: ssh [-p port] user@host [command]");
    writeln("Log in to host over SSH with a password, or run one command there.");
    writeln("Lines you type go to the remote shell; \"~.\" on its own disconnects.");
    writeln("Host keys are remembered in " KNOWN_HOSTS ".");
}

static int parse_port(const char* s) {
    int port = 0;
    if (!*s) return -1;
    for (; *s; s++) {
        if (*s < '0' || *s > '9') return -1;
        port = port * 10 + (*s - '0');
        if (port > 65535) return -1;
    }
    return port > 0 ? port : -1;
}

void _start(void) {
    char* argv[MAX_ARGS];
    int argc = get_argv(argv, MAX_ARGS);
    int i = 0;
    host_port = 22;
    while (i < argc && argv[i][0] == '-') {
        if (strcmp(argv[i], "-p") == 0 && i + 1 < argc) {
            host_port = parse_port(argv[i + 1]);
            if (host_port < 0) fail("bad port");
            i += 2;
        } else {
            print_help();
            exit();
        }
    }
    if (i >= argc) {
        print_help();
        exit();
    }
    char* at = strrchr(argv[i], '@');
    if (!at || at == argv[i] || !at[1]) fail("expected user@host");
    *at = '\0';
    const char* user = argv[i];
    host_name = at + 1;
    i++;
    char command[INPUT_MAX] = "";
    for (; i < argc; i++) {
        if (command[0]) strlcpy(command + strlen(command), " ", sizeof command - strlen(command));
        strlcpy(command + strlen(command), argv[i], sizeof command - strlen(command));
    }

    ssh.tcp = tcp_connect(host_name, host_port);
    if (ssh.tcp < 0) {
        printf("ssh: can't connect to %s port %d: %s\n", host_name, host_port, last_error());
        exit();
    }
    exchange_versions();
    kex(NULL, 0);
    log_in(user);
    open_channel();
    if (command[0]) run_command(command);
    else run_shell();
    if (!ssh.closed) disconnect();
    tcp_close(ssh.tcp);
    if (command[0] && ssh.exit_status > 0) printf("ssh: the command exited with status %ld\n", ssh.exit_status);
    else if (!command[0]) printf("Connection to %s closed.\n", host_name);
    exit();
}
