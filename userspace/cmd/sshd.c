// sshd: an SSH 2 server, so another computer can log in to KOS.
//
//   sshd -passwd        set the login password (do this first, in the Terminal)
//   bg sshd [-p port]   serve on port (22 if not given) in the background
//
// Under QEMU, forward a port to it: `make run-gui FWD="tcp::2222-:22"`,
// then `ssh -p 2222 anything@localhost` on this computer. There are no
// user accounts yet, so any user name works with the password; it's kept
// as an Argon2id hash in /.kos/ssh/sshd_password. The host key is made the
// first time, in /.kos/ssh/sshd_host_key. A background program's output is
// thrown away, so sshd notes what it does in /.kos/ssh/sshd.log.
//
// One set of algorithms, the same as ssh's and offered by every OpenSSH
// client: key exchange curve25519-sha256, host key ssh-ed25519, cipher
// aes256-gcm@openssh.com.
//
// A session gets sshd's own shell (`help` lists its commands). Anything
// else in /bin (grep, wc, curl...) runs as a program, with its output sent
// to the client and what the client types as its input, until it ends;
// Ctrl+C ends it. `ssh host command` runs one command. One session at a
// time: anyone else connecting waits until it ends.

#include "../libc/sys.h"
#include "../libc/net.h"
#include "../libc/crypto.h"
#include "../libc/stdio.h"
#include "../libc/stdlib.h"
#include "../libc/string.h"
#include "../libc/time.h"
#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>

#define SSH_DIR "/.kos/ssh"
#define HOST_KEY SSH_DIR "/sshd_host_key"
#define PASSWORD_FILE SSH_DIR "/sshd_password"
#define LOG_FILE SSH_DIR "/sshd.log"
/// The log starts again once it's bigger than this.
#define LOG_MAX (256 * 1024)
#define VERSION "SSH-2.0-KOS_sshd_1.0"
#define DEFAULT_PORT 22
/// The biggest packet we take (RFC 4253 says at least 35000).
#define MAX_PACKET 65536
/// Our receive window, and the most data per packet we ask for.
#define WINDOW (1024 * 1024)
#define MAX_DATA 32768
/// How long a client gets to log in, and how many tries.
#define LOGIN_MS 60000
#define PASSWORD_TRIES 3
#define LINE_MAX 1024
#define HISTORY 32

// Message numbers (RFC 4250).
enum {
    MSG_DISCONNECT = 1, MSG_IGNORE = 2, MSG_UNIMPLEMENTED = 3, MSG_DEBUG = 4,
    MSG_SERVICE_REQUEST = 5, MSG_SERVICE_ACCEPT = 6,
    MSG_KEXINIT = 20, MSG_NEWKEYS = 21, MSG_KEX_ECDH_INIT = 30, MSG_KEX_ECDH_REPLY = 31,
    MSG_USERAUTH_REQUEST = 50, MSG_USERAUTH_FAILURE = 51, MSG_USERAUTH_SUCCESS = 52,
    MSG_GLOBAL_REQUEST = 80, MSG_REQUEST_SUCCESS = 81, MSG_REQUEST_FAILURE = 82,
    MSG_CHANNEL_OPEN = 90, MSG_CHANNEL_OPEN_CONFIRMATION = 91, MSG_CHANNEL_OPEN_FAILURE = 92,
    MSG_CHANNEL_WINDOW_ADJUST = 93, MSG_CHANNEL_DATA = 94, MSG_CHANNEL_EXTENDED_DATA = 95,
    MSG_CHANNEL_EOF = 96, MSG_CHANNEL_CLOSE = 97, MSG_CHANNEL_REQUEST = 98,
    MSG_CHANNEL_SUCCESS = 99, MSG_CHANNEL_FAILURE = 100,
};

/// DISCONNECT reasons (RFC 4253 11.1).
enum { DISCONNECT_PROTOCOL_ERROR = 2, DISCONNECT_KEY_EXCHANGE_FAILED = 3, DISCONNECT_BY_APPLICATION = 11,
       DISCONNECT_NO_MORE_AUTH_METHODS = 14 };

// ---- the log -------------------------------------------------------------------------

static void log_line(const char* fmt, ...) __attribute__((format(printf, 1, 2)));

static void log_line(const char* fmt, ...) {
    char line[512];
    time_t now = time(NULL);
    struct tm tm;
    size_t n = now > 0 ? strftime(line, sizeof line, "%Y-%m-%d %H:%M:%S ", localtime_r(&now, &tm)) : 0;
    va_list args;
    va_start(args, fmt);
    vsnprintf(line + n, sizeof line - n - 1, fmt, args);
    va_end(args);
    strcat(line, "\n");
    // Shown when started with `run`; `bg` throws it away.
    write(line);
    if (fs_size(LOG_FILE) > LOG_MAX) fs_remove(LOG_FILE);
    fs_append(LOG_FILE, line, strlen(line));
}

// ---- getting back to the accept loop ----------------------------------------------------
// Anything that ends a session (the client leaving, a bad packet, a network
// error) jumps straight back to the loop that waits for the next client.

/// x19-x30, sp, d8-d15: what a C function must find as it left them.
typedef uint64_t jump_point_t[21];

__attribute__((naked, returns_twice)) static int set_jump_point(jump_point_t p) {
    __asm volatile(
        "stp x19, x20, [x0, #0]\n"
        "stp x21, x22, [x0, #16]\n"
        "stp x23, x24, [x0, #32]\n"
        "stp x25, x26, [x0, #48]\n"
        "stp x27, x28, [x0, #64]\n"
        "stp x29, x30, [x0, #80]\n"
        "mov x9, sp\n"
        "str x9, [x0, #96]\n"
        "stp d8, d9, [x0, #104]\n"
        "stp d10, d11, [x0, #120]\n"
        "stp d12, d13, [x0, #136]\n"
        "stp d14, d15, [x0, #152]\n"
        "mov w0, #0\n"
        "ret\n");
}

/// Returns from set_jump_point(p) again, with 1.
__attribute__((naked, noreturn)) static void jump_to(jump_point_t p) {
    __asm volatile(
        "ldp x19, x20, [x0, #0]\n"
        "ldp x21, x22, [x0, #16]\n"
        "ldp x23, x24, [x0, #32]\n"
        "ldp x25, x26, [x0, #48]\n"
        "ldp x27, x28, [x0, #64]\n"
        "ldp x29, x30, [x0, #80]\n"
        "ldr x9, [x0, #96]\n"
        "mov sp, x9\n"
        "ldp d8, d9, [x0, #104]\n"
        "ldp d10, d11, [x0, #120]\n"
        "ldp d12, d13, [x0, #136]\n"
        "ldp d14, d15, [x0, #152]\n"
        "mov w0, #1\n"
        "ret\n");
}

static jump_point_t session_over;
static bool in_session;
/// Who's connected, for the log.
static char client[32];

/// Ends the session (or, outside one, sshd), noting why.
__attribute__((noreturn)) static void fail(const char* why) {
    if (!in_session) {
        log_line("sshd: %s", why);
        exit(0);
    }
    log_line("%s: %s", client, why);
    jump_to(session_over);
}

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

// ---- building and reading messages (as in ssh.c) -------------------------------------------

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

static uint8_t host_seed[32];
/// The host key as SSH sends it: "ssh-ed25519" and the public key.
static buf_t host_blob;
static char password_hash[256];

static struct {
    long tcp;
    // Bytes read from the network and not used yet.
    uint8_t* in;
    size_t in_len, in_cap;
    /// While logging in: when to give up (uptime_ms), else 0.
    uint64_t deadline;
    bool encrypted;
    gcm_t send_key, recv_key;
    uint8_t session_id[32];
    bool have_session_id;
    char client_version[256];
} ssh;

/// Makes room in ssh.in for more of what the network has.
static void grow_in(void) {
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
}

