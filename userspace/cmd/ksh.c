// ksh: the KOS shell, in a window of its own.
//
//     ksh [script]
//
// Typed in the Terminal, it opens the Shell window and gives the Terminal
// back. Builtins (cd, ls, cat, echo, alias, ...) print in the window;
// programs in /bin run as usual and print where the Terminal does.
//
// A line can chain commands with ; && || and end one in & to run it in the
// background. $NAME, ${NAME} and $? expand, "double" quotes keep spaces
// and still expand, 'single' quotes keep everything, \ escapes one
// character, # starts a comment and * ? match file names. Up / Down go
// through history, Tab completes commands and paths, Page Up / Page Down
// scroll. /.kshrc runs at the start, and `source` runs any file of commands.
//
// ksh keeps its own current directory: relative paths given to programs
// are made absolute first. There are no pipes or redirection yet, since
// programs can't pass their output to anything but the console.

#include "../libc/sys.h"
#include "../libc/stdio.h"
#include "../libc/stdlib.h"
#include "../libc/string.h"
#include "../libc/ctype.h"
#include "../libc/time.h"
#include "../libc/canvas.h"
#include <stdarg.h>

#define SELF "/bin/ksh.elf.b64"
#define RC_FILE "/.kshrc"

// ---- look --------------------------------------------------------------------

#define WIN_W 800
#define WIN_H 480
#define HEADER_H 28
#define PAD 10
#define CELL_W 8
#define CELL_H 16
#define COLS ((WIN_W - 2 * PAD - 6) / CELL_W)
#define ROWS ((WIN_H - HEADER_H - 12) / CELL_H)
#define TEXT_TOP (HEADER_H + 6)

// Colors, as indexes into `palette` so a cell takes two bytes.
enum {
    C_NONE,   // no background
    C_TEXT,
    C_DIM,
    C_ERR,
    C_DIR,
    C_EXEC,
    C_ACCENT,
    C_PINK,
    C_DARK,
    C_PATH,
    C_WHITE,
    C_WARN,
};

static const uint32_t palette[] = {
    [C_NONE] = 0x000000,
    [C_TEXT] = 0xd6dcf0,
    [C_DIM] = 0x6b7193,
    [C_ERR] = 0xff5d7a,
    [C_DIR] = 0x7aa2ff,
    [C_EXEC] = 0x5fe3a1,
    [C_ACCENT] = 0x2ee6d6,
    [C_PINK] = 0xff6ac1,
    [C_DARK] = 0x0d0b1e,
    [C_PATH] = 0x5b3fd9,
    [C_WHITE] = 0xffffff,
    [C_WARN] = 0xffc857,
};

extern const uint8_t font8x8[128][8];

// ---- the screen ----------------------------------------------------------------
// Everything printed goes into a ring of lines; the last one is still
// being written ("open"). While ksh waits for a command, the prompt and
// the line being typed are drawn in place of the open line, which is then
// empty.

#define MAX_LINES 500
#define LIVE_LINES 8

typedef struct {
    char ch[COLS];
    uint8_t fg[COLS];
    uint8_t bg[COLS];
    int len;
} line_t;

static line_t lines[MAX_LINES];
static int first_line = 0, line_count = 1;

static line_t* line_at(int i) {
    return &lines[(first_line + i) % MAX_LINES];
}

static line_t* open_line(void) {
    return line_at(line_count - 1);
}

static void new_line(void) {
    if (line_count == MAX_LINES) {
        first_line = (first_line + 1) % MAX_LINES;
        line_count--;
    }
    line_at(line_count)->len = 0;
    line_count++;
}

static void put_char(char c, int fg, int bg) {
    if (c == '\n') {
        new_line();
        return;
    }
    if (c == '\t') {
        do put_char(' ', fg, bg); while (open_line()->len % 4);
        return;
    }
    if ((unsigned char)c < 32 || (unsigned char)c > 126) c = '?';
    line_t* l = open_line();
    if (l->len == COLS) {
        new_line();
        l = open_line();
    }
    l->ch[l->len] = c;
    l->fg[l->len] = fg;
    l->bg[l->len] = bg;
    l->len++;
}

static void put_bg(const char* s, int fg, int bg) {
    while (*s) put_char(*s++, fg, bg);
}

static void put(const char* s, int fg) {
    put_bg(s, fg, C_NONE);
}

static void print(int fg, const char* format, ...) __attribute__((format(printf, 2, 3)));
static void print(int fg, const char* format, ...) {
    char buf[1024];
    va_list args;
    va_start(args, format);
    vsnprintf(buf, sizeof buf, format, args);
    va_end(args);
    put(buf, fg);
}

// Starts a fresh line unless the open one is empty.
static void end_line(void) {
    if (open_line()->len) new_line();
}

// Prints "ksh: <message>" in red and returns 1, the failed status.
static int fail(const char* format, ...) __attribute__((format(printf, 1, 2)));
static int fail(const char* format, ...) {
    char buf[512];
    va_list args;
    va_start(args, format);
    vsnprintf(buf, sizeof buf, format, args);
    va_end(args);
    end_line();
    put("ksh: ", C_ERR);
    put(buf, C_ERR);
    put("\n", C_ERR);
    return 1;
}

static void clear_screen(void) {
    first_line = 0;
    line_count = 1;
    lines[0].len = 0;
}

// ---- state ---------------------------------------------------------------------

static canvas_t win;
static char cwd[256] = "/";
static char old_cwd[256] = "/";
static int status = 0;      // $?: the last command's, 0 for success

#define MAX_VARS 64
typedef struct {
    char* name;
    char* value;
    int exported;
} var_t;
static var_t vars[MAX_VARS];
static int var_count;

#define MAX_ALIASES 32
typedef struct {
    char* name;
    char* value;
} alias_t;
static alias_t aliases[MAX_ALIASES];
static int alias_count;

#define MAX_JOBS 16
typedef struct {
    long id;
    int number;
    char name[32];
} job_t;
static job_t jobs[MAX_JOBS];
static int job_count, next_job = 1;

#define MAX_HISTORY 200
static char* history[MAX_HISTORY];
static int history_count;

// ---- paths ---------------------------------------------------------------------

// `path` (absolute, or relative to ksh's directory) as a clean absolute
// path: no ".", "..", or doubled slashes.
static void abs_path(const char* path, char* out, size_t size) {
    char tmp[512];
    if (path[0] == '/') snprintf(tmp, sizeof tmp, "%s", path);
    else snprintf(tmp, sizeof tmp, "%s/%s", cwd, path);
    size_t len = 0;
    out[0] = '\0';
    char* save;
    for (char* part = strtok_r(tmp, "/", &save); part; part = strtok_r(NULL, "/", &save)) {
        if (!strcmp(part, ".")) continue;
        if (!strcmp(part, "..")) {
            while (len > 0 && out[len - 1] != '/') len--;
            if (len > 0) len--;
            out[len] = '\0';
            continue;
        }
        size_t n = strlen(part);
        if (len + 1 + n + 1 > size) break;
        out[len++] = '/';
        memcpy(out + len, part, n + 1);
        len += n;
    }
    if (len == 0) {
        out[0] = '/';
        out[1] = '\0';
    }
}

