#include "net.h"
#include "string.h"
#include "ctype.h"
#include "sys.h"

#define SVC_NET5(sub, a0, a1, a2, a3, a4) ({                             \
    register uint64_t x0 __asm("x0") = (uint64_t)(a0);                  \
    register uint64_t x1 __asm("x1") = (uint64_t)(a1);                  \
    register uint64_t x2 __asm("x2") = (uint64_t)(a2);                  \
    register uint64_t x3 __asm("x3") = (uint64_t)(a3);                  \
    register uint64_t x4 __asm("x4") = (uint64_t)(a4);                  \
    register uint64_t x16 __asm("x16") = (uint64_t)(sub);               \
    __asm volatile("svc #0xc"                                            \
                   : "+r"(x0)                                           \
                   : "r"(x1), "r"(x2), "r"(x3), "r"(x4), "r"(x16)       \
                   : "memory");                                         \
    (long)x0;                                                            \
})
#define SVC_NET(sub, a0, a1, a2, a3) SVC_NET5(sub, a0, a1, a2, a3, 0)

enum { HTTP, HTTP_BODY, HTTP_HEADERS, PING, ADDRESS, TCP_CONNECT, TCP_SEND, TCP_RECV, TCP_CLOSE,
       UDP_OPEN, UDP_SEND, UDP_RECV, UDP_CLOSE, TCP_LISTEN, TCP_ACCEPT, TCP_LISTEN_CLOSE };

// The kernel's result for "nothing has arrived yet" (u64::MAX - 1).
#define KERNEL_NOTHING_YET (-2L)

// Copies out one part of the last response, as a NUL-terminated string.
static char* fetch_part(int sub, size_t* len) {
    long n = SVC_NET(sub, NULL, 0, 0, 0);
    if (n < 0) return NULL;
    char* buf = alloc(n + 1);
    if (!buf) return NULL;
    SVC_NET(sub, buf, n, 0, 0);
    buf[n] = '\0';
    if (len) *len = n;
    return buf;
}

http_response_t* http_request(const http_request_t* request) {
    long status = SVC_NET(HTTP, request, 0, 0, 0);
    if (status < 0) return NULL;
    http_response_t* r = alloc_z(sizeof(http_response_t));
    if (!r) return NULL;
    r->status = (int)status;
    r->body = fetch_part(HTTP_BODY, &r->body_len);
    r->headers = fetch_part(HTTP_HEADERS, NULL);
    if (!r->body || !r->headers) {
        http_free(r);
        return NULL;
    }
    return r;
}

http_response_t* http_get(const char* url) {
    http_request_t request = { "GET", url, NULL, NULL, 0 };
    return http_request(&request);
}

http_response_t* http_post(const char* url, const char* content_type, const void* body, size_t len) {
    char header[160] = "Content-Type: ";
    strlcpy(header + 14, content_type ? content_type : "application/octet-stream", sizeof(header) - 14);
    http_request_t request = { "POST", url, header, body, len };
    return http_request(&request);
}

int http_header(const http_response_t* response, const char* name, char* buf, size_t max) {
    size_t name_len = strlen(name);
    for (const char* line = response->headers; *line; ) {
        const char* end = strchr(line, '\n');
        if (!end) end = line + strlen(line);
        if (strncasecmp(line, name, name_len) == 0 && line[name_len] == ':') {
            const char* v = line + name_len + 1;
            while (v < end && *v == ' ') v++;
            size_t n = end - v;
            if (max > 0) {
                if (n > max - 1) n = max - 1;
                memcpy(buf, v, n);
                buf[n] = '\0';
            }
            return 1;
        }
        line = *end ? end + 1 : end;
    }
    return 0;
}

void http_free(http_response_t* response) {
    if (!response) return;
    free(response->body);
    free(response->headers);
    free(response);
}

long ping(const char* host) {
    return SVC_NET(PING, host, 0, 0, 0);
}

long net_address(char* buf, size_t max) {
    long n = SVC_NET(ADDRESS, buf, max, 0, 0);
    if (n < 0 || (size_t)n >= max) return -1;
    buf[n] = '\0';
    return n;
}