/// Reads more of what the network has into ssh.in. Fails once the client
/// has gone (or, while logging in, takes too long).
static void pump(void) {
    grow_in();
    for (;;) {
        long n;
        if (ssh.deadline) {
            if (uptime_ms() > ssh.deadline) fail("took too long to log in");
            n = tcp_try_recv(ssh.tcp, ssh.in + ssh.in_len, ssh.in_cap - ssh.in_len);
            if (n == TCP_NOTHING_YET) {
                sleep_ms(20);
                continue;
            }
        } else {
            n = tcp_recv(ssh.tcp, ssh.in + ssh.in_len, ssh.in_cap - ssh.in_len);
        }
        if (n <= 0) fail("disconnected");
        ssh.in_len += n;
        return;
    }
}

/// Takes whatever the network has right now into ssh.in, without waiting.
static void pump_now(void) {
    grow_in();
    long n = tcp_try_recv(ssh.tcp, ssh.in + ssh.in_len, ssh.in_cap - ssh.in_len);
    if (n == TCP_NOTHING_YET) return;
    if (n <= 0) fail("disconnected");
    ssh.in_len += n;
}

/// Whether ssh.in holds a whole packet (or a garbled one, which reading
/// it will complain about).
static bool packet_waiting(void) {
    if (ssh.in_len < 4) return false;
    uint32_t packet_len = (uint32_t)ssh.in[0] << 24 | (uint32_t)ssh.in[1] << 16 | (uint32_t)ssh.in[2] << 8 | ssh.in[3];
    if (packet_len > MAX_PACKET) return true;
    return ssh.in_len >= 4 + packet_len + (ssh.encrypted ? AES_GCM_TAG_LEN : 0);
}

/// Makes sure ssh.in holds `need` bytes.
static void have(size_t need) {
    while (ssh.in_len < need) pump();
}

static void consume(size_t n) {
    memmove(ssh.in, ssh.in + n, ssh.in_len - n);
    ssh.in_len -= n;
}

static void send_all(const void* data, size_t len) {
    if (tcp_send(ssh.tcp, data, len) < 0) fail("disconnected");
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
}

/// Tells the client why we're hanging up, then ends the session.
__attribute__((noreturn)) static void disconnect(uint32_t reason, const char* why) {
    buf_t b = {0};
    put_u8(&b, MSG_DISCONNECT);
    put_u32(&b, reason);
    put_cstr(&b, why);
    put_cstr(&b, "");
    send_packet(&b);
    buf_free(&b);
    fail(why);
}

/// Takes the next packet off the network into *out (its payload).
static void recv_raw(buf_t* out) {
    have(4);
    uint32_t packet_len = (uint32_t)ssh.in[0] << 24 | (uint32_t)ssh.in[1] << 16 | (uint32_t)ssh.in[2] << 8 | ssh.in[3];
    size_t block = ssh.encrypted ? 16 : 8;
    if (packet_len < 5 || packet_len > MAX_PACKET || (ssh.encrypted ? packet_len : packet_len + 4) % block != 0)
        fail("sent a garbled packet");
    size_t total = 4 + packet_len + (ssh.encrypted ? AES_GCM_TAG_LEN : 0);
    have(total);
    if (ssh.encrypted) {
        aead_t a = {ssh.recv_key.key, ssh.recv_key.nonce, ssh.in, 4, ssh.in + 4, packet_len, ssh.in + 4 + packet_len};
        if (aes_gcm_open(&a) < 0) fail("a packet failed its integrity check (corrupted or tampered with)");
        next_nonce(ssh.recv_key.nonce);
    }
    uint8_t pad = ssh.in[4];
    if (pad < 4 || pad > packet_len - 1) fail("sent a garbled packet");
    out->len = 0;
    put_bytes(out, ssh.in + 5, packet_len - 1 - pad);
    consume(total);
    if (out->len == 0) fail("sent an empty packet");
}

static void kex(const uint8_t* client_kexinit, size_t client_kexinit_len);

/// The next packet that isn't transport chatter: IGNORE, DEBUG and
/// UNIMPLEMENTED are dropped, DISCONNECT ends the session, and a KEXINIT
/// (the client re-keying) is dealt with here.
static void recv_packet(buf_t* out) {
    for (;;) {
        recv_raw(out);
        switch (out->data[0]) {
        case MSG_IGNORE:
        case MSG_DEBUG:
        case MSG_UNIMPLEMENTED:
            continue;
        case MSG_DISCONNECT:
            fail("disconnected");
        case MSG_KEXINIT:
            if (ssh.have_session_id) {
                kex(out->data, out->len);
                continue;
            }
            return;
        default:
            return;
        }
    }
}

// ---- connecting: versions and key exchange ---------------------------------------------

static void exchange_versions(void) {
    send_all(VERSION "\r\n", strlen(VERSION) + 2);
    size_t n = 0;
    for (;;) {
        have(n + 1);
        if (ssh.in[n] == '\n') break;
        if (++n > 250) fail("isn't an SSH client (its greeting is too long)");
    }
    size_t len = n > 0 && ssh.in[n - 1] == '\r' ? n - 1 : n;
    memcpy(ssh.client_version, ssh.in, len);
    ssh.client_version[len] = '\0';
    consume(n + 1);
    if (strncmp(ssh.client_version, "SSH-2.0-", 8) != 0) fail("isn't an SSH 2 client");
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

/// Checks the client's KEXINIT offers what we need.
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
            char why[128];
            snprintf(why, sizeof why, "the client doesn't offer %s", needs[i]);
            disconnect(DISCONNECT_KEY_EXCHANGE_FAILED, why);
        }
    }
    for (int i = 0; i < 2; i++) get_string(&r, &(uint32_t){0}); // MACs
    for (int i = 0; i < 2; i++) {
        uint32_t n;
        const uint8_t* list = get_string(&r, &n);
        if (!list_has(list, n, "none")) disconnect(DISCONNECT_KEY_EXCHANGE_FAILED, "compression isn't supported");
    }
    if (r.bad) fail("sent a garbled KEXINIT");
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

/// A key exchange: the first one, or a re-key the client started (its
/// KEXINIT given). Ends with both sides on new keys.
static void kex(const uint8_t* client_kexinit, size_t client_kexinit_len) {
    buf_t ours = {0}, theirs = {0}, p = {0};
    make_kexinit(&ours);
    send_packet(&ours);
    if (client_kexinit) {
        put_bytes(&theirs, client_kexinit, client_kexinit_len);
    } else {
        recv_packet(&theirs);
        if (theirs.data[0] != MSG_KEXINIT) fail("didn't start with a KEXINIT");
    }
    check_kexinit(theirs.data, theirs.len);

    // KEX_ECDH_INIT: the client's ephemeral key.
    for (;;) {
        recv_raw(&p);
        if (p.data[0] != MSG_IGNORE && p.data[0] != MSG_DEBUG) break;
    }
    if (p.data[0] != MSG_KEX_ECDH_INIT) fail("didn't send its key exchange key");
    reader_t r = reader(p.data, p.len);
    get_u8(&r);
    uint32_t qc_len;
    const uint8_t* qc_data = get_string(&r, &qc_len);
    if (r.bad || qc_len != 32) fail("sent a garbled key exchange key");
    uint8_t q_c[32];
    memcpy(q_c, qc_data, 32);

    // Ours, and the secret we now share.
    uint8_t secret[32], q_s[32], shared[32];
    if (random_bytes(secret, sizeof secret) < 0) fail("no random numbers");
    x25519(q_s, secret, NULL);
    x25519(shared, secret, q_c);
    memset(secret, 0, sizeof secret);
    uint8_t zero[32] = {0};
    if (memcmp(shared, zero, 32) == 0) fail("sent a bad key exchange key");
    buf_t k = {0};
    put_mpint(&k, shared, 32);
    memset(shared, 0, sizeof shared);

    // The exchange hash H, which the host key signs.
    buf_t hb = {0};
    put_cstr(&hb, ssh.client_version);
    put_cstr(&hb, VERSION);
    put_string(&hb, theirs.data, theirs.len);
    put_string(&hb, ours.data, ours.len);
    put_string(&hb, host_blob.data, host_blob.len);
    put_string(&hb, q_c, 32);
    put_string(&hb, q_s, 32);
    put_bytes(&hb, k.data, k.len);
    uint8_t h[32];
    sha256(hb.data, hb.len, h);
    buf_free(&hb);
    if (!ssh.have_session_id) {
        memcpy(ssh.session_id, h, 32);
        ssh.have_session_id = true;
    }
    uint8_t sig[64];
    if (ed25519_sign(sig, host_seed, h, 32) < 0) fail("out of memory");

    buf_t reply = {0}, sig_blob = {0};
    put_cstr(&sig_blob, "ssh-ed25519");
    put_string(&sig_blob, sig, 64);
    put_u8(&reply, MSG_KEX_ECDH_REPLY);
    put_string(&reply, host_blob.data, host_blob.len);
    put_string(&reply, q_s, 32);
    put_string(&reply, sig_blob.data, sig_blob.len);
    send_packet(&reply);
    buf_free(&reply);
    buf_free(&sig_blob);

    // We send with the server-to-client keys, and receive with the others.
    uint8_t iv_cs[32], iv_sc[32];
    gcm_t send_key, recv_key;
    derive(iv_cs, &k, h, 'A');
    derive(iv_sc, &k, h, 'B');
    derive(recv_key.key, &k, h, 'C');
    derive(send_key.key, &k, h, 'D');
    memcpy(recv_key.nonce, iv_cs, 12);
    memcpy(send_key.nonce, iv_sc, 12);
    buf_free(&k);

    buf_t newkeys = {0};
    put_u8(&newkeys, MSG_NEWKEYS);
    send_packet(&newkeys);
    buf_free(&newkeys);
    ssh.send_key = send_key;
    for (;;) {
        recv_raw(&p);
        if (p.data[0] == MSG_NEWKEYS) break;
        if (p.data[0] != MSG_IGNORE && p.data[0] != MSG_DEBUG) fail("didn't finish the key exchange");
    }
    ssh.recv_key = recv_key;
    ssh.encrypted = true;
    buf_free(&ours);
    buf_free(&theirs);
    buf_free(&p);
}