static int is_file(const char* abs) {
    return fs_size(abs) >= 0;
}

static int is_dir(const char* abs) {
    if (!strcmp(abs, "/")) return 1;
    if (is_file(abs)) return 0;
    int n;
    fs_entry_t* entries = fs_list(abs, &n);
    if (!entries) return 0;
    free(entries);
    return 1;
}

static int ends_with(const char* s, const char* suffix) {
    size_t a = strlen(s), b = strlen(suffix);
    return a >= b && !strcmp(s + a - b, suffix);
}

static int is_program(const char* name) {
    return ends_with(name, ".elf") || ends_with(name, ".elf.b64") || ends_with(name, ".kbin");
}

static int compare_names(const void* a, const void* b) {
    return strcasecmp(*(char* const*)a, *(char* const*)b);
}

static int compare_entries(const void* a, const void* b) {
    return strcasecmp(((const fs_entry_t*)a)->name, ((const fs_entry_t*)b)->name);
}

// ---- variables and aliases --------------------------------------------------------

static var_t* find_var(const char* name) {
    for (int i = 0; i < var_count; i++)
        if (!strcmp(vars[i].name, name)) return &vars[i];
    return NULL;
}

static const char* get_var(const char* name) {
    var_t* v = find_var(name);
    return v ? v->value : "";
}

static void set_var(const char* name, const char* value) {
    var_t* v = find_var(name);
    if (!v) {
        if (var_count == MAX_VARS) {
            fail("too many variables");
            return;
        }
        v = &vars[var_count++];
        v->name = strdup(name);
        v->value = NULL;
        v->exported = 0;
    }
    free(v->value);
    v->value = strdup(value);
}

static void unset_var(const char* name) {
    var_t* v = find_var(name);
    if (!v) return;
    free(v->name);
    free(v->value);
    *v = vars[--var_count];
}

static int is_name(const char* s, size_t len) {
    if (len == 0 || !(isalpha(s[0]) || s[0] == '_')) return 0;
    for (size_t i = 1; i < len; i++)
        if (!(isalnum(s[i]) || s[i] == '_')) return 0;
    return 1;
}

static alias_t* find_alias(const char* name) {
    for (int i = 0; i < alias_count; i++)
        if (!strcmp(aliases[i].name, name)) return &aliases[i];
    return NULL;
}

// ---- parsing ---------------------------------------------------------------------
// A line becomes tokens: words (already expanded and unquoted) and the
// operators between them.

typedef enum { T_WORD, T_SEMI, T_AND, T_OR, T_BG, T_PIPE, T_REDIR } kind_t;

typedef struct {
    kind_t kind;
    char* text;     // the word, or the operator as typed
    int quoted;     // had quotes or escapes: no alias, glob or assignment
    int glob;       // has an unquoted * or ?
    int assign;     // NAME=value: the '=' is at assign - 1
} token_t;

#define MAX_TOKENS 128
#define WORD_MAX 1024

typedef struct {
    token_t t[MAX_TOKENS];
    int n;
} tokens_t;

static void free_tokens(tokens_t* ts) {
    for (int i = 0; i < ts->n; i++) free(ts->t[i].text);
    ts->n = 0;
}

static int add_token(tokens_t* ts, kind_t kind, const char* text, int quoted, int glob, int assign) {
    if (ts->n == MAX_TOKENS) return fail("line is too long");
    ts->t[ts->n++] = (token_t){ kind, strdup(text), quoted, glob, assign };
    return 0;
}

typedef struct {
    char s[WORD_MAX];
    int len;
} word_t;

static void word_add(word_t* w, char c) {
    if (w->len < WORD_MAX - 1) w->s[w->len++] = c;
}

static void word_add_str(word_t* w, const char* s) {
    while (*s) word_add(w, *s++);
}

// Expands the variable at p (just after a '$') into w. Returns how many
// characters it used.
static int expand_var(const char* p, word_t* w) {
    char name[64];
    if (*p == '?') {
        char num[16];
        snprintf(num, sizeof num, "%d", status);
        word_add_str(w, num);
        return 1;
    }
    if (*p == '{') {
        const char* end = strchr(p, '}');
        if (!end || end - p - 1 >= (long)sizeof name) {
            word_add(w, '$');
            return 0;
        }
        memcpy(name, p + 1, end - p - 1);
        name[end - p - 1] = '\0';
        word_add_str(w, get_var(name));
        return end - p + 1;
    }
    int n = 0;
    while ((isalnum(p[n]) || p[n] == '_') && n < (int)sizeof name - 1) {
        name[n] = p[n];
        n++;
    }
    if (n == 0 || isdigit(p[0])) {
        word_add(w, '$');
        return 0;
    }
    name[n] = '\0';
    word_add_str(w, get_var(name));
    return n;
}

// Splits the text at *pp into tokens, moving *pp past what it used. With
// `one`, it stops after the first operator, so a line can be run one
// command at a time and `X=1; echo $X` sees X. Returns 0, or 1 after
// printing what's wrong.
static int lex(const char** pp, tokens_t* ts, int one) {
    ts->n = 0;
    const char* p = *pp;
    for (;;) {
        *pp = p;
        while (*p == ' ' || *p == '\t') p++;
        if (!*p || *p == '#') {
            *pp = p + strlen(p);
            return 0;
        }
        char c = *p;
        if (strchr(";&|<>", c)) {
            int two = p[1] == c && c != ';' && c != '<';
            static const struct { char c; int two; kind_t kind; const char* text; } ops[] = {
                { ';', 0, T_SEMI, ";" }, { '&', 0, T_BG, "&" }, { '&', 1, T_AND, "&&" },
                { '|', 0, T_PIPE, "|" }, { '|', 1, T_OR, "||" }, { '>', 0, T_REDIR, ">" },
                { '>', 1, T_REDIR, ">>" }, { '<', 0, T_REDIR, "<" },
            };
            for (int i = 0; i < (int)(sizeof ops / sizeof ops[0]); i++) {
                if (ops[i].c != c || ops[i].two != two) continue;
                if (add_token(ts, ops[i].kind, ops[i].text, 0, 0, 0)) return 1;
            }
            p += two ? 2 : 1;
            if (one) {
                *pp = p;
                return 0;
            }
            continue;
        }

        word_t w = { .len = 0 };
        int quoted = 0, glob = 0, assign = 0;
        while (*p && !strchr(" \t;&|<>", *p)) {
            c = *p;
            if (c == '\'') {
                const char* end = strchr(p + 1, '\'');
                if (!end) return fail("missing closing '");
                while (++p < end) word_add(&w, *p);
                p++;
                quoted = 1;
            } else if (c == '"') {
                p++;
                while (*p && *p != '"') {
                    if (*p == '\\' && p[1] && strchr("\"\\$", p[1])) {
                        word_add(&w, p[1]);
                        p += 2;
                    } else if (*p == '$') {
                        p++;
                        p += expand_var(p, &w);
                    } else {
                        word_add(&w, *p++);
                    }
                }
                if (!*p) return fail("missing closing \"");
                p++;
                quoted = 1;
            } else if (c == '\\') {
                if (p[1]) word_add(&w, p[1]);
                p += p[1] ? 2 : 1;
                quoted = 1;
            } else if (c == '$') {
                p++;
                p += expand_var(p, &w);
            } else {
                if (c == '=' && !assign && !quoted && is_name(w.s, w.len)) assign = w.len + 1;
                if (c == '*' || c == '?') glob = !quoted;
                word_add(&w, c);
                p++;
            }
        }
        w.s[w.len] = '\0';
        if (add_token(ts, T_WORD, w.s, quoted, glob, assign)) return 1;
    }
}

