#include "../libc/sys.h"
#include "../libc/stdio.h"
#include "../libc/math.h"

#include <stdbool.h>
#include <stddef.h>

typedef struct {
    const char* stmt;
} math_args_t;

typedef struct {
    const char* stmt;
    size_t pos;
    bool error;
} parser_t;

static void skip_spaces(parser_t* p) {
    while (p->stmt[p->pos] == ' ' || p->stmt[p->pos] == '\t') {
        p->pos++;
    }
}

static bool match(parser_t* p, char c) {
    skip_spaces(p);
    if (p->stmt[p->pos] == c) {
        p->pos++;
        return true;
    }
    return false;
}

static double parse_expression(parser_t* p);

static double parse_primary(parser_t* p) {
    skip_spaces(p);
    if (match(p, '(')) {
        double value = parse_expression(p);
        if (!match(p, ')')) { p->error = true; }
        return value;
    }
    if (p->stmt[p->pos] == '+') {
        p->pos++;
        return parse_primary(p);
    }
    if (p->stmt[p->pos] == '-') {
        p->pos++;
        return -parse_primary(p);
    }
    if (((p->stmt[p->pos] >= '0') && (p->stmt[p->pos] <= '9')) || (p->stmt[p->pos] == '.')) {
        double value = 0.0;
        while (p->stmt[p->pos] >= '0' && p->stmt[p->pos] <= '9') {
            value = value * 10.0 + (p->stmt[p->pos] - '0');
            p->pos++;
        }
        if (p->stmt[p->pos] == '.') {
            p->pos++;
            double place = 0.1;
            while ((p->stmt[p->pos] >= '0') && (p->stmt[p->pos] <= '9')) {
                value += (p->stmt[p->pos] - '0') * place;
                place *= 0.1;
                p->pos++;
            }
        }
        return value;
    }
    if ((p->stmt[p->pos] == 'p') && (p->stmt[p->pos + 1] == 'i')) {
        p->pos += 2;
        return M_PI;
    }
    if (p->stmt[p->pos] == 'e') {
        p->pos++;
        return M_E;
    }
    if (((p->stmt[p->pos] >= 'a') && (p->stmt[p->pos] <= 'z')) || ((p->stmt[p->pos] >= 'A') && (p->stmt[p->pos] <= 'Z'))) {
        char name[8];
        size_t n = 0;
        while (((p->stmt[p->pos] >= 'a') && (p->stmt[p->pos] <= 'z')) || ((p->stmt[p->pos] >= 'A') && (p->stmt[p->pos] <= 'Z'))) {
            if (n < sizeof(name) - 1) {
                name[n++] = p->stmt[p->pos];
            }
            p->pos++;
        }
        name[n] = '\0';
        if (!match(p, '(')) {
            p->error = true;
            return 0.0;
        }
        double arg = parse_expression(p);
        if (!match(p, ')')) {
            p->error = true;
            return 0.0;
        }
        if (name[0] == 's' && name[1] == 'i' &&
            name[2] == 'n' && name[3] == '\0') {
            return sin(arg);
        }
        if (name[0] == 'c' && name[1] == 'o' &&
            name[2] == 's' && name[3] == '\0') {
            return cos(arg);
        }
        p->error = true;
        return 0.0;
    }
    p->error = true;
    return 0.0;
}

static double parse_power(parser_t* p) {
    double value = parse_primary(p);
    if (match(p, '^')) {
        double exponent = parse_power(p);
        double result = 1.0;
        if (exponent < 0.0) {
            p->error = true;
            return 0.0;
        }
        for (int i = 0; i < (int)exponent; i++) {
            result *= value;
        }
        return result;
    }
    return value;
}

static double parse_term(parser_t* p) {
    double value = parse_power(p);
    while (!p->error) {
        if (match(p, '*')) {
            value *= parse_power(p);
        } else if (match(p, '/')) {
            double divisor = parse_power(p);
            if (divisor == 0.0) {
                p->error = true;
                return 0.0;
            }
            value /= divisor;
        } else {
            break;
        }
    }
    return value;
}

static double parse_expression(parser_t* p) {
    double value = parse_term(p);
    while (!p->error) {
        if (match(p, '+')) {
            value += parse_term(p);
        } else if (match(p, '-')) {
            value -= parse_term(p);
        } else {
            break;
        }
    }
    return value;
}

static bool evaluate(const char* input, double* result) {
    parser_t p = { .stmt = input, .pos = 0, .error = false };
    *result = parse_expression(&p);
    skip_spaces(&p);
    return !p.error && p.stmt[p.pos] == '\0';
}

void _start(void) {
    char input[256];
    writeln("KOS expression evaluator");
    writeln("Supports + - * / ^, parentheses, pi, sin(), cos().");
    writeln("Angles use radians. Type an expression:");
    long len = readln(input, sizeof(input) - 1);
    if (len < 0 || len >= (long)sizeof(input)) {
        writeln("Input error.");
        exit();
    }
    input[len] = '\0';
    double result;
    if (!evaluate(input, &result)) {
        writeln("Invalid expression.");
        exit();
    }
    char output[64];
    int n = snprintf(output, sizeof(output), "%.10g", result);
    if (n > 0) {
        writeln(output);
    } else {
        writeln("Could not format result.");
    }
    exit();
}
