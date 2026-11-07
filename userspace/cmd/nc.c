#include "../libc/sys.h"
#include "../libc/net.h"
#include "../libc/stdio.h"

#include <stdbool.h>

#define MAX_ARGS 128
#define ARG_BUFFER_SIZE 8192
#define BUFFER_SIZE 1024

typedef struct {
    const char* host;
    int port;
    bool help;
    bool udp;
    bool listen;
} nc_args_t;

static int parse_port(const char* s) {
    int port = 0;
    if (!s || !*s) return -1;
    for (size_t i = 0; s[i]; i++) {
        if (s[i] < '0' || s[i] > '9') return -1;
        port = port * 10 + (s[i] - '0');
        if (port > 65535) return -1;
    }
    if (port < 1) return -1;
    return port;
}

static void print_help(void) {
    writeln("Usage: nc HOST PORT        connect to HOST on PORT using TCP");
    writeln("       nc -u HOST PORT     send to HOST on PORT using UDP");
    writeln("       nc -l PORT          wait for one TCP connection on PORT");
    writeln("       nc -u -l PORT       listen for UDP datagrams on PORT");
    writeln("Type lines to send them (each one a datagram with -u); received data is printed.");
}

static int parse_args(int argc, char** argv, nc_args_t* parsed) {
    *parsed = (nc_args_t){0};
    if (argc == 1 && argv[0][0] == '-') {
        if (argv[0][1] == 'h' || (argv[0][1] == '-' && argv[0][2] == 'h')) {
            parsed->help = true;
            return 0;
        }
    }
    while (argc > 0 && argv[0][0] == '-' && argv[0][1] && !argv[0][2]) {
        if (argv[0][1] == 'u') parsed->udp = true;
        else if (argv[0][1] == 'l') parsed->listen = true;
        else break;
        argc--;
        argv++;
    }
    if (parsed->listen) {
        if (argc != 1) {
            writeln("nc: expected PORT");
            return -1;
        }
        parsed->port = parse_port(argv[0]);
        if (parsed->port < 1) {
            writeln("nc: invalid port");
            return -1;
        }
        return 0;
    }
    if (argc != 2) {
        writeln("nc: expected HOST and PORT");
        return -1;
    }
    parsed->host = argv[0];
    parsed->port = parse_port(argv[1]);
    if (parsed->port < 1) {
        writeln("nc: invalid port");
        return -1;
    }
    return 0;
}

// Prints a datagram, and with `show_from` who sent it.
static void print_datagram(const char* data, long len, const udp_from_t* from, bool show_from) {
    if (show_from) printf("[%s:%d] ", from->host, from->port);
    write_bytes((const uint8_t*)data, (size_t)len);
    if (len == 0 || data[len - 1] != '\n') writeln("");
}

static int run_udp_listen(const nc_args_t* parsed) {
    long sock = udp_open(parsed->port);
    if (sock < 0) {
        write("nc: can't listen: ");
        writeln(last_error());
        return 1;
    }
    printf("Listening on UDP port %d\n", parsed->port);
    char received[UDP_MAX_DATAGRAM];
    for (;;) {
        udp_from_t from;
        long n = udp_recv(sock, received, sizeof(received), &from);
        if (n < 0) {
            write("nc: receive failed: ");
            writeln(last_error());
            break;
        }
        print_datagram(received, n, &from, true);
    }
    udp_close(sock);
    return 0;
}

static int run_udp(const nc_args_t* parsed) {
    long sock = udp_open(0);
    if (sock < 0) {
        write("nc: can't open a socket: ");
        writeln(last_error());
        return 1;
    }
    char input[BUFFER_SIZE];
    char received[UDP_MAX_DATAGRAM];
    for (;;) {
        long input_len = readln(input, sizeof(input) - 1);
        if (input_len < 0) break;
        input[input_len++] = '\n';
        if (udp_send(sock, parsed->host, parsed->port, input, (size_t)input_len) < 0) {
            write("nc: send failed: ");
            writeln(last_error());
            break;
        }
        // Give a reply a moment to arrive, then print whatever has.
        sleep_ms(200);
        udp_from_t from;
        long n;
        while ((n = udp_try_recv(sock, received, sizeof(received), &from)) >= 0) {
            print_datagram(received, n, &from, false);
        }
    }
    udp_close(sock);
    return 0;
}

// Prints what has arrived. Returns false once the connection is over.
static bool print_received(long conn) {
    char received[BUFFER_SIZE];
    for (;;) {
        long n = tcp_try_recv(conn, received, sizeof(received));
        if (n == TCP_NOTHING_YET) return true;
        if (n == 0) return false;
        if (n < 0) {
            write("nc: receive failed: ");
            writeln(last_error());
            return false;
        }
        write_bytes((const uint8_t*)received, (size_t)n);
    }
}

// Sends typed lines and prints what comes back, until either side is done.
static void talk(long conn) {
    char input[BUFFER_SIZE];
    // Give a greeting a moment to arrive.
    sleep_ms(200);
    while (print_received(conn)) {
        long input_len = readln(input, sizeof(input) - 1);
        if (input_len < 0) {
            // No more input: print whatever else is on its way, then stop.
            sleep_ms(200);
            print_received(conn);
            break;
        }
        input[input_len++] = '\n';
        if (tcp_send(conn, input, (size_t)input_len) < 0) {
            write("nc: send failed: ");
            writeln(last_error());
            break;
        }
        // Give a reply a moment to arrive.
        sleep_ms(200);
    }
    tcp_close(conn);
}

static int run_nc(const nc_args_t* parsed) {
    long conn = tcp_connect(parsed->host, parsed->port);
    if (conn < 0) {
        write("nc: connection failed: ");
        writeln(last_error());
        return 1;
    }
    printf("Connected to %s:%d\n", parsed->host, parsed->port);
    talk(conn);
    return 0;
}

static int run_tcp_listen(const nc_args_t* parsed) {
    long listener = tcp_listen(parsed->port);
    if (listener < 0) {
        write("nc: can't listen: ");
        writeln(last_error());
        return 1;
    }
    printf("Listening on TCP port %d\n", parsed->port);
    tcp_peer_t from;
    long conn = tcp_accept(listener, &from);
    // Only the one connection.
    tcp_listen_close(listener);
    if (conn < 0) {
        write("nc: accept failed: ");
        writeln(last_error());
        return 1;
    }
    printf("Connection from %s:%d\n", from.host, from.port);
    talk(conn);
    return 0;
}

void _start(void) {
    char arg_buffer[ARG_BUFFER_SIZE];
    char* argv[MAX_ARGS];
    long len = args(arg_buffer, sizeof(arg_buffer) - 1);
    if (len < 0 || len >= (long)sizeof(arg_buffer)) {
        writeln("nc: failed to read arguments");
        exit(2);
    }
    arg_buffer[len] = '\0';
    int argc = get_argv(argv, MAX_ARGS);
    nc_args_t parsed;
    if (parse_args(argc, argv, &parsed) != 0) {
        exit(2);
    }
    if (parsed.help) {
        print_help();
        exit(0);
    }
    if (parsed.listen && parsed.udp) (void)run_udp_listen(&parsed);
    else if (parsed.listen) (void)run_tcp_listen(&parsed);
    else if (parsed.udp) (void)run_udp(&parsed);
    else (void)run_nc(&parsed);
    exit(0);
}