// Replaces aliases at the start of each command with their tokens. An
// alias isn't expanded inside its own expansion, so `alias ls='ls -a'`
// works.
static int expand_aliases(tokens_t* ts) {
    const char* used[8];
    int used_count = 0;
    int at_start = 1;
    for (int i = 0; i < ts->n; i++) {
        token_t* t = &ts->t[i];
        if (t->kind != T_WORD) {
            at_start = 1;
            used_count = 0;
            continue;
        }
        if (!at_start) continue;
        alias_t* a = t->quoted ? NULL : find_alias(t->text);
        for (int u = 0; a && u < used_count; u++)
            if (used[u] == a->name) a = NULL;
        if (!a || used_count == 8) {
            at_start = 0;
            continue;
        }
        used[used_count++] = a->name;
        tokens_t* sub = malloc(sizeof *sub);
        if (!sub) return fail("out of memory");
        const char* value = a->value;
        if (lex(&value, sub, 0)) {
            free(sub);
            return 1;
        }
        if (ts->n - 1 + sub->n > MAX_TOKENS) {
            free_tokens(sub);
            free(sub);
            return fail("line is too long");
        }
        free(t->text);
        memmove(&ts->t[i + sub->n], &ts->t[i + 1], (ts->n - i - 1) * sizeof(token_t));
        memcpy(&ts->t[i], sub->t, sub->n * sizeof(token_t));
        ts->n += sub->n - 1;
        free(sub);
        i--;  // look at the alias's first word again
    }
    return 0;
}

// Whether `name` matches the glob `pattern` (* and ?).
static int glob_match(const char* pattern, const char* name) {
    if (!*pattern) return !*name;
    if (*pattern == '*') {
        for (;;) {
            if (glob_match(pattern + 1, name)) return 1;
            if (!*name) return 0;
            name++;
        }
    }
    if (!*name) return 0;
    if (*pattern == '?' || *pattern == *name) return glob_match(pattern + 1, name + 1);
    return 0;
}

#define MAX_ARGS 64

typedef struct {
    char* v[MAX_ARGS];
    int n;
} args_t;

static void free_args(args_t* a) {
    for (int i = 0; i < a->n; i++) free(a->v[i]);
    a->n = 0;
}

static int add_arg(args_t* a, const char* s) {
    if (a->n == MAX_ARGS - 1) return fail("too many arguments");
    a->v[a->n++] = strdup(s);
    a->v[a->n] = NULL;
    return 0;
}

// Adds the files matching `word` (a glob in its last part) in sorted
// order, or the word itself if none do.
static int add_glob(args_t* a, const char* word) {
    const char* slash = strrchr(word, '/');
    char dir_part[256] = "";
    const char* pattern = word;
    if (slash) {
        size_t n = slash - word + 1;
        if (n >= sizeof dir_part) return add_arg(a, word);
        memcpy(dir_part, word, n);
        dir_part[n] = '\0';
        pattern = slash + 1;
        if (strpbrk(dir_part, "*?")) return add_arg(a, word);
    }
    char dir[256];
    abs_path(dir_part[0] ? dir_part : ".", dir, sizeof dir);
    int count;
    fs_entry_t* entries = fs_list(dir, &count);
    if (!entries) return add_arg(a, word);
    char* found[MAX_ARGS];
    int n = 0;
    for (int i = 0; i < count && n < MAX_ARGS; i++) {
        if (entries[i].name[0] == '.' && pattern[0] != '.') continue;
        if (!glob_match(pattern, entries[i].name)) continue;
        char full[512];
        snprintf(full, sizeof full, "%s%s", dir_part, entries[i].name);
        found[n++] = strdup(full);
    }
    free(entries);
    if (n == 0) return add_arg(a, word);
    qsort(found, n, sizeof found[0], compare_names);
    int err = 0;
    for (int i = 0; i < n; i++) {
        if (!err) err = add_arg(a, found[i]);
        free(found[i]);
    }
    return err;
}

// ---- jobs ------------------------------------------------------------------------

// Reports background jobs that have ended, and forgets them.
static int check_jobs(void) {
    int changed = 0;
    for (int i = 0; i < job_count;) {
        if (proc_running(jobs[i].id)) {
            i++;
            continue;
        }
        end_line();
        print(C_DIM, "[%d] done  %s\n", jobs[i].number, jobs[i].name);
        jobs[i] = jobs[--job_count];
        changed = 1;
    }
    if (job_count == 0) next_job = 1;
    return changed;
}

// ---- drawing ---------------------------------------------------------------------

static int prompting = 0;
static char input[700];
static int input_len, input_cursor;
static int scroll = 0;          // lines scrolled back from the bottom
static int blink_on = 1;
static uint64_t blink_at;

static line_t live[LIVE_LINES];
static int live_count, cursor_row, cursor_col;

static void live_put(char c, int fg, int bg) {
    line_t* l = &live[live_count - 1];
    if (l->len == COLS) {
        if (live_count == LIVE_LINES) return;
        l = &live[live_count++];
        l->len = 0;
    }
    l->ch[l->len] = c;
    l->fg[l->len] = fg;
    l->bg[l->len] = bg;
    l->len++;
}

static void live_put_str(const char* s, int fg, int bg) {
    while (*s) live_put(*s++, fg, bg);
}

// The prompt: a "ksh" badge (red after a failed command), the directory,
// then an arrow.
static void prompt_to(void (*out)(const char*, int, int)) {
    char dir[sizeof cwd + 2];
    snprintf(dir, sizeof dir, " %s ", cwd);
    out(" ksh ", C_DARK, status ? C_ERR : C_ACCENT);
    out(dir, C_WHITE, C_PATH);
    out(" > ", C_PINK, C_NONE);
}

// Lays out the prompt and the line being typed, and finds the cursor.
static void build_live(void) {
    live_count = 1;
    live[0].len = 0;
    prompt_to(live_put_str);
    int start = live_count == 1 ? live[0].len : (live_count - 1) * COLS + live[live_count - 1].len;
    for (int i = 0; i < input_len; i++) live_put(input[i], C_TEXT, C_NONE);
    int pos = start + input_cursor;
    cursor_row = pos / COLS;
    cursor_col = pos % COLS;
    if (cursor_row >= LIVE_LINES) {
        cursor_row = LIVE_LINES - 1;
        cursor_col = COLS - 1;
    }
    // The cursor can sit just past the last character.
    while (live_count <= cursor_row) live[live_count++].len = 0;
}

static void draw_glyph(int x, int y, unsigned char ch, uint32_t color) {
    const uint8_t* glyph = font8x8[ch < 128 ? ch : '?'];
    for (int row = 0; row < 8; row++) {
        uint8_t bits = glyph[row];
        for (int col = 0; bits && col < 8; col++) {
            if (!(bits & (1 << col))) continue;
            int px = x + col, py = y + row * 2;
            if (px < 0 || px >= win.width || py < 0 || py + 1 >= win.height) continue;
            win.pixels[py * win.width + px] = color;
            win.pixels[(py + 1) * win.width + px] = color;
        }
    }
}