long tcp_connect(const char* host, int port) {
    return SVC_NET(TCP_CONNECT, host, port, 0, 0);
}

long tcp_send(long conn, const void* data, size_t len) {
    return SVC_NET(TCP_SEND, conn, data, len, 0);
}

long tcp_send_str(long conn, const char* s) {
    return tcp_send(conn, s, strlen(s));
}

long tcp_recv(long conn, void* buf, size_t max) {
    return SVC_NET(TCP_RECV, conn, buf, max, 0);
}

long tcp_try_recv(long conn, void* buf, size_t max) {
    long n = SVC_NET(TCP_RECV, conn, buf, max, 1);
    return n == KERNEL_NOTHING_YET ? TCP_NOTHING_YET : n;
}

long tcp_recv_line(long conn, char* buf, size_t max) {
    size_t n = 0;
    for (;;) {
        char c;
        long got = tcp_recv(conn, &c, 1);
        if (got <= 0) {
            if (n == 0) return -1;
            break;
        }
        if (c == '\n') break;
        if (n + 1 < max) buf[n++] = c;
    }
    if (n > 0 && buf[n - 1] == '\r') n--;
    if (max > 0) buf[n] = '\0';
    return (long)n;
}

void tcp_close(long conn) {
    SVC_NET(TCP_CLOSE, conn, 0, 0, 0);
}

static void format_from(const uint64_t raw[2], udp_from_t* from);

long tcp_listen(int port) {
    return SVC_NET(TCP_LISTEN, port, 0, 0, 0);
}

static long tcp_take(long listener, tcp_peer_t* from, int dont_wait) {
    uint64_t raw[2];
    long conn = SVC_NET(TCP_ACCEPT, listener, raw, dont_wait, 0);
    if (conn >= 0 && from) format_from(raw, from);
    return conn;
}

long tcp_accept(long listener, tcp_peer_t* from) {
    return tcp_take(listener, from, 0);
}

long tcp_try_accept(long listener, tcp_peer_t* from) {
    long conn = tcp_take(listener, from, 1);
    return conn == KERNEL_NOTHING_YET ? TCP_NOTHING_YET : conn;
}

void tcp_listen_close(long listener) {
    SVC_NET(TCP_LISTEN_CLOSE, listener, 0, 0, 0);
}

long udp_open(int port) {
    return SVC_NET(UDP_OPEN, port, 0, 0, 0);
}

long udp_send(long sock, const char* host, int port, const void* data, size_t len) {
    uint64_t to[2] = { (uint64_t)host, (uint64_t)port };
    return SVC_NET(UDP_SEND, sock, to, data, len);
}

long udp_send_str(long sock, const char* host, int port, const char* s) {
    return udp_send(sock, host, port, s, strlen(s));
}

// Writes the kernel's address (a big-endian IPv4 number) as text.
static void format_from(const uint64_t raw[2], udp_from_t* from) {
    char* p = from->host;
    for (int i = 3; i >= 0; i--) {
        unsigned octet = (raw[0] >> (i * 8)) & 0xff;
        if (octet >= 100) *p++ = (char)('0' + octet / 100);
        if (octet >= 10) *p++ = (char)('0' + octet / 10 % 10);
        *p++ = (char)('0' + octet % 10);
        if (i > 0) *p++ = '.';
    }
    *p = '\0';
    from->port = (int)raw[1];
}

static long udp_receive(long sock, void* buf, size_t max, udp_from_t* from, int dont_wait) {
    uint64_t raw[2];
    long n = SVC_NET5(UDP_RECV, sock, buf, max, raw, dont_wait);
    if (n >= 0 && from) format_from(raw, from);
    return n;
}

long udp_recv(long sock, void* buf, size_t max, udp_from_t* from) {
    return udp_receive(sock, buf, max, from, 0);
}

long udp_try_recv(long sock, void* buf, size_t max, udp_from_t* from) {
    long n = udp_receive(sock, buf, max, from, 1);
    return n == KERNEL_NOTHING_YET ? UDP_NOTHING_YET : n;
}

void udp_close(long sock) {
    SVC_NET(UDP_CLOSE, sock, 0, 0, 0);
}
