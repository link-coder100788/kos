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
    exit(0);
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
            exit(0);
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
                exit(0);
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
            exit(0);
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
                exit(0);
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
            exit(0);
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
        exit(0);
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
            exit(0);
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
        exit(0);
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
            exit(0);
        }
        if (result == 1) {
            buf_free(&p);
            return;
        }
        writeln("Permission denied, please try again.");
    }
    printf("ssh: %s@%s: permission denied\n", user, host_name);
    exit(0);
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
            exit(0);
        }
        if (type == MSG_GLOBAL_REQUEST || type == MSG_EXT_INFO) {
            if (type == MSG_GLOBAL_REQUEST) handle_channel_packet(&p);
            continue;
        }
        printf("ssh: unexpected message %d opening a session\n", type);
        exit(0);
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
            exit(0);
        }
    }
    if (i >= argc) {
        print_help();
        exit(0);
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
        exit(0);
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
    exit(0);
}