static void draw_text(int x, int y, const char* s, uint32_t color) {
    for (; *s; s++, x += CELL_W) draw_glyph(x, y, (unsigned char)*s, color);
}

static uint32_t mix(uint32_t a, uint32_t b, int t, int of) {
    uint32_t out = 0;
    for (int shift = 0; shift <= 16; shift += 8) {
        int ca = (a >> shift) & 0xff, cb = (b >> shift) & 0xff;
        out |= (uint32_t)(ca + (cb - ca) * t / of) << shift;
    }
    return out;
}

static void draw_line(const line_t* l, int y, int cursor) {
    for (int i = 0; i < l->len || i == cursor; i++) {
        int x = PAD + i * CELL_W;
        unsigned char ch = i < l->len ? l->ch[i] : ' ';
        uint32_t fg = i < l->len ? palette[l->fg[i]] : 0;
        if (i < l->len && l->bg[i]) canvas_rect(&win, x, y, CELL_W, CELL_H, palette[l->bg[i]]);
        if (i == cursor) {
            canvas_rect(&win, x, y, CELL_W, CELL_H, palette[C_ACCENT]);
            fg = palette[C_DARK];
        }
        if (ch != ' ') draw_glyph(x, y, ch, fg);
    }
}

static int total_rows(void) {
    int tail = prompting ? live_count : (open_line()->len ? 1 : 0);
    return line_count - 1 + tail;
}

static void render(void) {
    if (prompting) build_live();
    // A deep indigo gradient.
    for (int y = 0; y < win.height; y++) {
        uint32_t color = mix(0x1a1336, 0x08080f, y, win.height);
        uint32_t* row = &win.pixels[y * win.width];
        for (int x = 0; x < win.width; x++) row[x] = color;
    }

    // Header: the name, the directory, jobs and the time, over a glowing
    // teal-to-pink rule.
    canvas_rect(&win, 0, 0, win.width, HEADER_H, 0x120e26);
    for (int x = 0; x < win.width; x++) {
        uint32_t c = mix(palette[C_ACCENT], palette[C_PINK], x, win.width);
        win.pixels[(HEADER_H - 2) * win.width + x] = c;
        win.pixels[(HEADER_H - 1) * win.width + x] = mix(c, 0x120e26, 1, 2);
    }
    canvas_text(&win, PAD, 5, "ksh", palette[C_ACCENT], 2);
    canvas_text(&win, PAD + 1, 5, "ksh", palette[C_ACCENT], 2);
    draw_text(PAD + 60, 6, cwd, palette[C_TEXT]);
    char right[48];
    time_t now = unix_time();
    struct tm tm;
    localtime_r(&now, &tm);
    if (job_count) snprintf(right, sizeof right, "%d job%s  %02d:%02d", job_count, job_count == 1 ? "" : "s", tm.tm_hour, tm.tm_min);
    else snprintf(right, sizeof right, "%02d:%02d", tm.tm_hour, tm.tm_min);
    draw_text(win.width - PAD - (int)strlen(right) * CELL_W, 6, right, palette[C_DIM]);

    // Text.
    int total = total_rows();
    int max_scroll = total > ROWS ? total - ROWS : 0;
    if (scroll > max_scroll) scroll = max_scroll;
    int top = total - ROWS - scroll;
    if (top < 0) top = 0;
    for (int r = 0; r < ROWS && top + r < total; r++) {
        int i = top + r;
        int y = TEXT_TOP + r * CELL_H;
        if (i < line_count - 1) {
            draw_line(line_at(i), y, -1);
        } else if (prompting) {
            int li = i - (line_count - 1);
            draw_line(&live[li], y, li == cursor_row && blink_on && scroll == 0 ? cursor_col : -1);
        } else {
            draw_line(open_line(), y, -1);
        }
    }

    // A thin scrollbar once there's more than fits.
    if (max_scroll > 0) {
        int track_h = ROWS * CELL_H;
        int thumb_h = track_h * ROWS / total;
        if (thumb_h < 12) thumb_h = 12;
        int thumb_y = TEXT_TOP + (track_h - thumb_h) * (max_scroll - scroll) / max_scroll;
        canvas_rect(&win, win.width - 5, TEXT_TOP, 2, track_h, 0x221b44);
        canvas_rect(&win, win.width - 5, thumb_y, 2, thumb_h, palette[C_ACCENT]);
    }
    canvas_present(&win);
}

// ---- running commands ----------------------------------------------------------

static int run_line(const char* line);
static int run_file(const char* path);

static __attribute__((noreturn)) void quit(void) {
    canvas_close(&win);
    exit();
}

// Commands only the kernel shell (the Terminal) has.
static const char* kernel_commands[] = {
    "k", "kai", "net", "ping", "fetch", "browse", "open", "stats", "log", "dmesg",
    "mmu", "load", "time", "tasks", "run", "bg", "decode", "encode", "hash",
    "heap_remain", NULL,
};

static int is_kernel_command(const char* name) {
    for (int i = 0; kernel_commands[i]; i++)
        if (!strcmp(kernel_commands[i], name)) return 1;
    return 0;
}

// Where program `name` is: a path, or /bin/<name>.elf.b64. Returns 0 if
// it's found.
static int find_program(const char* name, char* out, size_t size) {
    if (strchr(name, '/') || is_program(name)) {
        abs_path(name, out, size);
        return is_file(out) ? 0 : -1;
    }
    snprintf(out, size, "/bin/%s.elf.b64", name);
    return is_file(out) ? 0 : -1;
}

static int start_program(args_t* a, int background) {
    char path[256];
    if (find_program(a->v[0], path, sizeof path)) {
        if (is_kernel_command(a->v[0]))
            return fail("%s: that's a kernel shell command; run it in the Terminal", a->v[0]);
        return fail("%s: command not found", a->v[0]);
    }
    // Programs get their arguments as one string split at spaces, and
    // look up relative paths from the Terminal's directory, not ours.
    char args[1024] = "";
    size_t len = 0;
    for (int i = 1; i < a->n; i++) {
        const char* arg = a->v[i];
        if (strchr(arg, ' ')) return fail("%s: arguments with spaces can't be passed to programs yet", a->v[0]);
        char full[256];
        if (arg[0] != '-' && arg[0] != '/') {
            abs_path(arg, full, sizeof full);
            if (is_file(full) || is_dir(full)) arg = full;
        }
        int n = snprintf(args + len, sizeof args - len, "%s%s", len ? " " : "", arg);
        if (n < 0 || len + n >= sizeof args) return fail("%s: arguments are too long", a->v[0]);
        len += n;
    }

    long id = spawn(path, args);
    if (id < 0) return fail("%s: %s", a->v[0], last_error());
    const char* base = strrchr(a->v[0], '/') ? strrchr(a->v[0], '/') + 1 : a->v[0];
    if (background) {
        if (job_count == MAX_JOBS) {
            print(C_WARN, "ksh: too many jobs to track; %s runs untracked\n", base);
            return 0;
        }
        job_t* j = &jobs[job_count++];
        j->id = id;
        j->number = next_job++;
        snprintf(j->name, sizeof j->name, "%s", base);
        print(C_DIM, "[%d] %ld  %s\n", j->number, id, base);
        return 0;
    }
    end_line();
    print(C_DIM, "  %s is running; its output is in the Terminal\n", base);
    render();
    proc_wait(id);
    return 0;
}

