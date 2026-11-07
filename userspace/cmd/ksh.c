// ksh: the KOS shell, in a window of its own.
//
//     ksh [script]
//
// Typed in the Terminal, it opens the Shell window and gives the Terminal
// back. Builtins (cd, ls, cat, echo, alias, ...) and programs in /bin both
// print in the window; while a program runs, lines typed go to its input
// (Ctrl+D ends it, Ctrl+C ends the program).
//
// A line can chain commands with ; && || and end one in & to run it in the
// background. `a | b` feeds what a prints to b; `< file` reads input from
// a file, `> file` and `>> file` write or add output to one, `2> file`
// sends errors there and `2>&1` sends them with the output. $NAME, ${NAME}
// and $? (the last command's exit status) expand, "double" quotes keep
// spaces and still expand, 'single' quotes keep everything, \ escapes one
// character, # starts a comment and * ? match file names. Up / Down go
// through history, Tab completes commands and paths, Page Up / Page Down
// scroll. /.kshrc runs at the start, and `source` runs any file of commands.
//
// ksh keeps its own current directory: relative paths given to programs
// are made absolute first. Builtins don't read input, so `... | cat` shows
// nothing; programs (grep, wc...) do.

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

// While a builtin's output goes to a pipe or a file, what it prints is
// collected here instead of shown. (fail()'s errors still show.)
static int capturing;
static char* captured;
static size_t captured_len, captured_cap;

static void capture_char(char c) {
    if (captured_len == captured_cap) {
        size_t cap = captured_cap ? captured_cap * 2 : 4096;
        char* bigger = realloc(captured, cap);
        if (!bigger) return;
        captured = bigger;
        captured_cap = cap;
    }
    captured[captured_len++] = c;
}

