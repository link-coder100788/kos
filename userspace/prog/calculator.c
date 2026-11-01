// A scientific calculator on a canvas. Type or click an expression; the
// result shows as you go. It knows + - * / and ^ (with the usual order),
// brackets, sin and cos (in radians), exp, ln, log (base 10), pi and e.
// A number next to a bracket, function or constant multiplies: 2pi.
//
// Keys: digits, + - * / ^ ( ) ., letters for names (sin, pi, ...), Enter
// or = for the result, Backspace deletes, Escape clears, q quits.
#include "../libc/sys.h"
#include "../libc/canvas.h"
#include "../libc/math.h"

#define W 280
#define H 494
#define DISPLAY_H 96
#define COLS 4
#define ROWS 7
#define GAP 6
#define BTN_W ((W - GAP * (COLS + 1)) / COLS)
#define BTN_H ((H - DISPLAY_H - GAP * (ROWS + 1)) / ROWS)
#define MAX_EXPR 120

static const char* labels[ROWS][COLS] = {
    {"sin", "cos", "^", "pi"},
    {"exp", "ln", "log", "e"},
    {"C", "(", ")", "<-"},
    {"7", "8", "9", "/"},
    {"4", "5", "6", "*"},
    {"1", "2", "3", "-"},
    {"0", ".", "=", "+"},
};

static char expr[MAX_EXPR + 1];
static int len;
// After =, the result replaces the expression; typing a digit then starts
// over, and typing an operator carries on from the result.
static int just_evaluated;
static char error[40];

// ---- evaluating ---------------------------------------------------------------
// A recursive-descent parser: each level handles one precedence, calling
// the next level for its operands. Lower in the list binds tighter.
//     expr    = term (('+' | '-') term)*
//     term    = unary (('*' | '/') unary | unary)*    (the last: 2pi, 3(4))
//     unary   = ('-' | '+') unary | power
//     power   = primary ('^' unary)?                   (2^3^2 = 2^9; -2^2 = -4)
//     primary = number | '(' expr ')' | name '(' expr ')' | pi | e

static const char* pos;
static const char* fail;  // why evaluating failed, or 0

static double parse_expr(void);
static double parse_unary(void);

static int is_digit(char c) { return c >= '0' && c <= '9'; }
static int is_letter(char c) { return c >= 'a' && c <= 'z'; }

static int name_is(const char* start, int n, const char* name) {
    for (int i = 0; i < n; i++)
        if (name[i] != start[i]) return 0;
    return name[n] == '\0';
}

// A number: 12, 3.5, .5, and 1.2e17 (as results are shown).
static double parse_number(void) {
    double v = 0, scale = 1;
    int digits = 0, seen_point = 0;
    for (; is_digit(*pos) || *pos == '.'; pos++) {
        if (*pos == '.') {
            if (seen_point) { fail = "Syntax error"; return 0; }
            seen_point = 1;
            continue;
        }
        digits++;
        if (seen_point) { scale /= 10; v += (*pos - '0') * scale; }
        else v = v * 10 + (*pos - '0');
    }
    if (digits == 0) { fail = "Syntax error"; return 0; }
    // An exponent only if digits follow: "2e" alone is 2 times e.
    const char* p = pos + 1;
    if (*pos == 'e' && (*p == '+' || *p == '-')) p++;
    if (*pos == 'e' && is_digit(*p)) {
        int negative = pos[1] == '-', exponent = 0;
        for (pos = p; is_digit(*pos); pos++) exponent = exponent * 10 + (*pos - '0');
        v *= pow(10, negative ? -exponent : exponent);
    }
    return v;
}