// ---- logging in --------------------------------------------------------------------

static char user[64];

static void auth_failure(void) {
    buf_t b = {0};
    put_u8(&b, MSG_USERAUTH_FAILURE);
    put_cstr(&b, "password");
    put_u8(&b, 0);
    send_packet(&b);
    buf_free(&b);
}

/// Waits for the client to ask for the user-auth service, then for it to
/// log in with the password.
static void log_in(void) {
    buf_t p = {0};
    recv_packet(&p);
    reader_t r = reader(p.data, p.len);
    char service[32];
    get_u8(&r);
    get_cstr(&r, service, sizeof service);
    if (p.data[0] != MSG_SERVICE_REQUEST || strcmp(service, "ssh-userauth") != 0)
        disconnect(DISCONNECT_PROTOCOL_ERROR, "expected a request to log in");
    buf_t accept = {0};
    put_u8(&accept, MSG_SERVICE_ACCEPT);
    put_cstr(&accept, "ssh-userauth");
    send_packet(&accept);
    buf_free(&accept);

    int tries = 0;
    for (int requests = 0; requests < 30; requests++) {
        recv_packet(&p);
        if (p.data[0] != MSG_USERAUTH_REQUEST) disconnect(DISCONNECT_PROTOCOL_ERROR, "expected a login request");
        r = reader(p.data, p.len);
        get_u8(&r);
        char method[32];
        get_cstr(&r, user, sizeof user);
        get_cstr(&r, service, sizeof service);
        get_cstr(&r, method, sizeof method);
        if (r.bad || strcmp(service, "ssh-connection") != 0) {
            auth_failure();
            continue;
        }
        // Other methods ("none", "publickey", ...) are told to use a password.
        if (strcmp(method, "password") != 0) {
            auth_failure();
            continue;
        }
        bool change = get_u8(&r);
        uint32_t len;
        const uint8_t* password = get_string(&r, &len);
        bool ok = !r.bad && !change && argon2id_verify((const char*)password, len, password_hash);
        // Wipe the password from the packet.
        memset(p.data, 0, p.len);
        if (ok) {
            buf_t b = {0};
            put_u8(&b, MSG_USERAUTH_SUCCESS);
            send_packet(&b);
            buf_free(&b);
            buf_free(&p);
            log_line("%s: logged in as %s", client, user);
            return;
        }
        log_line("%s: wrong password for %s", client, user);
        if (++tries >= PASSWORD_TRIES) disconnect(DISCONNECT_NO_MORE_AUTH_METHODS, "too many wrong passwords");
        auth_failure();
    }
    disconnect(DISCONNECT_NO_MORE_AUTH_METHODS, "too many login attempts");
}

// ---- the channel -----------------------------------------------------------------------
// One session channel. Its data arrives through handle_packet into `input`;
// output goes out through channel_send, within the client's window.

static struct {
    bool open;
    uint32_t remote;
    uint64_t remote_window;
    uint32_t remote_max_packet;
    /// How much the client may still send before we adjust the window.
    uint32_t local_window;
    bool pty;
    int cols;
    /// "shell" or "exec" has been asked for.
    bool started;
    char exec[LINE_MAX];
    bool remote_eof;
    // Typed bytes not handled yet.
    uint8_t* input;
    size_t input_len, input_cap;
} chan;

static void channel_simple(uint8_t msg) {
    buf_t b = {0};
    put_u8(&b, msg);
    put_u32(&b, chan.remote);
    send_packet(&b);
    buf_free(&b);
}

static void take_input(const uint8_t* data, size_t len) {
    if (chan.input_len + len > chan.input_cap) {
        size_t cap = chan.input_cap ? chan.input_cap * 2 : 4096;
        while (cap < chan.input_len + len) cap *= 2;
        uint8_t* in = alloc(cap);
        if (!in) fail("out of memory");
        if (chan.input) {
            memcpy(in, chan.input, chan.input_len);
            free(chan.input);
        }
        chan.input = in;
        chan.input_cap = cap;
    }
    memcpy(chan.input + chan.input_len, data, len);
    chan.input_len += len;
}

static void handle_channel_open(reader_t* r) {
    char type[32];
    get_cstr(r, type, sizeof type);
    uint32_t sender = get_u32(r);
    uint32_t window = get_u32(r);
    uint32_t max_packet = get_u32(r);
    buf_t b = {0};
    if (r->bad || strcmp(type, "session") != 0 || chan.open) {
        // Port forwarding and a second session aren't offered.
        put_u8(&b, MSG_CHANNEL_OPEN_FAILURE);
        put_u32(&b, sender);
        put_u32(&b, strcmp(type, "session") == 0 ? 1 : 3); // administratively prohibited / unknown type
        put_cstr(&b, strcmp(type, "session") == 0 ? "one session at a time" : "only sessions are offered");
        put_cstr(&b, "");
    } else {
        chan.open = true;
        chan.remote = sender;
        chan.remote_window = window;
        chan.remote_max_packet = max_packet ? max_packet : MAX_DATA;
        chan.local_window = WINDOW;
        put_u8(&b, MSG_CHANNEL_OPEN_CONFIRMATION);
        put_u32(&b, sender);
        put_u32(&b, 0);
        put_u32(&b, WINDOW);
        put_u32(&b, MAX_DATA);
    }
    send_packet(&b);
    buf_free(&b);
}

static void handle_channel_request(reader_t* r) {
    char name[64];
    get_cstr(r, name, sizeof name);
    bool want_reply = get_u8(r);
    bool ok = false;
    if (strcmp(name, "pty-req") == 0) {
        get_string(r, &(uint32_t){0}); // TERM
        chan.cols = (int)get_u32(r);
        chan.pty = ok = !r->bad;
    } else if (strcmp(name, "window-change") == 0) {
        chan.cols = (int)get_u32(r);
        ok = true;
    } else if (strcmp(name, "shell") == 0 && !chan.started) {
        chan.started = ok = true;
    } else if (strcmp(name, "exec") == 0 && !chan.started) {
        get_cstr(r, chan.exec, sizeof chan.exec);
        chan.started = ok = !r->bad && chan.exec[0];
    } else if (strcmp(name, "env") == 0) {
        ok = true; // noted, and ignored
    }
    if (want_reply) channel_simple(ok ? MSG_CHANNEL_SUCCESS : MSG_CHANNEL_FAILURE);
}

