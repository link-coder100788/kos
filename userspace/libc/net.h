#ifndef C_NET_H
#define C_NET_H

// The network: HTTP and HTTPS requests, ping, plain TCP connections and
// UDP sockets.
// Host names are looked up for you. Everything here waits until it's
// done (or has failed, after about 15 seconds); on failure it returns -1
// or NULL, and last_error() (sys.h) says why.

#include <stddef.h>
#include <stdint.h>

// ---- HTTP ----------------------------------------------------------------------
// Redirects are followed. HTTPS is encrypted but the server's certificate
// isn't checked yet. Bodies are cut off at 4 MiB.

typedef struct {
    int status;       // 200, 404, ...
    char* body;       // body_len bytes, with a NUL after them
    size_t body_len;
    char* headers;    // "Name: value\n" lines
} http_response_t;

// The fields of a request; a NULL method is GET, NULL headers none.
// headers holds extra "Name: value" lines, separated by \n.
typedef struct {
    const char* method;
    const char* url;
    const char* headers;
    const void* body;
    size_t body_len;
} http_request_t;

http_response_t* http_request(const http_request_t* request);
http_response_t* http_get(const char* url);
// A POST with a body of the given Content-Type (e.g. "application/json").
http_response_t* http_post(const char* url, const char* content_type, const void* body, size_t len);

// Copies header `name`'s value (case doesn't matter) into buf, NUL-
// terminated. Returns 1, or 0 if the response doesn't have it.
int http_header(const http_response_t* response, const char* name, char* buf, size_t max);

void http_free(http_response_t* response);

// ---- other things ------------------------------------------------------------------

// Pings host (a name or an IPv4 address). Returns the round trip in ms.
long ping(const char* host);

// This machine's IPv4 address ("10.0.2.15"), NUL-terminated. Returns its
// length, or -1 before the network is set up.
long net_address(char* buf, size_t max);

// ---- TCP ---------------------------------------------------------------------------
// A connection is a number. It's closed when the program ends, if it
// hasn't closed it itself.

// Connects to host:port. Returns the connection, or -1.
long tcp_connect(const char* host, int port);

// Sends len bytes. Returns len, or -1.
long tcp_send(long conn, const void* data, size_t len);
// Sends a NUL-terminated string.
long tcp_send_str(long conn, const char* s);

// Waits for data and reads up to max bytes. Returns how many, 0 once the
// other side has closed the connection (and everything's been read), or -1.
long tcp_recv(long conn, void* buf, size_t max);

// Like tcp_recv, but doesn't wait: returns TCP_NOTHING_YET if nothing has
// arrived. For programs that also have a window to keep drawing.
#define TCP_NOTHING_YET (-2)
long tcp_try_recv(long conn, void* buf, size_t max);

// Reads one line (up to \n, which isn't kept; a \r before it is dropped
// too) into buf, NUL-terminated. Returns its length, or -1 if the
// connection closed first. Lines longer than max - 1 are cut short.
long tcp_recv_line(long conn, char* buf, size_t max);

void tcp_close(long conn);

// ---- UDP ---------------------------------------------------------------------------
// A socket is a number, bound to one local port. Each send is one datagram
// of at most UDP_MAX_DATAGRAM bytes; datagrams can be lost or arrive out
// of order, as UDP allows. Up to 64 received datagrams wait to be read
// (more are dropped). A socket is closed when the program ends, if it
// hasn't closed it itself.

#define UDP_MAX_DATAGRAM 1472

// Where a datagram came from.
typedef struct {
    char host[16];    // "10.0.2.2"
    int port;
} udp_from_t;

// Opens a socket on local port (0 picks a free one). Returns it, or -1.
long udp_open(int port);

// Sends len bytes as one datagram to host:port. Returns 0, or -1.
long udp_send(long sock, const char* host, int port, const void* data, size_t len);
// Sends a NUL-terminated string.
long udp_send_str(long sock, const char* host, int port, const char* s);

// Waits for a datagram and reads it into buf, cut short at max bytes.
// Returns its length (which can be 0), or -1. from may be NULL.
long udp_recv(long sock, void* buf, size_t max, udp_from_t* from);

// Like udp_recv, but doesn't wait: returns UDP_NOTHING_YET if nothing has
// arrived.
#define UDP_NOTHING_YET (-2)
long udp_try_recv(long sock, void* buf, size_t max, udp_from_t* from);

void udp_close(long sock);

#endif //C_NET_H