static double parse_primary(void) {
    if (*pos == '(') {
        pos++;
        double v = parse_expr();
        // A missing ')' at the very end is fine while you're still typing.
        if (*pos == ')') pos++;
        else if (*pos != '\0') fail = "Syntax error";
        return v;
    }
    if (is_digit(*pos) || *pos == '.') return parse_number();
    if (is_letter(*pos)) {
        const char* start = pos;
        while (is_letter(*pos)) pos++;
        int n = (int)(pos - start);
        if (name_is(start, n, "pi")) return M_PI;
        if (name_is(start, n, "e")) return M_E;
        // Everything else is a function, and needs its argument in brackets.
        if (*pos != '(') { fail = "Syntax error"; return 0; }
        double x = parse_primary();
        if (fail) return 0;
        if (name_is(start, n, "sin") || name_is(start, n, "cos")) {
            double v = name_is(start, n, "sin") ? sin(x) : cos(x);
            // pi is only pi to 16 digits, so sin(pi) comes out as 1.2e-16,
            // not 0. Like a pocket calculator, call anything that small 0.
            return v > -1e-15 && v < 1e-15 ? 0 : v;
        }
        if (name_is(start, n, "exp")) return exp(x);
        if (name_is(start, n, "ln")) return log(x);
        if (name_is(start, n, "log")) return log10(x);
        fail = "Syntax error";
        return 0;
    }
    fail = "Syntax error";
    return 0;
}

static double parse_power(void) {
    double base = parse_primary();
    if (!fail && *pos == '^') {
        pos++;
        // The exponent may be negative (2^-1), and powers chain to the
        // right: 2^3^2 is 2^(3^2).
        double exponent = parse_unary();
        if (!fail) return pow(base, exponent);
    }
    return base;
}

static double parse_unary(void) {
    if (*pos == '-') { pos++; return -parse_unary(); }
    if (*pos == '+') { pos++; return parse_unary(); }
    return parse_power();
}

static double parse_term(void) {
    double v = parse_unary();
    while (!fail) {
        if (*pos == '*' || *pos == '/') {
            char op = *pos++;
            double rhs = parse_unary();
            if (op == '*') v *= rhs;
            else if (rhs == 0) fail = "Can't divide by 0";
            else v /= rhs;
        } else if (*pos == '(' || is_digit(*pos) || *pos == '.' || is_letter(*pos)) {
            v *= parse_unary();  // implicit: 2pi, 3sin(1), (1+1)(2)
        } else {
            break;
        }
    }
    return v;
}

static double parse_expr(void) {
    double v = parse_term();
    while (!fail && (*pos == '+' || *pos == '-')) {
        char op = *pos++;
        double rhs = parse_term();
        v = op == '+' ? v + rhs : v - rhs;
    }
    return v;
}

// Evaluates `text`. Returns 0 and sets *out, or returns the error.
static const char* evaluate(const char* text, double* out) {
    pos = text;
    fail = 0;
    double v = parse_expr();
    if (!fail && *pos != '\0') fail = "Syntax error";
    // A NaN (ln of a negative number, say) isn't equal to itself.
    if (!fail && v != v) fail = "Not defined";
    if (!fail && (v > 1.7e308 || v < -1.7e308)) fail = "Too big";
    if (!fail) *out = v;
    return fail;
}

// ---- formatting numbers -----------------------------------------------------------
// printf has no %f, so numbers are formatted by hand: up to 12 significant
// digits (enough to hide 0.1 + 0.2 = 0.30000000000000004), trailing zeros
// trimmed, and scientific notation for very big or very small values.

static char* put_uint(char* p, uint64_t n, int min_digits) {
    char digits[24];
    int count = 0;
    do { digits[count++] = '0' + n % 10; n /= 10; } while (n > 0 || count < min_digits);
    while (count > 0) *p++ = digits[--count];
    return p;
}

static void format_number(double v, char* out) {
    char* p = out;
    if (v < 0) { *p++ = '-'; v = -v; }
    if (v != 0 && (v >= 1e15 || v < 1e-9)) {
        // Scientific: m x 10^e with 1 <= m < 10.
        int e = 0;
        while (v >= 10) { v /= 10; e++; }
        while (v < 1) { v *= 10; e--; }
        uint64_t m = (uint64_t)(v * 1e9 + 0.5);  // 10 significant digits
        if (m >= 10000000000ULL) { m /= 10; e++; }
        p = put_uint(p, m / 1000000000, 1);
        uint64_t frac = m % 1000000000;
        if (frac) {
            *p++ = '.';
            p = put_uint(p, frac, 9);
            while (p[-1] == '0') p--;
        }
        *p++ = 'e';
        if (e < 0) { *p++ = '-'; e = -e; }
        p = put_uint(p, (uint64_t)e, 1);
        *p = '\0';
        return;
    }
    // Fixed: as many decimals as 12 significant digits leave room for. (A
    // number below 1 has no significant digits before the point.)
    int int_digits = v < 1 ? 0 : 1;
    for (double t = v; t >= 10; t /= 10) int_digits++;
    int decimals = int_digits >= 12 ? 0 : 12 - int_digits;
    uint64_t scale = 1;
    for (int i = 0; i < decimals; i++) scale *= 10;
    uint64_t n = (uint64_t)(v * (double)scale + 0.5);
    p = put_uint(p, n / scale, 1);
    if (decimals > 0 && n % scale) {
        *p++ = '.';
        p = put_uint(p, n % scale, decimals);
        while (p[-1] == '0') p--;
    }
    *p = '\0';
}