/// Deals with one packet from the client.
static void handle_packet(const buf_t* p) {
    reader_t r = reader(p->data, p->len);
    uint8_t msg = get_u8(&r);
    if (msg == MSG_GLOBAL_REQUEST) {
        get_string(&r, &(uint32_t){0});
        if (get_u8(&r)) {
            buf_t b = {0};
            put_u8(&b, MSG_REQUEST_FAILURE);
            send_packet(&b);
            buf_free(&b);
        }
        return;
    }
    if (msg == MSG_CHANNEL_OPEN) {
        handle_channel_open(&r);
        return;
    }
    if (msg < MSG_CHANNEL_WINDOW_ADJUST || msg > MSG_CHANNEL_FAILURE) {
        buf_t b = {0};
        put_u8(&b, MSG_UNIMPLEMENTED);
        put_u32(&b, 0);
        send_packet(&b);
        buf_free(&b);
        return;
    }
    get_u32(&r); // our channel: there's only the one
    if (!chan.open) fail("used a channel it hadn't opened");
    switch (msg) {
    case MSG_CHANNEL_WINDOW_ADJUST:
        chan.remote_window += get_u32(&r);
        break;
    case MSG_CHANNEL_DATA: {
        uint32_t len;
        const uint8_t* data = get_string(&r, &len);
        if (r.bad || len > chan.local_window) fail("sent more than its window");
        chan.local_window -= len;
        take_input(data, len);
        // Let it send more once it's used half.
        if (chan.local_window < WINDOW / 2) {
            buf_t b = {0};
            put_u8(&b, MSG_CHANNEL_WINDOW_ADJUST);
            put_u32(&b, chan.remote);
            put_u32(&b, WINDOW - chan.local_window);
            send_packet(&b);
            buf_free(&b);
            chan.local_window = WINDOW;
        }
        break;
    }
    case MSG_CHANNEL_EOF:
        chan.remote_eof = true;
        break;
    case MSG_CHANNEL_CLOSE:
        channel_simple(MSG_CHANNEL_CLOSE);
        fail("logged out");
    case MSG_CHANNEL_REQUEST:
        handle_channel_request(&r);
        break;
    default:
        break; // EXTENDED_DATA, SUCCESS, FAILURE: nothing to do
    }
}

/// Waits for the next packet from the client and deals with it.
static void wait_packet(void) {
    buf_t p = {0};
    recv_packet(&p);
    handle_packet(&p);
    buf_free(&p);
}

/// Sends data to the client (as stderr if `err`, without a terminal),
/// waiting while its window is full. With a terminal, \n becomes \r\n.
static void channel_send(const void* data, size_t len, bool err) {
    const uint8_t* d = data;
    buf_t converted = {0};
    if (chan.pty) {
        for (size_t i = 0; i < len; i++) {
            if (d[i] == '\n' && (i == 0 || d[i - 1] != '\r')) put_u8(&converted, '\r');
            put_u8(&converted, d[i]);
        }
        d = converted.data;
        len = converted.len;
    }
    while (len > 0) {
        while (chan.remote_window == 0) wait_packet();
        size_t n = len;
        if (n > chan.remote_window) n = chan.remote_window;
        if (n > chan.remote_max_packet) n = chan.remote_max_packet;
        if (n > MAX_DATA) n = MAX_DATA;
        buf_t b = {0};
        bool extended = err && !chan.pty;
        put_u8(&b, extended ? MSG_CHANNEL_EXTENDED_DATA : MSG_CHANNEL_DATA);
        put_u32(&b, chan.remote);
        if (extended) put_u32(&b, 1); // stderr
        put_string(&b, d, n);
        send_packet(&b);
        buf_free(&b);
        chan.remote_window -= n;
        d += n;
        len -= n;
    }
    buf_free(&converted);
}

/// Ends the session the polite way: the exit status, EOF and CLOSE.
__attribute__((noreturn)) static void finish(int status) {
    buf_t b = {0};
    put_u8(&b, MSG_CHANNEL_REQUEST);
    put_u32(&b, chan.remote);
    put_cstr(&b, "exit-status");
    put_u8(&b, 0);
    put_u32(&b, (uint32_t)status);
    send_packet(&b);
    buf_free(&b);
    channel_simple(MSG_CHANNEL_EOF);
    channel_simple(MSG_CHANNEL_CLOSE);
    // The client answers with its CLOSE (handle_packet then ends the session).
    for (;;) wait_packet();
}

// ---- output --------------------------------------------------------------------------
// Commands print through out/outf (which `> file` can capture) and err.

static bool capturing;
static buf_t captured;

static void out(const void* data, size_t len) {
    if (capturing) put_bytes(&captured, data, len);
    else channel_send(data, len, false);
}

static void outs(const char* s) {
    out(s, strlen(s));
}

static void vformat(buf_t* b, const char* fmt, va_list args) {
    char small[512];
    va_list again;
    va_copy(again, args);
    int n = vsnprintf(small, sizeof small, fmt, args);
    if (n < 0) n = 0;
    if ((size_t)n < sizeof small) {
        put_bytes(b, small, n);
    } else {
        buf_reserve(b, n + 1);
        vsnprintf((char*)b->data + b->len, n + 1, fmt, again);
        b->len += n;
    }
    va_end(again);
}

static void outf(const char* fmt, ...) __attribute__((format(printf, 1, 2)));
static void outf(const char* fmt, ...) {
    buf_t b = {0};
    va_list args;
    va_start(args, fmt);
    vformat(&b, fmt, args);
    va_end(args);
    out(b.data, b.len);
    buf_free(&b);
}

/// An error message: never captured.
static void err(const char* fmt, ...) __attribute__((format(printf, 1, 2)));
static void err(const char* fmt, ...) {
    buf_t b = {0};
    va_list args;
    va_start(args, fmt);
    vformat(&b, fmt, args);
    va_end(args);
    put_u8(&b, '\n');
    channel_send(b.data, b.len, true);
    buf_free(&b);
}

// ---- paths -------------------------------------------------------------------------

static char cwd[512] = "/";

/// `path` made absolute from cwd, with . and .. worked out, into out.
static void resolve(const char* path, char* out_path, size_t max) {
    char joined[1024];
    if (path[0] == '/') snprintf(joined, sizeof joined, "%s", path);
    else snprintf(joined, sizeof joined, "%s/%s", cwd, path);
    size_t n = 0;
    out_path[n++] = '/';
    char* save;
    for (char* part = strtok_r(joined, "/", &save); part; part = strtok_r(NULL, "/", &save)) {
        if (strcmp(part, ".") == 0) continue;
        if (strcmp(part, "..") == 0) {
            if (n > 1) {
                n--;
                while (n > 1 && out_path[n - 1] != '/') n--;
                if (n > 1) n--;
            }
            continue;
        }
        size_t len = strlen(part);
        if (n + len + 2 >= max) break;
        if (n > 1) out_path[n++] = '/';
        memcpy(out_path + n, part, len);
        n += len;
    }
    out_path[n] = '\0';
}

static bool is_dir(const char* path) {
    int count;
    fs_entry_t* entries = fs_list(path, &count);
    if (!entries) return false;
    free(entries);
    return true;
}

/// The last part of a path.
static const char* base_name(const char* path) {
    const char* slash = strrchr(path, '/');
    return slash ? slash + 1 : path;
}

// ---- commands ----------------------------------------------------------------------------

static int status;
static bool want_exit;

typedef int (*command_fn)(int argc, char** argv);

static int c_help(int argc, char** argv);

static int c_echo(int argc, char** argv) {
    for (int i = 1; i < argc; i++) {
        if (i > 1) outs(" ");
        outs(argv[i]);
    }
    outs("\n");
    return 0;
}

static int c_pwd(int argc, char** argv) {
    outf("%s\n", cwd);
    return 0;
}

static int c_cd(int argc, char** argv) {
    char path[512];
    resolve(argc > 1 ? argv[1] : "/", path, sizeof path);
    if (!is_dir(path)) {
        err("cd: %s: no such folder", argc > 1 ? argv[1] : "/");
        return 1;
    }
    strlcpy(cwd, path, sizeof cwd);
    return 0;
}