// ---- builtins --------------------------------------------------------------------

typedef int (*builtin_fn)(int argc, char** argv);

static void print_entry(const fs_entry_t* e, int width) {
    int color = e->is_dir ? C_DIR : is_program(e->name) ? C_EXEC : C_TEXT;
    put(e->name, color);
    int len = (int)strlen(e->name);
    if (e->is_dir) {
        put("/", C_DIR);
        len++;
    }
    for (; len < width; len++) put_char(' ', C_TEXT, C_NONE);
}

static void human_size(uint32_t size, char* out, size_t n) {
    if (size < 1024) snprintf(out, n, "%u B", (unsigned)size);
    else if (size < 1024 * 1024) snprintf(out, n, "%u.%u K", (unsigned)(size / 1024), (unsigned)(size % 1024 * 10 / 1024));
    else snprintf(out, n, "%u.%u M", (unsigned)(size >> 20), (unsigned)((size & 0xfffff) * 10 >> 20));
}

static int b_ls(int argc, char** argv) {
    int all = 0, longform = 0, paths = 0, result = 0;
    for (int i = 1; i < argc; i++) {
        if (argv[i][0] != '-') {
            paths++;
            continue;
        }
        for (const char* f = argv[i] + 1; *f; f++) {
            if (*f == 'a') all = 1;
            else if (*f == 'l') longform = 1;
            else return fail("ls: unknown option -%c (try -a, -l)", *f);
        }
    }
    for (int i = paths ? 1 : 0; i < argc; i++) {
        const char* arg = i == 0 ? "." : argv[i];
        if (i && arg[0] == '-') continue;
        char path[256];
        abs_path(arg, path, sizeof path);
        if (is_file(path)) {
            fs_entry_t e = { .is_dir = 0, .size = (uint32_t)fs_size(path) };
            snprintf(e.name, sizeof e.name, "%s", arg);
            print_entry(&e, 0);
            put("\n", C_TEXT);
            continue;
        }
        int count;
        fs_entry_t* entries = fs_list(path, &count);
        if (!entries) {
            result = fail("ls: no such file or folder: %s", arg);
            continue;
        }
        if (paths > 1) print(C_ACCENT, "%s:\n", arg);
        qsort(entries, count, sizeof *entries, compare_entries);
        int shown = 0, width = 0;
        for (int k = 0; k < count; k++) {
            if (entries[k].name[0] == '.' && !all) continue;
            int w = (int)strlen(entries[k].name) + entries[k].is_dir;
            if (w > width) width = w;
            entries[shown++] = entries[k];
        }
        if (longform) {
            for (int k = 0; k < shown; k++) {
                char size[16] = "-";
                if (!entries[k].is_dir) human_size(entries[k].size, size, sizeof size);
                print(C_DIM, "%9s  ", size);
                print_entry(&entries[k], 0);
                put("\n", C_TEXT);
            }
        } else if (shown) {
            width += 2;
            int columns = COLS / width;
            if (columns < 1) columns = 1;
            int rows = (shown + columns - 1) / columns;
            for (int r = 0; r < rows; r++) {
                for (int col = 0; col < columns; col++) {
                    int k = col * rows + r;
                    if (k < shown) print_entry(&entries[k], col + 1 < columns ? width : 0);
                }
                put("\n", C_TEXT);
            }
        }
        free(entries);
    }
    return result;
}

static int b_cat(int argc, char** argv) {
    if (argc < 2) return fail("cat: usage: cat <file>...");
    int result = 0;
    for (int i = 1; i < argc; i++) {
        char path[256];
        abs_path(argv[i], path, sizeof path);
        size_t len;
        char* data = fs_read_all(path, &len);
        if (!data) {
            result = fail("cat: %s: %s", argv[i], is_dir(path) ? "is a folder" : "no such file");
            continue;
        }
        for (size_t k = 0; k < len; k++) {
            char c = data[k];
            if (c == '\r') continue;
            put_char(c == '\n' || c == '\t' || isprint((unsigned char)c) ? c : '.', C_TEXT, C_NONE);
        }
        free(data);
        end_line();
    }
    return result;
}

static int b_cd(int argc, char** argv) {
    const char* target = argc < 2 ? "/" : argv[1];
    if (!strcmp(target, "-")) target = old_cwd;
    char path[256];
    abs_path(target, path, sizeof path);
    if (!is_dir(path)) return fail("cd: %s: %s", target, is_file(path) ? "not a folder" : "no such folder");
    strcpy(old_cwd, cwd);
    strcpy(cwd, path);
    return 0;
}

static int b_pwd(int argc, char** argv) {
    print(C_TEXT, "%s\n", cwd);
    return 0;
}

static int b_mkdir(int argc, char** argv) {
    if (argc < 2) return fail("mkdir: usage: mkdir <folder>...");
    int result = 0;
    for (int i = 1; i < argc; i++) {
        char path[256];
        abs_path(argv[i], path, sizeof path);
        if (fs_mkdir(path) < 0) result = fail("mkdir: %s: %s", argv[i], last_error());
    }
    return result;
}

static int b_rm(int argc, char** argv) {
    if (argc < 2) return fail("rm: usage: rm <file or empty folder>...");
    int result = 0;
    for (int i = 1; i < argc; i++) {
        char path[256];
        abs_path(argv[i], path, sizeof path);
        if (fs_remove(path) < 0) result = fail("rm: %s: %s", argv[i], last_error());
    }
    return result;
}

static int b_write(int argc, char** argv) {
    if (argc < 3) return fail("write: usage: write <file> <text>...");
    char text[1024] = "";
    size_t len = 0;
    for (int i = 2; i < argc; i++) {
        int n = snprintf(text + len, sizeof text - len, "%s%s", i > 2 ? " " : "", argv[i]);
        if (n < 0 || len + n >= sizeof text) return fail("write: text is too long");
        len += n;
    }
    char path[256];
    abs_path(argv[1], path, sizeof path);
    if (fs_write(path, text, len) < 0) return fail("write: %s: %s", argv[1], last_error());
    return 0;
}

static int b_echo(int argc, char** argv) {
    int newline = 1, i = 1;
    if (argc > 1 && !strcmp(argv[1], "-n")) {
        newline = 0;
        i++;
    }
    for (int first = i; i < argc; i++) {
        if (i > first) put(" ", C_TEXT);
        put(argv[i], C_TEXT);
    }
    if (newline) put("\n", C_TEXT);
    return 0;
}

static int b_clear(int argc, char** argv) {
    clear_screen();
    return 0;
}

static int b_history(int argc, char** argv) {
    for (int i = 0; i < history_count; i++) {
        print(C_DIM, "%4d  ", i + 1);
        print(C_TEXT, "%s\n", history[i]);
    }
    return 0;
}

