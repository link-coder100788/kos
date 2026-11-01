#include "../libc/sys.h"
#include "../libc/net.h"
#include <string.h>
#include <stdbool.h>

#define MAX_ARGS 128
#define MAX_HEADERS 32
#define ARG_BUFFER_SIZE 8192

typedef struct {
    const char* url;
    const char* method;
    const char* output_file;
    const char* data;
    const char* headers[MAX_HEADERS];
    int header_count;
    bool include_headers;
    bool head;
    bool follow_redirects;
    bool silent;
    bool verbose;
    bool help;
} curl_args_t;

static void print_help(void) {
    writeln("Usage: curl [options] URL");
    writeln("");
    writeln("Options:");
    writeln("  -X METHOD   Specify request method");
    writeln("  -d DATA     Send request body");
    writeln("  -H HEADER   Add request header (repeatable)");
    writeln("  -o FILE     Save response body to a file");
    writeln("  -i          Include response headers");
    writeln("  -I          Send HEAD request");
    writeln("  -L          Follow redirects");
    writeln("  -s          Silent mode");
    writeln("  -v          Verbose mode");
    writeln("  -h          Show this help");
}

static int parse_args(int argc, char** argv, curl_args_t* args) {
    *args = (curl_args_t){0};
    for (int i = 0; i < argc; i++) {
        const char* arg = argv[i];
        if (strcmp(arg, "-h") == 0 || strcmp(arg, "--help") == 0) {
            args->help = true;
        } else if (strcmp(arg, "-i") == 0) {
            args->include_headers = true;
        } else if (strcmp(arg, "-I") == 0) {
            args->head = true;
        } else if (strcmp(arg, "-L") == 0) {
            args->follow_redirects = true;
        } else if (strcmp(arg, "-s") == 0) {
            args->silent = true;
        } else if (strcmp(arg, "-v") == 0) {
            args->verbose = true;
        } else if (strcmp(arg, "-X") == 0 || strcmp(arg, "-d") == 0 || strcmp(arg, "-H") == 0 || strcmp(arg, "-o") == 0) {
            if (++i >= argc) {
                write("curl: missing value for ");
                writeln(arg);
                return -1;
            }
            if (strcmp(arg, "-X") == 0) {
                args->method = argv[i];
            } else if (strcmp(arg, "-d") == 0) {
                args->data = argv[i];
            } else if (strcmp(arg, "-o") == 0) {
                args->output_file = argv[i];
            } else {
                if (args->header_count >= MAX_HEADERS) {
                    writeln("curl: too many headers");
                    return -1;
                }
                args->headers[args->header_count++] = argv[i];
            }
        } else if (arg[0] == '-') {
            write("curl: unknown option: ");
            writeln(arg);
            return -1;
        } else if (!args->url) {
            args->url = arg;
        } else {
            writeln("curl: only one URL is supported");
            return -1;
        }
    }
    if (!args->help && !args->url) {
        writeln("curl: missing URL");
        return -1;
    }
    return 0;
}

static char* build_headers(const curl_args_t* args) {
    size_t length = 1;
    for (int i = 0; i < args->header_count; i++) {
        size_t n = strlen(args->headers[i]);
        if (n > (size_t)-1 - length - 1) {
            return NULL;
        }
        length += n + 1;
    }
    char* result = alloc(length);
    if (!result) {
        return NULL;
    }
    char* p = result;
    for (int i = 0; i < args->header_count; i++) {
        size_t n = strlen(args->headers[i]);
        for (size_t j = 0; j < n; j++) {
            p[j] = args->headers[i][j];
        }
        p += n;
        *p++ = '\n';
    }
    *p = '\0';
    return result;
}

static void print_response_headers(const http_response_t* response) {
    write("HTTP status: ");
    char digits[4];
    int status = response->status;
    int n = 0;
    if (status == 0) {
        digits[n++] = '0';
    } else if (status >= 100 && status <= 999) {
        digits[n++] = (char)('0' + status / 100);
        digits[n++] = (char)('0' + (status / 10) % 10);
        digits[n++] = (char)('0' + status % 10);
    } else {
        digits[n++] = '?';
    }
    digits[n] = '\0';
    writeln(digits);
    if (response->headers && response->headers[0]) {
        write(response->headers);
        size_t len = strlen(response->headers);
        if (len && response->headers[len - 1] != '\n') {
            write("\n");
        }
    }
    write("\n");
}

static int run_curl(const curl_args_t* args) {
    char* headers = build_headers(args);
    if (!headers) {
        writeln("curl: failed to allocate headers");
        return 1;
    }
    const char* method = args->method;
    if (args->head) {
        method = "HEAD";
    } else if (!method) {
        method = args->data ? "POST" : "GET";
    }
    if (args->verbose) {
        write("> ");
        write(method);
        write(" ");
        writeln(args->url);
        for (int i = 0; i < args->header_count; i++) {
            write("> ");
            writeln(args->headers[i]);
        }
        writeln(">");
    }
    http_request_t request = {
        .method = method,
        .url = args->url,
        .headers = headers,
        .body = args->data,
        .body_len = args->data ? strlen(args->data) : 0
    };
    http_response_t* response = http_request(&request);
    free(headers);
    if (!response) {
        if (!args->silent) {
            write("curl: request failed: ");
            writeln(last_error());
        }
        return 1;
    }
    if (args->verbose) {
        write("< status ");
        print_response_headers(response);
    }
    if (args->include_headers || args->head) {
        print_response_headers(response);
    }
    if (!args->head && response->body && response->body_len > 0) {
        if (args->output_file) {
            if (fs_write(args->output_file, response->body, response->body_len) < 0) {
                if (!args->silent) {
                    write("curl: could not write file: ");
                    writeln(last_error());
                }
                http_free(response);
                return 1;
            }
        } else {
            write_bytes((const uint8_t*)response->body, response->body_len);
        }
    }
    http_free(response);
    return 0;
}

void _start(void) {
    char arg_buffer[ARG_BUFFER_SIZE];
    char* argv[MAX_ARGS];
    long len = args(arg_buffer, sizeof(arg_buffer) - 1);
    if (len < 0) {
        writeln("curl: failed to read arguments");
        exit();
    }
    if (len >= (long)sizeof(arg_buffer)) {
        writeln("curl: arguments too long");
        exit();
    }
    arg_buffer[len] = '\0';
    int argc = get_argv(argv, MAX_ARGS);
    curl_args_t parsed;
    if (parse_args(argc, argv, &parsed) != 0) {
        exit();
    }
    if (parsed.help) {
        print_help();
        exit();
    }
    int result = run_curl(&parsed);
    (void)result;
    exit();
}