static int compare_entries(const void* a, const void* b) {
    const fs_entry_t* x = a;
    const fs_entry_t* y = b;
    return strcasecmp(x->name, y->name);
}

static int c_ls(int argc, char** argv) {
    bool all = false, long_form = false;
    int first = 1;
    for (; first < argc && argv[first][0] == '-' && argv[first][1]; first++) {
        for (char* f = argv[first] + 1; *f; f++) {
            if (*f == 'a') all = true;
            else if (*f == 'l') long_form = true;
            else {
                err("ls: unknown option -%c", *f);
                return 1;
            }
        }
    }
    int result = 0;
    int paths = argc - first;
    for (int i = first; i < argc || (paths == 0 && i == first); i++) {
        const char* arg = paths ? argv[i] : ".";
        char path[512];
        resolve(arg, path, sizeof path);
        int count;
        fs_entry_t* entries = fs_list(path, &count);
        if (!entries) {
            if (fs_size(path) >= 0) {
                if (long_form) outf("%10ld  %s\n", fs_size(path), arg);
                else outf("%s\n", arg);
            } else {
                err("ls: %s: not found", arg);
                result = 1;
            }
            continue;
        }
        if (paths > 1) outf("%s%s:\n", i > first ? "\n" : "", arg);
        qsort(entries, count, sizeof *entries, compare_entries);
        int width = 0, shown = 0;
        for (int e = 0; e < count; e++) {
            int w = (int)strlen(entries[e].name) + entries[e].is_dir;
            if (w > width) width = w;
        }
        int cols = chan.cols > 0 ? chan.cols : 80;
        int per_row = cols / (width + 2);
        if (per_row < 1) per_row = 1;
        for (int e = 0; e < count; e++) {
            fs_entry_t* entry = &entries[e];
            if (!all && entry->name[0] == '.') continue;
            if (long_form) {
                if (entry->is_dir) outf("%10s  %s/\n", "-", entry->name);
                else outf("%10u  %s\n", entry->size, entry->name);
                continue;
            }
            char name[260];
            snprintf(name, sizeof name, "%s%s", entry->name, entry->is_dir ? "/" : "");
            bool last_in_row = ++shown % per_row == 0;
            // In columns only on a terminal, like Unix's ls.
            if (capturing || !chan.pty) outf("%s\n", name);
            else outf("%-*s%s", width + 2, name, last_in_row ? "\n" : "");
        }
        if (!long_form && !capturing && chan.pty && shown % per_row != 0) outs("\n");
        free(entries);
    }
    return result;
}

static int c_cat(int argc, char** argv) {
    if (argc < 2) {
        err("usage: cat <file>...");
        return 1;
    }
    int result = 0;
    for (int i = 1; i < argc; i++) {
        char path[512];
        resolve(argv[i], path, sizeof path);
        size_t len;
        char* data = fs_read_all(path, &len);
        if (!data) {
            err("cat: %s: %s", argv[i], last_error());
            result = 1;
            continue;
        }
        out(data, len);
        free(data);
    }
    return result;
}

static int c_head(int argc, char** argv) {
    long lines = 10;
    int first = 1;
    if (argc > 2 && strcmp(argv[1], "-n") == 0) {
        lines = strtol(argv[2], NULL, 10);
        first = 3;
    }
    if (first >= argc) {
        err("usage: head [-n lines] <file>");
        return 1;
    }
    char path[512];
    resolve(argv[first], path, sizeof path);
    size_t len;
    char* data = fs_read_all(path, &len);
    if (!data) {
        err("head: %s: %s", argv[first], last_error());
        return 1;
    }
    size_t end = 0;
    for (long n = 0; end < len && n < lines; end++)
        if (data[end] == '\n') n++;
    out(data, end);
    free(data);
    return 0;
}

static int c_wc(int argc, char** argv) {
    if (argc < 2) {
        err("usage: wc <file>...");
        return 1;
    }
    int result = 0;
    for (int i = 1; i < argc; i++) {
        char path[512];
        resolve(argv[i], path, sizeof path);
        size_t len;
        char* data = fs_read_all(path, &len);
        if (!data) {
            err("wc: %s: %s", argv[i], last_error());
            result = 1;
            continue;
        }
        size_t lines = 0, words = 0;
        bool in_word = false;
        for (size_t j = 0; j < len; j++) {
            char c = data[j];
            if (c == '\n') lines++;
            bool space = c == ' ' || c == '\n' || c == '\t' || c == '\r';
            if (!space && !in_word) words++;
            in_word = !space;
        }
        outf("%7zu %7zu %7zu %s\n", lines, words, len, argv[i]);
        free(data);
    }
    return result;
}

static int c_grep(int argc, char** argv) {
    bool ignore_case = false, numbers = false;
    int first = 1;
    for (; first < argc && argv[first][0] == '-' && argv[first][1]; first++) {
        for (char* f = argv[first] + 1; *f; f++) {
            if (*f == 'i') ignore_case = true;
            else if (*f == 'n') numbers = true;
        }
    }
    if (argc - first < 2) {
        err("usage: grep [-i] [-n] <text> <file>...");
        return 1;
    }
    const char* pattern = argv[first];
    int files = argc - first - 1;
    int found = 0;
    for (int i = first + 1; i < argc; i++) {
        char path[512];
        resolve(argv[i], path, sizeof path);
        char* data = fs_read_all(path, NULL);
        if (!data) {
            err("grep: %s: %s", argv[i], last_error());
            continue;
        }
        int number = 0;
        for (char* line = data; line && *line;) {
            char* end = strchr(line, '\n');
            if (end) *end = '\0';
            number++;
            if (ignore_case ? strcasestr(line, pattern) != NULL : strstr(line, pattern) != NULL) {
                found++;
                if (files > 1) outf("%s:", argv[i]);
                if (numbers) outf("%d:", number);
                outf("%s\n", line);
            }
            line = end ? end + 1 : NULL;
        }
        free(data);
    }
    return found ? 0 : 1;
}

static int c_mkdir(int argc, char** argv) {
    int result = 0;
    for (int i = 1; i < argc; i++) {
        char path[512];
        resolve(argv[i], path, sizeof path);
        if (fs_mkdir(path) < 0) {
            err("mkdir: %s: %s", argv[i], last_error());
            result = 1;
        }
    }
    return result;
}

static int c_rm(int argc, char** argv) {
    int result = 0;
    for (int i = 1; i < argc; i++) {
        char path[512];
        resolve(argv[i], path, sizeof path);
        if (fs_remove(path) < 0) {
            err("rm: %s: %s", argv[i], last_error());
            result = 1;
        }
    }
    return result;
}

static int c_touch(int argc, char** argv) {
    int result = 0;
    for (int i = 1; i < argc; i++) {
        char path[512];
        resolve(argv[i], path, sizeof path);
        if (fs_size(path) < 0 && fs_write(path, "", 0) < 0) {
            err("touch: %s: %s", argv[i], last_error());
            result = 1;
        }
    }
    return result;
}

/// cp and mv: copies src to dst (or into it, if it's a folder), then with
/// `move` removes src.
static int copy(int argc, char** argv, bool move) {
    if (argc != 3) {
        err("usage: %s <from> <to>", argv[0]);
        return 1;
    }
    char src[512], dst[512];
    resolve(argv[1], src, sizeof src);
    resolve(argv[2], dst, sizeof dst);
    if (is_dir(dst)) {
        size_t n = strlen(dst);
        snprintf(dst + n, sizeof dst - n, "%s%s", n > 1 ? "/" : "", base_name(src));
    }
    size_t len;
    char* data = fs_read_all(src, &len);
    if (!data) {
        err("%s: %s: %s", argv[0], argv[1], last_error());
        return 1;
    }
    long written = fs_write(dst, data, len);
    free(data);
    if (written < 0) {
        err("%s: %s: %s", argv[0], argv[2], last_error());
        return 1;
    }
    if (move && fs_remove(src) < 0) {
        err("mv: couldn't remove %s: %s", argv[1], last_error());
        return 1;
    }
    return 0;
}