static int b_alias(int argc, char** argv) {
    if (argc == 1) {
        for (int i = 0; i < alias_count; i++) print(C_TEXT, "alias %s='%s'\n", aliases[i].name, aliases[i].value);
        return 0;
    }
    int result = 0;
    for (int i = 1; i < argc; i++) {
        char* eq = strchr(argv[i], '=');
        if (!eq) {
            alias_t* a = find_alias(argv[i]);
            if (a) print(C_TEXT, "alias %s='%s'\n", a->name, a->value);
            else result = fail("alias: %s: no such alias", argv[i]);
            continue;
        }
        *eq = '\0';
        if (!is_name(argv[i], strlen(argv[i]))) {
            result = fail("alias: %s: not a valid name", argv[i]);
            continue;
        }
        alias_t* a = find_alias(argv[i]);
        if (!a) {
            if (alias_count == MAX_ALIASES) return fail("alias: too many aliases");
            a = &aliases[alias_count++];
            a->name = strdup(argv[i]);
            a->value = NULL;
        }
        free(a->value);
        a->value = strdup(eq + 1);
    }
    return result;
}

static int b_unalias(int argc, char** argv) {
    if (argc < 2) return fail("unalias: usage: unalias <name>...");
    int result = 0;
    for (int i = 1; i < argc; i++) {
        alias_t* a = find_alias(argv[i]);
        if (!a) {
            result = fail("unalias: %s: no such alias", argv[i]);
            continue;
        }
        free(a->name);
        free(a->value);
        *a = aliases[--alias_count];
    }
    return result;
}

static int b_set(int argc, char** argv) {
    for (int i = 0; i < var_count; i++) print(C_TEXT, "%s=%s\n", vars[i].name, vars[i].value);
    return 0;
}

static int b_export(int argc, char** argv) {
    if (argc == 1) {
        for (int i = 0; i < var_count; i++)
            if (vars[i].exported) print(C_TEXT, "export %s=%s\n", vars[i].name, vars[i].value);
        return 0;
    }
    int result = 0;
    for (int i = 1; i < argc; i++) {
        char* eq = strchr(argv[i], '=');
        if (eq) *eq = '\0';
        if (!is_name(argv[i], strlen(argv[i]))) {
            result = fail("export: %s: not a valid name", argv[i]);
            continue;
        }
        if (eq) set_var(argv[i], eq + 1);
        else if (!find_var(argv[i])) set_var(argv[i], "");
        var_t* v = find_var(argv[i]);
        if (v) v->exported = 1;
    }
    return result;
}

static int b_unset(int argc, char** argv) {
    for (int i = 1; i < argc; i++) unset_var(argv[i]);
    return 0;
}

static int b_source(int argc, char** argv) {
    if (argc < 2) return fail("%s: usage: %s <file>", argv[0], argv[0]);
    return run_file(argv[1]);
}

static int b_exit(int argc, char** argv) {
    quit();
}

static int b_jobs(int argc, char** argv) {
    check_jobs();
    for (int i = 0; i < job_count; i++) print(C_TEXT, "[%d] %ld  %s\n", jobs[i].number, jobs[i].id, jobs[i].name);
    return 0;
}

static int b_kill(int argc, char** argv) {
    if (argc < 2) return fail("kill: usage: kill <id> or kill %%<job>");
    int result = 0;
    for (int i = 1; i < argc; i++) {
        long id = -1;
        if (argv[i][0] == '%') {
            int number = atoi(argv[i] + 1);
            for (int k = 0; k < job_count; k++)
                if (jobs[k].number == number) id = jobs[k].id;
            if (id < 0) {
                result = fail("kill: %s: no such job", argv[i]);
                continue;
            }
        } else {
            char* end;
            id = strtol(argv[i], &end, 10);
            if (*end || id < 0) {
                result = fail("kill: %s: not an id", argv[i]);
                continue;
            }
        }
        if (proc_kill(id) < 0) result = fail("kill: %s: %s", argv[i], last_error());
    }
    return result;
}

static int b_true(int argc, char** argv) { return 0; }
static int b_false(int argc, char** argv) { return 1; }

static int b_which(int argc, char** argv);
static int b_help(int argc, char** argv);

static const struct {
    const char* name;
    builtin_fn fn;
    const char* help;
} builtins[] = {
    { "alias", b_alias, "alias [name='command'] - list or set aliases" },
    { "cat", b_cat, "cat <file>... - print files" },
    { "cd", b_cd, "cd [folder | -] - change folder (- goes back)" },
    { "clear", b_clear, "clear - clear the window" },
    { "echo", b_echo, "echo [-n] <text>... - print text" },
    { "exit", b_exit, "exit - close the Shell" },
    { "export", b_export, "export [NAME[=value]] - mark variables exported" },
    { "false", b_false, "false - fail" },
    { "help", b_help, "help - this list" },
    { "history", b_history, "history - lines typed so far" },
    { "jobs", b_jobs, "jobs - programs running in the background" },
    { "kill", b_kill, "kill <id | %job> - end a program" },
    { "ls", b_ls, "ls [-a] [-l] [path]... - list files" },
    { "mkdir", b_mkdir, "mkdir <folder>... - make folders" },
    { "pwd", b_pwd, "pwd - print the current folder" },
    { "rm", b_rm, "rm <path>... - delete files or empty folders" },
    { "set", b_set, "set - list variables" },
    { "source", b_source, "source <file> - run a file of commands (also '.')" },
    { ".", b_source, NULL },
    { "true", b_true, "true - succeed" },
    { "type", b_which, NULL },
    { "unalias", b_unalias, "unalias <name>... - remove aliases" },
    { "unset", b_unset, "unset <NAME>... - remove variables" },
    { "which", b_which, "which <name>... - what a command runs (also 'type')" },
    { "write", b_write, "write <file> <text>... - write text to a file" },
};

#define BUILTIN_COUNT (int)(sizeof builtins / sizeof builtins[0])

static builtin_fn find_builtin(const char* name) {
    for (int i = 0; i < BUILTIN_COUNT; i++)
        if (!strcmp(builtins[i].name, name)) return builtins[i].fn;
    return NULL;
}

static int b_which(int argc, char** argv) {
    int result = 0;
    for (int i = 1; i < argc; i++) {
        char path[256];
        alias_t* a = find_alias(argv[i]);
        if (a) print(C_TEXT, "%s: alias for '%s'\n", argv[i], a->value);
        else if (find_builtin(argv[i])) print(C_TEXT, "%s: ksh builtin\n", argv[i]);
        else if (!find_program(argv[i], path, sizeof path)) print(C_TEXT, "%s: %s\n", argv[i], path);
        else if (is_kernel_command(argv[i])) print(C_TEXT, "%s: kernel shell command (Terminal only)\n", argv[i]);
        else result = fail("which: %s: not found", argv[i]);
    }
    return result;
}