// ---- editing ----------------------------------------------------------------------

static int is_operator(char c) {
    return c == '+' || c == '-' || c == '*' || c == '/' || c == '^';
}

static void clear(void) {
    len = 0;
    expr[0] = '\0';
    error[0] = '\0';
    just_evaluated = 0;
}

static void type_char(char c) {
    if (just_evaluated && !is_operator(c)) clear();
    just_evaluated = 0;
    error[0] = '\0';
    // Two operators in a row: the new one replaces the old, except that a
    // minus after * / or ^ is a negative number (2*-3, 2^-1).
    char prev = len > 0 ? expr[len - 1] : 0;
    if (is_operator(c) && is_operator(prev) && !(c == '-' && (prev == '*' || prev == '/' || prev == '^'))) {
        len--;
    }
    if (len < MAX_EXPR) {
        expr[len++] = c;
        expr[len] = '\0';
    }
}

static void backspace(void) {
    just_evaluated = 0;
    error[0] = '\0';
    if (len > 0) expr[--len] = '\0';
}

static void equals(void) {
    if (len == 0) return;
    double v;
    const char* err = evaluate(expr, &v);
    if (err) {
        int i = 0;
        for (; err[i] && i < (int)sizeof(error) - 1; i++) error[i] = err[i];
        error[i] = '\0';
        return;
    }
    format_number(v, expr);
    len = 0;
    while (expr[len]) len++;
    just_evaluated = 1;
}

static void type_text(const char* text) {
    for (; *text; text++) type_char(*text);
}

static void press(const char* label) {
    if (label[0] == 'C') clear();
    else if (label[0] == '<') backspace();
    else if (label[0] == '=') equals();
    else if (is_letter(label[0]) && label[1] && !name_is(label, 2, "pi")) {
        // A function: type its name and the opening bracket.
        type_text(label);
        type_char('(');
    } else {
        type_text(label);
    }
}

// ---- drawing ----------------------------------------------------------------------

static int button_at(int x, int y, int* row, int* col) {
    for (int r = 0; r < ROWS; r++) {
        for (int c = 0; c < COLS; c++) {
            int bx = GAP + c * (BTN_W + GAP), by = DISPLAY_H + GAP + r * (BTN_H + GAP);
            if (x >= bx && x < bx + BTN_W && y >= by && y < by + BTN_H) {
                *row = r;
                *col = c;
                return 1;
            }
        }
    }
    return 0;
}

// Draws `text` right-aligned to `right`, smaller if it's too wide, and
// showing just its end if even scale 1 is too wide.
static void text_right(canvas_t* cv, int right, int y, const char* text, uint32_t color, int scale) {
    int n = 0;
    while (text[n]) n++;
    while (scale > 1 && n * 8 * scale > right - 8) scale--;
    int fits = (right - 8) / (8 * scale);
    const char* shown = n > fits ? text + (n - fits) : text;
    canvas_text(cv, right - canvas_text_width(shown, scale), y, shown, color, scale);
}