static int c_cp(int argc, char** argv) {
    return copy(argc, argv, false);
}

static int c_mv(int argc, char** argv) {
    return copy(argc, argv, true);
}

static int c_uname(int argc, char** argv) {
    utsname_t u;
    uname(&u);
    if (argc > 1 && strcmp(argv[1], "-a") == 0) outf("%s %s %s %s %s\n", u.sysname, u.nodename, u.release, u.version, u.machine);
    else outf("%s\n", u.sysname);
    return 0;
}

static int c_uptime(int argc, char** argv) {
    uint64_t s = uptime_ms() / 1000;
    outf("up %lu:%02lu:%02lu\n", (unsigned long)(s / 3600), (unsigned long)(s / 60 % 60), (unsigned long)(s % 60));
    return 0;
}

static int c_date(int argc, char** argv) {
    time_t now = time(NULL);
    if (now < 0) {
        err("date: there's no clock");
        return 1;
    }
    struct tm tm;
    char text[64];
    strftime(text, sizeof text, "%a %b %e %H:%M:%S %Y", localtime_r(&now, &tm));
    outf("%s\n", text);
    return 0;
}

static int c_whoami(int argc, char** argv) {
    outf("%s\n", user);
    return 0;
}

static int c_ip(int argc, char** argv) {
    char address[32];
    if (net_address(address, sizeof address) < 0) {
        err("ip: no address yet");
        return 1;
    }
    outf("%s\n", address);
    return 0;
}

static int c_ping(int argc, char** argv) {
    if (argc != 2) {
        err("usage: ping <host>");
        return 1;
    }
    long ms = ping(argv[1]);
    if (ms < 0) {
        err("ping: %s: %s", argv[1], last_error());
        return 1;
    }
    outf("reply from %s in %ld ms\n", argv[1], ms);
    return 0;
}

static int c_get(int argc, char** argv) {
    if (argc != 2) {
        err("usage: get <url>");
        return 1;
    }
    http_response_t* response = http_get(argv[1]);
    if (!response) {
        err("get: %s", last_error());
        return 1;
    }
    if (response->status >= 400) err("get: HTTP %d", response->status);
    out(response->body, response->body_len);
    int result = response->status >= 400;
    http_free(response);
    return result;
}

/// /bin and /programs hold programs as name.elf.b64.
static bool find_program(const char* name, char* path, size_t max) {
    if (strchr(name, '/')) {
        resolve(name, path, max);
        return fs_size(path) >= 0;
    }
    static const char* const dirs[] = {"/bin", "/programs"};
    for (int i = 0; i < 2; i++) {
        snprintf(path, max, "%s/%s.elf.b64", dirs[i], name);
        if (fs_size(path) >= 0) return true;
    }
    return false;
}

// Typed bytes not passed to the running program yet: with a terminal,
// we echo and gather a line, as the shell does.
static char typed[LINE_MAX];
static size_t typed_len;

/// Passes what the client has typed to a running program's input `*in`
/// (program `id`): a line at a time with a terminal, as it comes without.
/// Ctrl+C ends the program; Ctrl+D (or the client's EOF) ends its input.
static void feed_input(long* in, long id) {
    while (chan.input_len > 0) {
        size_t used = 1;
        uint8_t c = chan.input[0];
        if (!chan.pty) {
            used = chan.input_len;
            if (*in >= 0) fd_write(*in, chan.input, used);
        } else if (c == 0x03) {
            channel_send("^C\r\n", 4, false);
            proc_kill(id);
        } else if (c == 0x04) {
            if (*in >= 0) fd_close(*in);
            *in = -1;
        } else if (c == '\r' || c == '\n') {
            // \r\n is one Enter.
            if (c == '\r' && chan.input_len > 1 && chan.input[1] == '\n') used = 2;
            channel_send("\r\n", 2, false);
            typed[typed_len++] = '\n';
            if (*in >= 0) fd_write(*in, typed, typed_len);
            typed_len = 0;
        } else if (c == 0x7f || c == '\b') {
            if (typed_len > 0) {
                typed_len--;
                channel_send("\b \b", 3, false);
            }
        } else if (c >= 0x20 && typed_len < sizeof typed - 1) {
            typed[typed_len++] = (char)c;
            channel_send(&c, 1, false);
        }
        memmove(chan.input, chan.input + used, chan.input_len - used);
        chan.input_len -= used;
    }
    if (chan.remote_eof && *in >= 0) {
        fd_close(*in);
        *in = -1;
    }
}

/// Shows what's waiting in pipe `*fd` (stderr if `is_err`), closing it
/// once it's over. Returns whether there was anything.
static bool forward_output(long* fd, bool is_err) {
    if (*fd < 0) return false;
    char buf[4096];
    long n = fd_read_now(*fd, buf, sizeof buf);
    if (n == -2) return false;
    if (n <= 0) {
        fd_close(*fd);
        *fd = -1;
        return false;
    }
    if (is_err) channel_send(buf, n, true);
    else out(buf, n);
    return true;
}

/// Runs program `path` with `args` until it ends: what it prints goes to
/// the client (stdout through out(), so `> file` catches it), and what the
/// client types to it. Returns its exit status.
static int run_here(const char* name, const char* path, const char* args) {
    int in[2] = { -1, -1 }, outp[2] = { -1, -1 }, errp[2] = { -1, -1 };
    if (fd_pipe(in) < 0 || fd_pipe(outp) < 0 || fd_pipe(errp) < 0) {
        err("%s: %s", name, last_error());
        for (int i = 0; i < 2; i++) {
            if (in[i] >= 0) fd_close(in[i]);
            if (outp[i] >= 0) fd_close(outp[i]);
            if (errp[i] >= 0) fd_close(errp[i]);
        }
        return 1;
    }
    int fds[3] = { in[0], outp[1], errp[1] };
    long id = spawn_io(path, args, fds);
    // Only the program holds those ends now.
    fd_close(in[0]);
    fd_close(outp[1]);
    fd_close(errp[1]);
    long in_fd = in[1], out_fd = outp[0], err_fd = errp[0];
    if (id < 0) {
        err("%s: %s", name, last_error());
        fd_close(in_fd);
        fd_close(out_fd);
        fd_close(err_fd);
        return 126;
    }
    typed_len = 0;
    while (out_fd >= 0 || err_fd >= 0 || proc_running(id)) {
        bool busy = forward_output(&out_fd, false);
        busy |= forward_output(&err_fd, true);
        pump_now();
        while (packet_waiting()) {
            wait_packet();
            busy = true;
        }
        feed_input(&in_fd, id);
        if (!busy) sleep_ms(10);
    }
    if (in_fd >= 0) fd_close(in_fd);
    return (int)proc_wait(id);
}

/// argv[1...] as one string of arguments for a program, with relative
/// paths made absolute, since it doesn't know our folder. free() it.
static char* program_args(int argc, char** argv) {
    buf_t args = {0};
    for (int i = 1; i < argc; i++) {
        if (args.len) put_u8(&args, ' ');
        char abs[512];
        bool looks_like_path = argv[i][0] != '-' && argv[i][0] != '/' && (strchr(argv[i], '/') || strchr(argv[i], '.'));
        if (looks_like_path) {
            resolve(argv[i], abs, sizeof abs);
            if (fs_size(abs) >= 0 || is_dir(abs)) {
                put_bytes(&args, abs, strlen(abs));
                continue;
            }
        }
        put_bytes(&args, argv[i], strlen(argv[i]));
    }
    put_u8(&args, '\0');
    return (char*)args.data;
}

static int c_run(int argc, char** argv) {
    if (argc < 2) {
        err("usage: run <program> [args...]");
        return 1;
    }
    char path[512];
    if (!find_program(argv[1], path, sizeof path)) {
        err("run: %s: no such program", argv[1]);
        return 127;
    }
    char* args = program_args(argc - 1, argv + 1);
    int result = run_here(argv[1], path, args);
    free(args);
    return result;
}

static int c_kill(int argc, char** argv) {
    if (argc != 2) {
        err("usage: kill <id>");
        return 1;
    }
    if (proc_kill(strtol(argv[1], NULL, 10)) < 0) {
        err("kill: %s", last_error());
        return 1;
    }
    return 0;
}