static void put_char(char c, int fg, int bg) {
    if (capturing) {
        capture_char(c);
        return;
    }
    if (c == '\r') return;
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
    if (capturing) {
        if (captured_len && captured[captured_len - 1] != '\n') capture_char('\n');
        return;
    }
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
    int was_capturing = capturing;
    capturing = 0;
    end_line();
    put("ksh: ", C_ERR);
    put(buf, C_ERR);
    put("\n", C_ERR);
    capturing = was_capturing;
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
static int prompting;       // waiting for a line (see read_line)

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
#define MAX_STAGES 8
typedef struct {
    long ids[MAX_STAGES];   // its programs, one per stage of its pipeline
    int count;
    long out;               // what they print, to show
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
        // 2> 2>> 2>&1: where errors go.
        if (c == '2' && p[1] == '>') {
            const char* op = p[2] == '&' && p[3] == '1' ? "2>&1" : p[2] == '>' ? "2>>" : "2>";
            if (add_token(ts, T_REDIR, op, 0, 0, 0)) return 1;
            p += strlen(op);
            continue;
        }
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
            // A pipeline and its redirections are one command.
            if (one && !(c == '|' && !two) && c != '<' && c != '>') {
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
        if (t->kind == T_REDIR) {
            // The file name after it isn't a command.
            if (strcmp(t->text, "2>&1")) i++;
            continue;
        }
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

// Shows what's waiting in pipe `fd`: 1 if there was something, 0 if not
// yet, -1 once it's over (every program writing to it has ended).
static int show_output(long fd) {
    char buf[1024];
    int shown = 0;
    for (;;) {
        long n = fd_read_now(fd, buf, sizeof buf);
        if (n == -2) return shown;
        if (n <= 0) return -1;
        for (long i = 0; i < n; i++) put_char(buf[i], C_TEXT, C_NONE);
        shown = 1;
    }
}

static int any_running(const long* ids, int count) {
    for (int i = 0; i < count; i++)
        if (ids[i] >= 0 && proc_running(ids[i])) return 1;
    return 0;
}

// Shows background jobs' output, and reports the ones that have ended
// and forgets them. Returns whether anything was shown.
static int check_jobs(void) {
    int changed = 0;
    for (int i = 0; i < job_count;) {
        job_t* j = &jobs[i];
        int out = j->out >= 0 ? show_output(j->out) : -1;
        if (out > 0) changed = 1;
        if (out >= 0 || any_running(j->ids, j->count)) {
            i++;
            continue;
        }
        if (j->out >= 0) fd_close(j->out);
        long last = j->ids[j->count - 1];
        long code = last >= 0 ? proc_wait(last) : 127;
        end_line();
        if (code) print(C_DIM, "[%d] done  %s  (exit %ld)\n", j->number, j->name, code);
        else print(C_DIM, "[%d] done  %s\n", j->number, j->name);
        jobs[i] = jobs[--job_count];
        changed = 1;
    }
    // Output that came in while ksh waits at the prompt goes above it.
    if (changed && prompting) end_line();
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
    exit(0);
}

// Commands only the kernel shell (the Terminal) has.
static const char* kernel_commands[] = {
    "k", "kai", "net", "ping", "fetch", "browse", "open", "stats", "log", "dmesg",
    "mmu", "load", "time", "run", "bg", "hash",
    "heap_remain", NULL,
};

static int is_kernel_command(const char* name) {
    for (int i = 0; kernel_commands[i]; i++)
        if (!strcmp(kernel_commands[i], name)) return 1;
    return 0;
}

// Where program `name` is: a path, or /bin/<name>.elf.b64, or else
// /programs/<name>.elf.b64. Returns 0 if it's found.
static int find_program(const char* name, char* out, size_t size) {
    if (strchr(name, '/') || is_program(name)) {
        abs_path(name, out, size);
        return is_file(out) ? 0 : -1;
    }
    snprintf(out, size, "/bin/%s.elf.b64", name);
    if (is_file(out)) return 0;
    snprintf(out, size, "/programs/%s.elf.b64", name);
    return is_file(out) ? 0 : -1;
}

// The arguments after a program's name as the one string programs get,
// split at spaces. Programs look up relative paths from the Terminal's
// directory, not ours, so paths that exist are made absolute.
static int program_args(const args_t* a, char* out, size_t size) {
    size_t len = 0;
    out[0] = '\0';
    for (int i = 1; i < a->n; i++) {
        const char* arg = a->v[i];
        if (strchr(arg, ' ')) return fail("%s: arguments with spaces can't be passed to programs yet", a->v[0]);
        char full[256];
        if (arg[0] != '-' && arg[0] != '/') {
            abs_path(arg, full, sizeof full);
            if (is_file(full) || is_dir(full)) arg = full;
        }
        int n = snprintf(out + len, size - len, "%s%s", len ? " " : "", arg);
        if (n < 0 || len + n >= size) return fail("%s: arguments are too long", a->v[0]);
        len += n;
    }
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
        } else if (shown && capturing) {
            // One a line, for grep and the like.
            for (int k = 0; k < shown; k++) print(C_TEXT, "%s%s\n", entries[k].name, entries[k].is_dir ? "/" : "");
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
    for (int i = 0; i < job_count; i++) print(C_TEXT, "[%d] %ld  %s\n", jobs[i].number, jobs[i].ids[jobs[i].count - 1], jobs[i].name);
    return 0;
}

static int b_kill(int argc, char** argv) {
    if (argc < 2) return fail("kill: usage: kill <id> or kill %%<job>");
    int result = 0;
    for (int i = 1; i < argc; i++) {
        long id = -1;
        if (argv[i][0] == '%') {
            int number = atoi(argv[i] + 1);
            job_t* job = NULL;
            for (int k = 0; k < job_count; k++)
                if (jobs[k].number == number) job = &jobs[k];
            if (!job) {
                result = fail("kill: %s: no such job", argv[i]);
                continue;
            }
            // Every program in its pipeline.
            for (int k = 0; k < job->count; k++)
                if (job->ids[k] >= 0 && proc_running(job->ids[k]) && proc_kill(job->ids[k]) < 0)
                    result = fail("kill: %s: %s", argv[i], last_error());
            continue;
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
        "  They print here, and get the lines typed while they run (Ctrl+D ends\n"
        "  the input, Ctrl+C the program). Add & to run one in the background.\n", C_TEXT);
    put("Lines\n", C_ACCENT);
    put("  a ; b   a && b   a || b   prog &   NAME=value   $NAME ${NAME} $?\n"
        "  a | b   < in   > out   >> out   2> errors   2>&1\n"
        "  \"double\" and 'single' quotes, \\ escapes, * ? globs, # comments\n", C_TEXT);
    put("Keys\n", C_ACCENT);
    put("  Up/Down history, Tab completes, PgUp/PgDn scroll, Esc clears the line\n", C_TEXT);
    return 0;
}

// ---- executing lines -----------------------------------------------------------

// One command of a pipeline: its words, and where its input and output go
// (-1: the pipeline's usual).
typedef struct {
    args_t args;
    builtin_fn builtin;
    char path[256];         // the program, if it isn't a builtin
    long in, out, err;      // descriptors from < > >> 2> 2>>
    int err_to_out;         // 2>&1
} stage_t;

static void close_fd(long* fd) {
    if (*fd >= 0) fd_close(*fd);
    *fd = -1;
}

// Opens a redirection's file for stage `st`. Returns 0, or 1 after
// saying why not.
static int redirect(stage_t* st, const char* op, const char* name) {
    char path[256];
    abs_path(name, path, sizeof path);
    int to_err = op[0] == '2';
    int append = !strcmp(op + to_err, ">>");
    int flags = op[0] == '<' ? FD_READ : FD_WRITE | FD_CREATE | (append ? FD_APPEND : FD_TRUNCATE);
    long* fd = op[0] == '<' ? &st->in : to_err ? &st->err : &st->out;
    close_fd(fd);
    if (is_dir(path)) return fail("%s: is a folder", name);
    *fd = fd_open(path, flags);
    if (*fd < 0) return fail("%s: %s", name, op[0] == '<' && !is_file(path) ? "no such file" : last_error());
    return 0;
}

// Turns a command's words into its arguments and redirections. Leading
// NAME=value words set variables. Returns 0, or 1 after saying why not.
static int parse_stage(token_t* words, int n, stage_t* st) {
    int i = 0;
    while (i < n && words[i].kind == T_WORD && words[i].assign) i++;
    for (int k = 0; k < i; k++) {
        char* text = words[k].text;
        text[words[k].assign - 1] = '\0';
        set_var(text, text + words[k].assign);
    }
    for (; i < n; i++) {
        token_t* t = &words[i];
        if (t->kind == T_REDIR) {
            if (!strcmp(t->text, "2>&1")) {
                st->err_to_out = 1;
                continue;
            }
            if (i + 1 == n || words[i + 1].kind != T_WORD) return fail("syntax error: '%s' needs a file name", t->text);
            if (redirect(st, t->text, words[++i].text)) return 1;
            continue;
        }
        if (t->glob ? add_glob(&st->args, t->text) : add_arg(&st->args, t->text)) return 1;
    }
    return 0;
}

// Runs a builtin with its output collected (see `capturing`). Returns its
// status; the output is in captured[0..captured_len].
static int run_captured(builtin_fn fn, args_t* a) {
    captured_len = 0;
    capturing = 1;
    int result = fn(a->n, a->v);
    capturing = 0;
    return result;
}

// Writes all of `data` to file descriptor `fd` (a file, so it doesn't
// wait), and says if that failed.
static int write_all(long fd, const char* data, size_t len, const char* what) {
    while (len) {
        long n = fd_write(fd, data, len);
        if (n <= 0) return fail("%s: %s", what, last_error());
        data += n;
        len -= n;
    }
    return 0;
}

// A line typed while a program runs, for its input.
static char typed[512];
static int typed_len;

// Handles a key typed while a pipeline runs: it goes to the input pipe
// `*in` a line at a time (Ctrl+D closes it). Ctrl+C ends the pipeline's
// programs. Returns 1 if something changed on screen.
static int running_key(int key, long* in, const long* ids, int count) {
    if (key == 3) {
        put("^C\n", C_DIM);
        for (int i = 0; i < count; i++)
            if (ids[i] >= 0) proc_kill(ids[i]);
        return 1;
    }
    if (key == CANVAS_KEY_PAGE_UP) {
        scroll += ROWS - 2;
        return 1;
    }
    if (key == CANVAS_KEY_PAGE_DOWN) {
        scroll = scroll > ROWS - 2 ? scroll - (ROWS - 2) : 0;
        return 1;
    }
    if (*in < 0) return 0;
    scroll = 0;
    if (key == 4) {
        close_fd(in);
        return 0;
    }
    if (key == '\n') {
        typed[typed_len++] = '\n';
        put_char('\n', C_TEXT, C_NONE);
        // A program that has stopped reading just doesn't get it.
        fd_write_now(*in, typed, typed_len);
        typed_len = 0;
        return 1;
    }
    if (key == 8 && typed_len > 0) {
        typed_len--;
        if (open_line()->len) open_line()->len--;
        return 1;
    }
    if (key >= 32 && key < 127 && typed_len < (int)sizeof typed - 1) {
        typed[typed_len++] = (char)key;
        put_char((char)key, C_ACCENT, C_NONE);
        return 1;
    }
    return 0;
}

// Runs a pipeline: the commands in `words` (n tokens) joined by |, each
// with its redirections. Programs print into the window (through a pipe
// ksh reads), get typed lines as input, and run together; a builtin runs
// in ksh first, with its output collected and fed on. Returns the last
// command's exit status.
static int run_pipeline(token_t* words, int n, int background) {
    stage_t st[MAX_STAGES];
    int count = 0, result = 0;
    int start = 0;
    for (int i = 0; i <= n && !result; i++) {
        if (i < n && words[i].kind != T_PIPE) continue;
        if (count == MAX_STAGES) {
            result = fail("too many commands in one pipeline (at most %d)", MAX_STAGES);
            break;
        }
        stage_t* s = &st[count++];
        memset(s, 0, sizeof *s);
        s->in = s->out = s->err = -1;
        if (i == start) {
            result = fail("syntax error near '|'");
            break;
        }
        result = parse_stage(&words[start], i - start, s);
        start = i + 1;
    }
    // Only NAME=value: nothing to run.
    if (!result && count == 1 && st[0].args.n == 0 && st[0].in < 0 && st[0].out < 0 && st[0].err < 0) {
        free_args(&st[0].args);
        return 0;
    }
    for (int i = 0; i < count && !result; i++) {
        stage_t* s = &st[i];
        if (s->args.n == 0) {
            result = fail("syntax error: a redirection without a command");
        } else if ((s->builtin = find_builtin(s->args.v[0]))) {
            if (background) result = fail("%s: builtins can't run in the background", s->args.v[0]);
        } else if (find_program(s->args.v[0], s->path, sizeof s->path)) {
            if (is_kernel_command(s->args.v[0]))
                fail("%s: that's a kernel shell command; run it in the Terminal", s->args.v[0]);
            else
                fail("%s: command not found", s->args.v[0]);
            result = 127;
        }
    }
    if (result) {
        for (int i = 0; i < count; i++) {
            close_fd(&st[i].in);
            close_fd(&st[i].out);
            close_fd(&st[i].err);
            free_args(&st[i].args);
        }
        return result;
    }

    // A lone builtin printing to the window, as it always did.
    if (count == 1 && st[0].builtin && st[0].out < 0) {
        close_fd(&st[0].in);
        close_fd(&st[0].err);
        result = st[0].builtin(st[0].args.n, st[0].args.v);
        free_args(&st[0].args);
        return result;
    }

    // pipes[i] joins stage i to i + 1; `shown` carries what the programs
    // print to the window, and `input` what's typed to the first one.
    int pipes[MAX_STAGES][2];
    int shown[2] = { -1, -1 }, input[2] = { -1, -1 };
    int made = 0;
    for (; made < count - 1; made++)
        if (fd_pipe(pipes[made]) < 0) break;
    int ok = made == count - 1 && fd_pipe(shown) == 0 && fd_pipe(input) == 0;

    long ids[MAX_STAGES];
    int statuses[MAX_STAGES];
    // What a builtin in the middle of the pipeline printed, still to feed
    // to the next command.
    char* feed = NULL;
    size_t feed_len = 0, fed = 0;
    long feed_fd = -1;
    for (int i = 0; i < count; i++) {
        ids[i] = -1;
        statuses[i] = 0;
        if (!ok) continue;
        stage_t* s = &st[i];
        long out = s->out >= 0 ? s->out : i == count - 1 ? shown[1] : pipes[i][1];
        if (s->builtin && i == count - 1 && s->out < 0) {
            statuses[i] = s->builtin(s->args.n, s->args.v);
        } else if (s->builtin) {
            statuses[i] = run_captured(s->builtin, &s->args);
            if (s->out >= 0) {
                if (write_all(out, captured, captured_len, s->args.v[0])) statuses[i] = 1;
            } else if (captured_len) {
                free(feed);
                feed = malloc(captured_len);
                if (feed) memcpy(feed, captured, captured_len);
                feed_len = feed ? captured_len : 0;
                fed = 0;
                feed_fd = pipes[i][1];
                pipes[i][1] = -1;
            }
            continue;
        }
        char args[1024];
        if (program_args(&s->args, args, sizeof args)) {
            statuses[i] = 1;
            continue;
        }
        int fds[3];
        fds[0] = s->in >= 0 ? s->in : i == 0 ? input[0] : pipes[i - 1][0];
        fds[1] = out;
        fds[2] = s->err_to_out ? out : s->err >= 0 ? s->err : shown[1];
        ids[i] = spawn_io(s->path, args, fds);
        if (ids[i] < 0) {
            fail("%s: %s", s->args.v[0], last_error());
            statuses[i] = 126;
        }
    }
    if (!ok) fail("couldn't make pipes: %s", last_error());
    char name[32];
    const char* first = st[0].args.v[0];
    snprintf(name, sizeof name, "%s%s", strrchr(first, '/') ? strrchr(first, '/') + 1 : first, count > 1 ? " | ..." : "");
    int input_redirected = st[0].in >= 0;

    // Only the programs hold their ends now (a builtin's stdin goes
    // unread, so whatever writes to it stops). Background jobs get no
    // input.
    for (int i = 0; i < count; i++) {
        close_fd(&st[i].in);
        close_fd(&st[i].out);
        close_fd(&st[i].err);
        free_args(&st[i].args);
    }
    for (int i = 0; i < made; i++) {
        long r = pipes[i][0], w = pipes[i][1];
        close_fd(&r);
        if (w >= 0) close_fd(&w);
    }
    long show_fd = shown[0], in_fd = input[1], unused;
    unused = shown[1];
    close_fd(&unused);
    unused = input[0];
    close_fd(&unused);
    if (background || input_redirected) close_fd(&in_fd);
    if (!ok) {
        close_fd(&show_fd);
        close_fd(&in_fd);
        free(feed);
        return 1;
    }

    if (background) {
        if (job_count == MAX_JOBS) {
            print(C_WARN, "ksh: too many jobs to track; %s runs untracked\n", name);
            close_fd(&show_fd);
            return 0;
        }
        job_t* j = &jobs[job_count++];
        memcpy(j->ids, ids, sizeof ids);
        j->count = count;
        j->out = show_fd;
        j->number = next_job++;
        snprintf(j->name, sizeof j->name, "%s", name);
        print(C_DIM, "[%d] %ld  %s\n", j->number, ids[count - 1], j->name);
        return 0;
    }

    // Show what they print and pass on what's typed until every program
    // has ended and the output is all shown.
    typed_len = 0;
    int out_done = show_fd < 0;
    uint64_t jobs_at = 0;
    for (;;) {
        int busy = 0;
        if (!out_done) {
            int got = show_output(show_fd);
            if (got < 0) out_done = 1;
            busy |= got != 0;
        }
        if (feed_fd >= 0) {
            long w = fd_write_now(feed_fd, feed + fed, feed_len - fed);
            if (w > 0) fed += w;
            if (w == -1 || fed == feed_len) close_fd(&feed_fd);
            busy |= w > 0;
        }
        canvas_event_t e;
        while (canvas_poll(&win, &e) > 0) {
            if (e.kind == CANVAS_CLOSED) {
                for (int i = 0; i < count; i++)
                    if (ids[i] >= 0) proc_kill(ids[i]);
                quit();
            }
            if (e.kind == CANVAS_KEY) busy |= running_key(e.key, &in_fd, ids, count);
        }
        uint64_t now = uptime_ms();
        if (now - jobs_at >= 500) {
            jobs_at = now;
            if (job_count) busy |= check_jobs();
        }
        if (out_done && feed_fd < 0 && !any_running(ids, count)) break;
        if (busy) render();
        else sleep_ms(10);
    }
    close_fd(&show_fd);
    close_fd(&in_fd);
    free(feed);
    for (int i = 0; i < count; i++)
        if (ids[i] >= 0) statuses[i] = (int)proc_wait(ids[i]);
    return statuses[count - 1];
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
    // Look over the whole line first: a missing quote anywhere stops it
    // before any of it runs.
    const char* p = line;
    int bad = lex(&p, ts, 0);
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
        // One pipeline and the operator after it (an alias can make more).
        int i = 0, error = 0;
        while (i < ts->n) {
            int start = i;
            while (i < ts->n && (ts->t[i].kind == T_WORD || ts->t[i].kind == T_PIPE || ts->t[i].kind == T_REDIR)) i++;
            int end = i;
            kind_t op = i < ts->n ? ts->t[i].kind : T_SEMI;
            if (i < ts->n) i++;
            if (start == end) {
                fail("syntax error near '%s'", op_name(op));
                status = error = 1;
                break;
            }
            int skip = (joined_by == T_AND && status != 0) || (joined_by == T_OR && status == 0);
            if (!skip) status = run_pipeline(&ts->t[start], end - start, op == T_BG);
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
        static const char* const dirs[] = { "/bin", "/programs" };
        for (int d = 0; d < 2; d++) {
            int count;
            fs_entry_t* entries = fs_list(dirs[d], &count);
            for (int i = 0; entries && i < count && n < MAX_MATCHES; i++) {
                if (!ends_with(entries[i].name, ".elf.b64") || strncmp(entries[i].name, word, wlen)) continue;
                entries[i].name[strlen(entries[i].name) - 8] = '\0';
                matches[n++] = strdup(entries[i].name);
            }
            free(entries);
        }
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
        exit(0);
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
    exit(0);
}