static int b_help(int argc, char** argv) {
    put("Builtins\n", C_ACCENT);
    for (int i = 0; i < BUILTIN_COUNT; i++)
        if (builtins[i].help) print(C_TEXT, "  %s\n", builtins[i].help);
    put("Programs\n", C_ACCENT);
    put("  Anything in /bin by name (grep, wc, curl...), or a program by path.\n"
        "  They print in the Terminal. Add & to run one in the background.\n", C_TEXT);
    put("Lines\n", C_ACCENT);
    put("  a ; b   a && b   a || b   prog &   NAME=value   $NAME ${NAME} $?\n"
        "  \"double\" and 'single' quotes, \\ escapes, * ? globs, # comments\n", C_TEXT);
    put("Keys\n", C_ACCENT);
    put("  Up/Down history, Tab completes, PgUp/PgDn scroll, Esc clears the line\n", C_TEXT);
    put("Not yet: pipes (|) and redirection (> >> <)\n", C_DIM);
    return 0;
}

// ---- executing lines -----------------------------------------------------------

static int run_command(token_t* words, int n, int background) {
    // Leading NAME=value words set variables.
    int i = 0;
    while (i < n && words[i].assign) i++;
    for (int k = 0; k < i; k++) {
        char* text = words[k].text;
        text[words[k].assign - 1] = '\0';
        set_var(text, text + words[k].assign);
    }
    if (i == n) return 0;

    args_t a = { .n = 0 };
    int err = 0;
    for (; i < n && !err; i++) err = words[i].glob ? add_glob(&a, words[i].text) : add_arg(&a, words[i].text);
    if (err) {
        free_args(&a);
        return 1;
    }
    int result;
    builtin_fn fn = find_builtin(a.v[0]);
    if (fn && background) result = fail("%s: builtins can't run in the background", a.v[0]);
    else if (fn) result = fn(a.n, a.v);
    else result = start_program(&a, background);
    free_args(&a);
    return result;
}

static const char* op_name(kind_t kind) {
    switch (kind) {
    case T_SEMI: return ";";
    case T_AND: return "&&";
    case T_OR: return "||";
    case T_BG: return "&";
    default: return "?";
    }
}

static int run_line(const char* line) {
    tokens_t* ts = malloc(sizeof *ts);
    if (!ts) return status = fail("out of memory");
    // Look over the whole line first: a missing quote or a pipe anywhere
    // stops it before any of it runs.
    const char* p = line;
    int bad = lex(&p, ts, 0);
    for (int i = 0; !bad && i < ts->n; i++) {
        if (ts->t[i].kind == T_PIPE || ts->t[i].kind == T_REDIR) {
            fail("'%s': pipes and redirection aren't supported yet (programs can only print to the console)", ts->t[i].text);
            bad = 1;
        }
    }
    free_tokens(ts);
    if (bad) {
        free(ts);
        return status = 1;
    }

    kind_t joined_by = T_SEMI;  // how the next command is joined to the last
    p = line;
    while (*p) {
        if (lex(&p, ts, 1) || expand_aliases(ts)) {
            free_tokens(ts);
            status = 1;
            break;
        }
        // One command and the operator after it (an alias can make more).
        int i = 0, error = 0;
        while (i < ts->n) {
            int start = i;
            while (i < ts->n && ts->t[i].kind == T_WORD) i++;
            int end = i;
            kind_t op = i < ts->n ? ts->t[i].kind : T_SEMI;
            if (i < ts->n) i++;
            if (start == end) {
                fail("syntax error near '%s'", op_name(op));
                status = error = 1;
                break;
            }
            int skip = (joined_by == T_AND && status != 0) || (joined_by == T_OR && status == 0);
            if (!skip) status = run_command(&ts->t[start], end - start, op == T_BG);
            joined_by = op;
        }
        free_tokens(ts);
        if (error) break;
    }
    if (joined_by == T_AND || joined_by == T_OR) {
        fail("syntax error: '%s' at the end of the line", op_name(joined_by));
        status = 1;
    }
    free(ts);
    return status;
}

static int source_depth = 0;

static int run_file(const char* name) {
    char path[256];
    abs_path(name, path, sizeof path);
    size_t len;
    char* data = fs_read_all(path, &len);
    if (!data) return fail("%s: no such file", name);
    if (source_depth == 8) {
        free(data);
        return fail("%s: files source each other too deeply", name);
    }
    source_depth++;
    char* save;
    for (char* line = strtok_r(data, "\n", &save); line; line = strtok_r(NULL, "\n", &save)) {
        size_t n = strlen(line);
        if (n && line[n - 1] == '\r') line[n - 1] = '\0';
        run_line(line);
    }
    source_depth--;
    free(data);
    return status;
}

// ---- line editing ----------------------------------------------------------------

static int history_pos;     // == history_count when on the new line
static char saved_input[sizeof input];

static void set_input(const char* s) {
    input_len = (int)strnlen(s, sizeof input - 1);
    memcpy(input, s, input_len);
    input[input_len] = '\0';
    input_cursor = input_len;
}

static void insert_text(const char* s) {
    int n = (int)strlen(s);
    if (input_len + n >= (int)sizeof input) return;
    memmove(input + input_cursor + n, input + input_cursor, input_len - input_cursor + 1);
    memcpy(input + input_cursor, s, n);
    input_len += n;
    input_cursor += n;
}

static void add_history(const char* line) {
    while (*line == ' ') line++;
    if (!*line) return;
    if (history_count && !strcmp(history[history_count - 1], line)) return;
    if (history_count == MAX_HISTORY) {
        free(history[0]);
        memmove(history, history + 1, (MAX_HISTORY - 1) * sizeof history[0]);
        history_count--;
    }
    history[history_count++] = strdup(line);
}

#define MAX_MATCHES 128