static int c_clear(int argc, char** argv) {
    if (chan.pty) outs("\x1b[H\x1b[2J");
    return 0;
}

static int c_exit(int argc, char** argv) {
    want_exit = true;
    return argc > 1 ? (int)strtol(argv[1], NULL, 10) : status;
}

static const struct {
    const char* name;
    command_fn fn;
    const char* help;
} commands[] = {
    {"cat", c_cat, "cat <file>...           print files"},
    {"cd", c_cd, "cd [folder]             change folder"},
    {"clear", c_clear, "clear                   clear the screen"},
    {"cp", c_cp, "cp <from> <to>          copy a file"},
    {"date", c_date, "date                    the date and time"},
    {"echo", c_echo, "echo <text>...          print text"},
    {"exit", c_exit, "exit [status]           log out (also logout, Ctrl+D)"},
    {"get", c_get, "get <url>               fetch a web page and print it"},
    {"grep", c_grep, "grep [-i] [-n] <text> <file>...  lines containing text"},
    {"head", c_head, "head [-n lines] <file>  the start of a file"},
    {"help", c_help, "help                    this list"},
    {"ip", c_ip, "ip                      KOS's network address"},
    {"kill", c_kill, "kill <id>               end a program"},
    {"logout", c_exit, NULL},
    {"ls", c_ls, "ls [-a] [-l] [path]...  list files"},
    {"mkdir", c_mkdir, "mkdir <folder>...       make folders"},
    {"mv", c_mv, "mv <from> <to>          move or rename a file"},
    {"ping", c_ping, "ping <host>             ping a computer"},
    {"pwd", c_pwd, "pwd                     the current folder"},
    {"rm", c_rm, "rm <path>...            delete files or empty folders"},
    {"run", c_run, "run <program> [args]    run a program (or just type its name)"},
    {"touch", c_touch, "touch <file>...         make empty files"},
    {"uname", c_uname, "uname [-a]              the system's name"},
    {"uptime", c_uptime, "uptime                  how long KOS has been running"},
    {"wc", c_wc, "wc <file>...            count lines, words and bytes"},
    {"whoami", c_whoami, "whoami                  the name you logged in with"},
};

#define COMMAND_COUNT (sizeof commands / sizeof commands[0])

static int c_help(int argc, char** argv) {
    outs("sshd's commands (output can go to a file with > or >>; ; separates commands):\n");
    for (size_t i = 0; i < COMMAND_COUNT; i++)
        if (commands[i].help) outf("  %s\n", commands[i].help);
    outs("Other programs in /bin and /programs start with `run`, and print on the KOS console.\n");
    return 0;
}

// ---- running a line -----------------------------------------------------------------------

#define MAX_ARGS 64

/// Splits one command at spaces, with "double" and 'single' quotes and \
/// escapes, into argv (pointing into `words`). Returns the count, or -1.
static int split(const char* line, char* words, char** argv) {
    int argc = 0;
    char* w = words;
    const char* p = line;
    for (;;) {
        while (*p == ' ' || *p == '\t') p++;
        if (!*p) break;
        if (argc == MAX_ARGS - 1) return -1;
        argv[argc++] = w;
        char quote = 0;
        for (; *p && (quote || (*p != ' ' && *p != '\t')); p++) {
            if (quote) {
                if (*p == quote) quote = 0;
                else if (*p == '\\' && quote == '"' && p[1]) *w++ = *++p;
                else *w++ = *p;
            } else if (*p == '"' || *p == '\'') {
                quote = *p;
            } else if (*p == '\\' && p[1]) {
                *w++ = *++p;
            } else {
                *w++ = *p;
            }
        }
        if (quote) return -1;
        *w++ = '\0';
    }
    argv[argc] = NULL;
    return argc;
}

/// Runs one command (no ;), with any > or >> redirection.
static void run_command(const char* text) {
    char words[LINE_MAX * 2];
    char* argv[MAX_ARGS];
    int argc = split(text, words, argv);
    if (argc < 0) {
        err("sshd: unfinished quote, or too many words");
        status = 2;
        return;
    }
    if (argc == 0) return;
    // A trailing "> file", ">> file", ">file" or ">>file".
    const char* target = NULL;
    bool append = false;
    for (int i = 0; i < argc; i++) {
        if (argv[i][0] != '>') continue;
        append = argv[i][1] == '>';
        const char* name = argv[i] + 1 + append;
        int used = 1;
        if (!*name) {
            if (i + 1 >= argc) {
                err("sshd: > needs a file name");
                status = 2;
                return;
            }
            name = argv[i + 1];
            used = 2;
        }
        target = name;
        for (int j = i; j + used <= argc; j++) argv[j] = argv[j + used];
        argc -= used;
        break;
    }
    if (argc == 0) return;
    command_fn fn = NULL;
    for (size_t i = 0; i < COMMAND_COUNT; i++)
        if (strcmp(argv[0], commands[i].name) == 0) fn = commands[i].fn;
    char path[512];
    if (!fn && !find_program(argv[0], path, sizeof path)) {
        err("%s: command not found (try help)", argv[0]);
        status = 127;
        return;
    }
    if (target) {
        capturing = true;
        captured.len = 0;
    }
    if (fn) {
        status = fn(argc, argv);
    } else {
        char* args = program_args(argc, argv);
        status = run_here(argv[0], path, args);
        free(args);
    }
    if (target) {
        capturing = false;
        char path[512];
        resolve(target, path, sizeof path);
        long written = append ? fs_append(path, captured.data, captured.len) : fs_write(path, captured.data, captured.len);
        if (written < 0) {
            err("sshd: %s: %s", target, last_error());
            status = 1;
        }
    }
}

/// Runs a line of commands separated by ;.
static void run_line(const char* line) {
    char part[LINE_MAX];
    size_t n = 0;
    char quote = 0;
    for (const char* p = line;; p++) {
        if (*p == '\0' || (*p == ';' && !quote)) {
            part[n] = '\0';
            run_command(part);
            n = 0;
            if (*p == '\0' || want_exit) return;
            continue;
        }
        if (quote ? *p == quote : (*p == '"' || *p == '\'')) quote = quote ? 0 : *p;
        if (n < sizeof part - 1) part[n++] = *p;
    }
}

// ---- the shell ---------------------------------------------------------------------------

static struct {
    char line[LINE_MAX];
    size_t len;
    char* history[HISTORY];
    int history_count;
    /// Which history entry is shown while going through it (history_count = none).
    int browsing;
    /// 0 normally; 1 after ESC; 2 inside an escape sequence.
    int escape;
    bool skip_lf;
} ed;

static void prompt(void) {
    utsname_t u;
    uname(&u);
    outf("%s@%s:%s$ ", user, u.nodename, cwd);
}

static void redraw(void) {
    outs("\r\x1b[K");
    prompt();
    out(ed.line, ed.len);
}

static void remember(const char* line) {
    if (!*line) return;
    if (ed.history_count > 0 && strcmp(ed.history[ed.history_count - 1], line) == 0) return;
    if (ed.history_count == HISTORY) {
        free(ed.history[0]);
        memmove(ed.history, ed.history + 1, (HISTORY - 1) * sizeof ed.history[0]);
        ed.history_count--;
    }
    ed.history[ed.history_count++] = strdup(line);
}

static void show_history(int index) {
    if (index < 0 || index > ed.history_count) return;
    ed.browsing = index;
    const char* text = index < ed.history_count ? ed.history[index] : "";
    ed.len = strlcpy(ed.line, text, sizeof ed.line);
    if (ed.len >= sizeof ed.line) ed.len = sizeof ed.line - 1;
    redraw();
}

static void enter(void) {
    outs("\r\n");
    ed.line[ed.len] = '\0';
    remember(ed.line);
    run_line(ed.line);
    ed.len = 0;
    ed.browsing = ed.history_count;
    if (want_exit) finish(status);
    prompt();
}