static void draw(canvas_t* cv, int hover_r, int hover_c, int down) {
    canvas_clear(cv, rgb(0xe9, 0xeb, 0xef));
    // The display: the expression, and below it the result (a preview in
    // grey while typing, or an error in red).
    canvas_rect(cv, 0, 0, W, DISPLAY_H, rgb(0xfb, 0xfb, 0xfc));
    canvas_rect(cv, 0, DISPLAY_H - 1, W, 1, rgb(0xc8, 0xcb, 0xd2));
    if (just_evaluated) {
        text_right(cv, W - 10, 46, expr, rgb(0x10, 0x10, 0x10), 4);
    } else {
        text_right(cv, W - 10, 14, len ? expr : "0", rgb(0x30, 0x30, 0x30), 2);
        double v;
        char preview[48];
        if (error[0]) {
            text_right(cv, W - 10, 54, error, rgb(0xc0, 0x30, 0x30), 2);
        } else if (len && !evaluate(expr, &v)) {
            format_number(v, preview);
            text_right(cv, W - 10, 50, preview, rgb(0x90, 0x94, 0x9c), 3);
        }
    }
    // The keypad.
    for (int r = 0; r < ROWS; r++) {
        for (int c = 0; c < COLS; c++) {
            const char* label = labels[r][c];
            int x = GAP + c * (BTN_W + GAP), y = DISPLAY_H + GAP + r * (BTN_H + GAP);
            uint32_t bg = is_digit(label[0]) || label[0] == '.' ? rgb(0xff, 0xff, 0xff) : rgb(0xdd, 0xe3, 0xee);
            // The scientific rows get their own shade.
            if (r < 2) bg = rgb(0xe8, 0xe4, 0xf2);
            uint32_t fg = rgb(0x20, 0x20, 0x24);
            if (label[0] == '=') { bg = rgb(0x3a, 0x6e, 0xa5); fg = rgb(0xff, 0xff, 0xff); }
            if (label[0] == 'C') fg = rgb(0xc0, 0x30, 0x30);
            if (r == hover_r && c == hover_c) {
                // Darken a little on hover, more while pressed.
                uint32_t k = down ? 0x18 : 0x0c;
                uint32_t rr = (bg >> 16) & 0xff, gg = (bg >> 8) & 0xff, bb = bg & 0xff;
                bg = rgb(rr > k ? rr - k : 0, gg > k ? gg - k : 0, bb > k ? bb - k : 0);
            }
            canvas_rect(cv, x, y, BTN_W, BTN_H, bg);
            canvas_rect_outline(cv, x, y, BTN_W, BTN_H, rgb(0xc4, 0xc8, 0xd0));
            int tw = canvas_text_width(label, 2);
            canvas_text(cv, x + (BTN_W - tw) / 2, y + (BTN_H - 16) / 2, label, fg, 2);
        }
    }
}

void _start(void) {
    canvas_t cv;
    if (canvas_open(&cv, "Calculator", W, H) != 0) {
        writeln("calculator: couldn't open a canvas");
        exit();
    }
    clear();
    int hover_r = -1, hover_c = -1, down = 0, changed = 1;
    for (;;) {
        canvas_event_t e;
        while (canvas_poll(&cv, &e) > 0) {
            changed = 1;
            int r, c;
            switch (e.kind) {
            case CANVAS_CLOSED:
                canvas_close(&cv);
                exit();
            case CANVAS_MOUSE_MOVE:
                if (!button_at(e.x, e.y, &hover_r, &hover_c)) hover_r = hover_c = -1;
                break;
            case CANVAS_MOUSE_DOWN:
                if (e.buttons == CANVAS_LEFT && button_at(e.x, e.y, &r, &c)) {
                    hover_r = r;
                    hover_c = c;
                    down = 1;
                    press(labels[r][c]);
                }
                break;
            case CANVAS_MOUSE_UP:
                down = 0;
                break;
            case CANVAS_KEY: {
                int k = e.key;
                if (k == 'q') { canvas_close(&cv); exit(); }
                if (k >= 'A' && k <= 'Z') k += 'a' - 'A';
                if (is_digit((char)k) || is_letter((char)k) || k == '.' || k == '(' || k == ')' || is_operator((char)k)) type_char((char)k);
                else if (k == '\n' || k == '=') equals();
                else if (k == 8) backspace();
                else if (k == 27) clear();
                break;
            }
            }
        }
        // Only redraw when something changed.
        if (changed) {
            draw(&cv, hover_r, hover_c, down);
            canvas_present(&cv);
            changed = 0;
        }
        sleep_ms(15);
    }
}