// Completes the word before the cursor: a command name for the first word,
// else a path. One match is filled in; several fill in what they share and
// are listed.
static void complete(void) {
    int start = input_cursor;
    while (start > 0 && !strchr(" \t;&|", input[start - 1])) start--;
    int p = start;
    while (p > 0 && input[p - 1] == ' ') p--;
    int command = p == 0 || strchr(";&|", input[p - 1]);

    char word[256];
    int wlen = input_cursor - start;
    if (wlen >= (int)sizeof word) return;
    memcpy(word, input + start, wlen);
    word[wlen] = '\0';

    char* matches[MAX_MATCHES];
    int n = 0;
    const char* prefix = word;
    char dir_part[256] = "";
    if (command && !strchr(word, '/')) {
        for (int i = 0; i < BUILTIN_COUNT && n < MAX_MATCHES; i++)
            if (!strncmp(builtins[i].name, word, wlen) && builtins[i].name[0] != '.') matches[n++] = strdup(builtins[i].name);
        for (int i = 0; i < alias_count && n < MAX_MATCHES; i++)
            if (!strncmp(aliases[i].name, word, wlen)) matches[n++] = strdup(aliases[i].name);
        int count;
        fs_entry_t* entries = fs_list("/bin", &count);
        for (int i = 0; entries && i < count && n < MAX_MATCHES; i++) {
            if (!ends_with(entries[i].name, ".elf.b64") || strncmp(entries[i].name, word, wlen)) continue;
            entries[i].name[strlen(entries[i].name) - 8] = '\0';
            matches[n++] = strdup(entries[i].name);
        }
        free(entries);
    } else {
        const char* slash = strrchr(word, '/');
        if (slash) {
            memcpy(dir_part, word, slash - word + 1);
            dir_part[slash - word + 1] = '\0';
            prefix = slash + 1;
        }
        char dir[256];
        abs_path(dir_part[0] ? dir_part : ".", dir, sizeof dir);
        int count;
        fs_entry_t* entries = fs_list(dir, &count);
        size_t plen = strlen(prefix);
        for (int i = 0; entries && i < count && n < MAX_MATCHES; i++) {
            if (entries[i].name[0] == '.' && prefix[0] != '.') continue;
            if (strncmp(entries[i].name, prefix, plen)) continue;
            char m[260];
            snprintf(m, sizeof m, "%s%s", entries[i].name, entries[i].is_dir ? "/" : "");
            matches[n++] = strdup(m);
        }
        free(entries);
    }
    if (n == 0) return;

    // Remove duplicates (an alias and a program can share a name).
    qsort(matches, n, sizeof matches[0], compare_names);
    int unique = 0;
    for (int i = 0; i < n; i++) {
        if (unique && !strcmp(matches[unique - 1], matches[i])) free(matches[i]);
        else matches[unique++] = matches[i];
    }
    n = unique;

    size_t shared = strlen(matches[0]);
    for (int i = 1; i < n; i++) {
        size_t k = 0;
        while (k < shared && matches[i][k] == matches[0][k]) k++;
        shared = k;
    }
    size_t typed = strlen(prefix);
    if (shared > typed) {
        char add[256];
        size_t extra = shared - typed;
        if (extra >= sizeof add) extra = sizeof add - 1;
        memcpy(add, matches[0] + typed, extra);
        add[extra] = '\0';
        insert_text(add);
    }
    if (n == 1) {
        if (!ends_with(matches[0], "/")) insert_text(" ");
    } else if (shared <= typed) {
        // Nothing more to fill in: show the choices above the prompt.
        int width = 0;
        for (int i = 0; i < n; i++)
            if ((int)strlen(matches[i]) > width) width = (int)strlen(matches[i]);
        width += 2;
        int columns = COLS / width > 0 ? COLS / width : 1;
        for (int i = 0; i < n; i++) {
            put(matches[i], ends_with(matches[i], "/") ? C_DIR : C_TEXT);
            if ((i + 1) % columns == 0 || i == n - 1) put("\n", C_TEXT);
            else for (int k = (int)strlen(matches[i]); k < width; k++) put(" ", C_TEXT);
        }
    }
    for (int i = 0; i < n; i++) free(matches[i]);
}

// Handles one key. Returns 1 when Enter finishes the line.
static int on_key(int key) {
    if (key == '\n') return 1;
    switch (key) {
    case 8:
        if (input_cursor > 0) {
            memmove(input + input_cursor - 1, input + input_cursor, input_len - input_cursor + 1);
            input_cursor--;
            input_len--;
        }
        break;
    case CANVAS_KEY_DELETE:
        if (input_cursor < input_len) {
            memmove(input + input_cursor, input + input_cursor + 1, input_len - input_cursor);
            input_len--;
        }
        break;
    case CANVAS_KEY_LEFT: if (input_cursor > 0) input_cursor--; break;
    case CANVAS_KEY_RIGHT: if (input_cursor < input_len) input_cursor++; break;
    case CANVAS_KEY_HOME: input_cursor = 0; break;
    case CANVAS_KEY_END: input_cursor = input_len; break;
    case CANVAS_KEY_UP:
        if (history_pos > 0) {
            if (history_pos == history_count) memcpy(saved_input, input, input_len + 1);
            set_input(history[--history_pos]);
        }
        break;
    case CANVAS_KEY_DOWN:
        if (history_pos < history_count) {
            history_pos++;
            set_input(history_pos == history_count ? saved_input : history[history_pos]);
        }
        break;
    case CANVAS_KEY_PAGE_UP: scroll += ROWS - 2; return 0;
    case CANVAS_KEY_PAGE_DOWN: scroll = scroll > ROWS - 2 ? scroll - (ROWS - 2) : 0; return 0;
    case '\t': complete(); break;
    case 27: set_input(""); break;
    default:
        if (key >= 32 && key < 127) {
            char s[2] = { (char)key, '\0' };
            insert_text(s);
        }
    }
    scroll = 0;
    return 0;
}

// Waits for a line, keeping the window alive: blinking the cursor,
// ticking the clock and reporting finished jobs.
static void read_line(void) {
    set_input("");
    history_pos = history_count;
    prompting = 1;
    blink_on = 1;
    blink_at = uptime_ms();
    int dirty = 1, minute = -1;
    uint64_t jobs_at = 0;
    for (;;) {
        canvas_event_t e;
        while (canvas_poll(&win, &e) > 0) {
            if (e.kind == CANVAS_CLOSED) quit();
            if (e.kind != CANVAS_KEY) continue;
            dirty = 1;
            blink_on = 1;
            blink_at = uptime_ms();
            if (on_key(e.key)) {
                prompting = 0;
                return;
            }
        }
        uint64_t now = uptime_ms();
        if (now - blink_at >= 530) {
            blink_on = !blink_on;
            blink_at = now;
            dirty = 1;
        }
        if (now - jobs_at >= 500) {
            jobs_at = now;
            if (job_count && check_jobs()) dirty = 1;
            int m = (int)(unix_time() / 60);
            if (m != minute) {
                minute = m;
                dirty = 1;
            }
        }
        if (dirty) {
            render();
            dirty = 0;
        }
        sleep_ms(15);
    }
}

// ---- start -----------------------------------------------------------------------

static void banner(void) {
    put("\n", C_TEXT);
    put_bg(" ksh ", C_DARK, C_ACCENT);
    put_bg(" the KOS shell ", C_WHITE, C_PATH);
    put_bg(" ", C_WHITE, C_PINK);
    put("  type ", C_DIM);
    put("help", C_ACCENT);
    put(" to see what it can do\n\n", C_DIM);
}

static __attribute__((noreturn)) void shell(int argc, char** argv) {
    if (canvas_open(&win, "Shell", WIN_W, WIN_H) < 0) {
        printf("ksh: couldn't open a window\n");
        exit();
    }
    char start[256];
    if (fs_cwd(start, sizeof start) > 0) abs_path(start, cwd, sizeof cwd);
    strcpy(old_cwd, cwd);
    banner();
    render();
    if (is_file(RC_FILE)) run_file(RC_FILE);
    if (argc > 0) run_file(argv[0]);
    for (;;) {
        end_line();
        read_line();
        // Keep what was typed, after its prompt, above the output.
        prompt_to(put_bg);
        put(input, C_TEXT);
        new_line();
        add_history(input);
        run_line(input);
        end_line();
    }
}

#define MAX_START_ARGS 8

void _start(void) {
    char* argv[MAX_START_ARGS];
    int argc = get_argv(argv, MAX_START_ARGS);
    if (argc > 0 && !strcmp(argv[0], "--window")) shell(argc - 1, argv + 1);

    // Started from a shell, which waits for us: start the real ksh as our
    // own child and end, so the Terminal gets its prompt back.
    char args[512] = "--window";
    for (int i = 0; i < argc; i++) {
        strncat(args, " ", sizeof args - strlen(args) - 1);
        strncat(args, argv[i], sizeof args - strlen(args) - 1);
    }
    if (spawn(SELF, args) < 0) printf("ksh: couldn't start: %s\n", last_error());
    exit();
}