/// One typed byte, with a terminal: we echo and edit the line.
static void key(uint8_t c) {
    if (ed.escape == 1) {
        ed.escape = (c == '[' || c == 'O') ? 2 : 0;
        return;
    }
    if (ed.escape == 2) {
        if (c >= 0x40 && c <= 0x7e) {
            ed.escape = 0;
            if (c == 'A') show_history(ed.browsing - 1);
            else if (c == 'B') show_history(ed.browsing + 1);
        }
        return;
    }
    bool after_cr = ed.skip_lf;
    ed.skip_lf = false;
    switch (c) {
    case '\r':
        ed.skip_lf = true;
        enter();
        break;
    case '\n':
        if (!after_cr) enter();
        break;
    case 0x7f:
    case '\b':
        if (ed.len > 0) {
            ed.len--;
            outs("\b \b");
        }
        break;
    case 0x03: // Ctrl+C
        outs("^C\r\n");
        ed.len = 0;
        ed.browsing = ed.history_count;
        prompt();
        break;
    case 0x04: // Ctrl+D
        if (ed.len == 0) {
            outs("logout\r\n");
            finish(status);
        }
        break;
    case 0x15: // Ctrl+U
        ed.len = 0;
        redraw();
        break;
    case 0x0c: // Ctrl+L
        outs("\x1b[H\x1b[2J");
        redraw();
        break;
    case 0x1b:
        ed.escape = 1;
        break;
    default:
        if (c >= 0x20 && ed.len < sizeof ed.line - 1) {
            ed.line[ed.len++] = (char)c;
            out(&c, 1);
        }
        break;
    }
}

/// The input without a terminal: whole lines, no echo or prompt.
static void lines(void) {
    for (;;) {
        uint8_t* nl = memchr(chan.input, '\n', chan.input_len);
        if (!nl) {
            if (chan.remote_eof) break;
            wait_packet();
            continue;
        }
        size_t n = nl - chan.input;
        if (n > 0 && chan.input[n - 1] == '\r') n--;
        if (n > LINE_MAX - 1) n = LINE_MAX - 1;
        char line[LINE_MAX];
        memcpy(line, chan.input, n);
        line[n] = '\0';
        size_t used = nl - chan.input + 1;
        memmove(chan.input, chan.input + used, chan.input_len - used);
        chan.input_len -= used;
        run_line(line);
        if (want_exit) finish(status);
    }
    // A last line without a newline.
    if (chan.input_len > 0) {
        char line[LINE_MAX];
        size_t n = chan.input_len < LINE_MAX - 1 ? chan.input_len : LINE_MAX - 1;
        memcpy(line, chan.input, n);
        line[n] = '\0';
        chan.input_len = 0;
        run_line(line);
    }
    finish(status);
}

static void shell(void) {
    if (!chan.pty) lines();
    utsname_t u;
    uname(&u);
    outf("Welcome to %s %s. Type help for the commands.\n", u.sysname, u.release);
    ed.browsing = ed.history_count;
    prompt();
    for (;;) {
        // Keys are taken one at a time, since a command can make more arrive.
        while (chan.input_len > 0) {
            uint8_t c = chan.input[0];
            memmove(chan.input, chan.input + 1, --chan.input_len);
            key(c);
        }
        if (chan.remote_eof) finish(status);
        wait_packet();
    }
}

// ---- a session -----------------------------------------------------------------------------

/// Everything from one client: logging in, then its session.
static void serve(void) {
    exchange_versions();
    kex(NULL, 0);
    log_in();
    ssh.deadline = 0;
    // Wait for it to open a session and ask for a shell or a command.
    while (!chan.started) wait_packet();
    if (chan.exec[0]) {
        log_line("%s: running %s", client, chan.exec);
        run_line(chan.exec);
        finish(status);
    }
    shell();
}

/// Forgets the last session (keeping the input buffer for the next).
static void reset(void) {
    uint8_t* in = ssh.in;
    size_t in_cap = ssh.in_cap;
    memset(&ssh, 0, sizeof ssh);
    ssh.in = in;
    ssh.in_cap = in_cap;
    free(chan.input);
    memset(&chan, 0, sizeof chan);
    for (int i = 0; i < ed.history_count; i++) free(ed.history[i]);
    memset(&ed, 0, sizeof ed);
    buf_free(&captured);
    capturing = false;
    want_exit = false;
    status = 0;
    user[0] = '\0';
    strcpy(cwd, "/");
}

// ---- starting up --------------------------------------------------------------------------

static void load_host_key(void) {
    fs_mkdir("/.kos");
    fs_mkdir(SSH_DIR);
    if (fs_read(HOST_KEY, host_seed, sizeof host_seed) != sizeof host_seed) {
        if (random_bytes(host_seed, sizeof host_seed) < 0) fail("can't make a host key: no random numbers (under ACCEL=hvf there are none yet)");
        if (fs_write(HOST_KEY, host_seed, sizeof host_seed) < 0) fail("can't save the host key");
        log_line("sshd: made a new host key");
    }
    uint8_t pk[32];
    ed25519_public_key(pk, host_seed);
    put_cstr(&host_blob, "ssh-ed25519");
    put_string(&host_blob, pk, 32);
    uint8_t digest[32];
    sha256(host_blob.data, host_blob.len, digest);
    char fingerprint[48];
    base64(fingerprint, digest, 32);
    fingerprint[43] = '\0'; // OpenSSH leaves off the '='
    log_line("sshd: host key ED25519 SHA256:%s", fingerprint);
}

static void set_password(void) {
    char first[256], second[256];
    write("New password: ");
    long n = readln(first, sizeof first - 1);
    first[n < 0 ? 0 : n] = '\0';
    write("Again: ");
    n = readln(second, sizeof second - 1);
    second[n < 0 ? 0 : n] = '\0';
    if (!first[0]) {
        writeln("sshd: no password given (run sshd -passwd in the Terminal, not with bg)");
        return;
    }
    if (strcmp(first, second) != 0) {
        writeln("sshd: the passwords don't match");
        return;
    }
    writeln("Hashing...");
    char hash[256];
    long len = argon2id_hash(first, strlen(first), hash, sizeof hash);
    memset(first, 0, sizeof first);
    memset(second, 0, sizeof second);
    fs_mkdir("/.kos");
    fs_mkdir(SSH_DIR);
    if (len < 0 || fs_write(PASSWORD_FILE, hash, len) < 0) {
        printf("sshd: couldn't save the password: %s\n", last_error());
        return;
    }
    writeln("Password set. Start the server with: bg sshd");
}

void _start(void) {
    char* argv[8];
    int argc = get_argv(argv, 8);
    int port = DEFAULT_PORT;
    if (argc == 1 && strcmp(argv[0], "-passwd") == 0) {
        set_password();
        exit(0);
    }
    if (argc == 2 && strcmp(argv[0], "-p") == 0) {
        port = atoi(argv[1]);
    } else if (argc != 0) {
        writeln("Usage: sshd -passwd        set the login password (in the Terminal)");
        writeln("       bg sshd [-p port]   serve SSH on port (22 if not given)");
        exit(0);
    }
    if (port < 1 || port > 65535) fail("bad port");
    long hash_len = fs_read(PASSWORD_FILE, password_hash, sizeof password_hash - 1);
    if (hash_len <= 0 || hash_len >= (long)sizeof password_hash) fail("no password set: run sshd -passwd in the Terminal first");
    password_hash[hash_len] = '\0';
    load_host_key();
    long listener = tcp_listen(port);
    if (listener < 0) {
        char why[160];
        snprintf(why, sizeof why, "can't listen on port %d: %s", port, last_error());
        fail(why);
    }
    log_line("sshd: listening on port %d (task %ld)", port, proc_self());
    for (;;) {
        tcp_peer_t from;
        long conn = tcp_accept(listener, &from);
        if (conn < 0) fail("accepting failed");
        reset();
        ssh.tcp = conn;
        ssh.deadline = uptime_ms() + LOGIN_MS;
        snprintf(client, sizeof client, "%s:%d", from.host, from.port);
        log_line("%s: connected", client);
        in_session = true;
        if (set_jump_point(session_over) == 0) serve();
        in_session = false;
        tcp_close(conn);
    }
}
