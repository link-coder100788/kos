// edit: a text editor, in a window of its own.
//
//     edit [file]
//
// Typed in the Terminal, it opens the Edit window and gives the Terminal
// back; a file that isn't there yet is made when you save. Type to write,
// arrows / Home / End / Page Up / Page Down to move, click to put the
// cursor somewhere and drag to select (double-click selects a word).
// Ctrl+S saves, Ctrl+F finds (Enter again for the next one), Ctrl+G goes to
// a line, Ctrl+Z undoes and Ctrl+Y redoes, Ctrl+A selects everything,
// Ctrl+X / Ctrl+C / Ctrl+V cut, copy and paste (the whole line if nothing is
// selected) and Ctrl+Q quits, asking first if there are unsaved changes.

#include "../libc/sys.h"
#include "../libc/stdio.h"
#include "../libc/stdlib.h"
#include "../libc/string.h"
#include "../libc/ctype.h"
#include "../libc/canvas.h"
#include <stdarg.h>

#define SELF "/bin/edit.elf.b64"

// ---- look --------------------------------------------------------------------

#define WIN_W 800
#define WIN_H 480
#define HEADER_H 28
#define FOOTER_H 20
#define LINE_H 12
#define CHAR_W 8
#define PAD 8
#define TEXT_TOP (HEADER_H + 6)
#define ROWS ((WIN_H - TEXT_TOP - FOOTER_H - 4) / LINE_H)
#define SAVE_W 50
#define SAVE_X (WIN_W - SAVE_W - 8)

#define BG      0x14122a
#define PANEL   0x1c1a38
#define HEADER  0x0d0b1e
#define TEXT    0xd6dcf0
#define DIM     0x6b7193
#define EXEC    0x5fe3a1
#define ACCENT  0x2ee6d6
#define SELECT  0x5b3fd9
#define ERR     0xff5d7a
#define WARN    0xffc857

#define TAB 4
#define DOUBLE_CLICK_MS 400
#define UNDO_MAX 200
#define SCROLL_LINES 3

// ---- state -------------------------------------------------------------------

static canvas_t win;
static char path[256];      // "" until saved for the first time
static char dir[256] = "/"; // where a new file's name is relative to
static int crlf;            // the file had \r\n line ends; saving keeps them

typedef struct {
    char* s;
    int len, cap;
} line_t;

static line_t* lines;
static int nlines, lines_cap;

typedef struct {
    int line, col;
} pos_t;

static pos_t cur;
static pos_t anchor;        // the other end of the selection, if has_anchor
static int has_anchor;
static int dragging;
static int want_vcol = -1;  // the column up / down keep to, across short lines
static int top, left;       // the first line and (screen) column shown

static char* clipboard;
static int clip_len, clip_lines; // clip_lines: a whole line, pasted above the cursor's

// A question in the footer that the next keys answer.
enum { ASK_NONE, ASK_FIND, ASK_LINE, ASK_SAVE_AS, ASK_QUIT };
static int asking;
static char answer[128];
static char found[128];     // what the last find looked for
static char status[200];
static uint32_t status_color = DIM;

static void set_status(uint32_t color, const char* format, ...) __attribute__((format(printf, 2, 3)));
static void set_status(uint32_t color, const char* format, ...) {
    va_list args;
    va_start(args, format);
    vsnprintf(status, sizeof status, format, args);
    va_end(args);
    status_color = color;
}

// ---- lines -------------------------------------------------------------------

static void reserve(line_t* l, int need) {
    if (need <= l->cap) return;
    int cap = l->cap ? l->cap : 16;
    while (cap < need) cap *= 2;
    l->s = realloc(l->s, cap);
    l->cap = cap;
}

// Makes an empty line at i, moving the ones from i down.
static void add_line(int i) {
    if (nlines == lines_cap) {
        lines_cap = lines_cap ? lines_cap * 2 : 64;
        lines = realloc(lines, lines_cap * sizeof *lines);
    }
    memmove(&lines[i + 1], &lines[i], (nlines - i) * sizeof *lines);
    lines[i] = (line_t){ 0 };
    nlines++;
}

static void remove_lines(int i, int n) {
    for (int k = i; k < i + n; k++) free(lines[k].s);
    memmove(&lines[i], &lines[i + n], (nlines - i - n) * sizeof *lines);
    nlines -= n;
}

static void append(line_t* l, const char* s, int n) {
    reserve(l, l->len + n);
    memcpy(l->s + l->len, s, n);
    l->len += n;
}

static int before(pos_t a, pos_t b) {
    return a.line < b.line || (a.line == b.line && a.col < b.col);
}

static int same(pos_t a, pos_t b) {
    return a.line == b.line && a.col == b.col;
}

// Puts text (which may have '\n's) at `at`. Returns where it ends.
static pos_t insert_raw(pos_t at, const char* text, int len) {
    line_t* l = &lines[at.line];
    int tail_len = l->len - at.col;
    char* tail = malloc(tail_len + 1);
    memcpy(tail, l->s + at.col, tail_len);
    l->len = at.col;
    int line = at.line;
    for (int i = 0, start = 0; i <= len; i++) {
        if (i < len && text[i] != '\n') continue;
        append(&lines[line], text + start, i - start);
        if (i < len) {
            add_line(++line);
        }
        start = i + 1;
    }
    pos_t end = { line, lines[line].len };
    append(&lines[line], tail, tail_len);
    free(tail);
    return end;
}

// Removes the text from a to b (a before b).
static void delete_raw(pos_t a, pos_t b) {
    line_t* l = &lines[a.line];
    if (a.line == b.line) {
        memmove(l->s + a.col, l->s + b.col, l->len - b.col);
        l->len -= b.col - a.col;
        return;
    }
    l->len = a.col;
    append(l, lines[b.line].s + b.col, lines[b.line].len - b.col);
    remove_lines(a.line + 1, b.line - a.line);
}

// The text from a to b, '\n' between lines, NUL after; free() it.
static char* text_of(pos_t a, pos_t b, int* len) {
    int n = 0;
    for (int i = a.line; i <= b.line; i++) {
        int from = i == a.line ? a.col : 0, to = i == b.line ? b.col : lines[i].len;
        n += to - from + (i < b.line);
    }
    char* out = malloc(n + 1);
    int k = 0;
    for (int i = a.line; i <= b.line; i++) {
        int from = i == a.line ? a.col : 0, to = i == b.line ? b.col : lines[i].len;
        memcpy(out + k, lines[i].s + from, to - from);
        k += to - from;
        if (i < b.line) out[k++] = '\n';
    }
    out[k] = '\0';
    *len = n;
    return out;
}

// The screen column of col in a line, with tabs taken to the next stop.
static int vcol(int line, int col) {
    int v = 0;
    for (int i = 0; i < col && i < lines[line].len; i++)
        v = lines[line].s[i] == '\t' ? (v / TAB + 1) * TAB : v + 1;
    return v;
}

// The column nearest screen column v.
static int col_at(int line, int v) {
    int x = 0;
    for (int i = 0; i < lines[line].len; i++) {
        int next = lines[line].s[i] == '\t' ? (x / TAB + 1) * TAB : x + 1;
        if (v < next) return v - x <= next - v - 1 ? i : i + 1;
        x = next;
    }
    return lines[line].len;
}

// ---- undo --------------------------------------------------------------------
// Every change is an insert or a delete of some text, which undo reverses.
// Letters typed one after another (and Backspaces) join into one change.

typedef struct {
    int insert;
    pos_t at, end;      // where the text starts and (once inserted) ends
    char* text;
    int len;
    pos_t cursor;       // the cursor before the change
    int id;             // which change this is, to tell if it's saved
    int typing;
} change_t;

static change_t undos[UNDO_MAX], redos[UNDO_MAX];
static int n_undo, n_redo;
static int next_id = 1, saved_id;
static int no_join;         // the next change starts a new undo step

static int modified(void) {
    return (n_undo ? undos[n_undo - 1].id : 0) != saved_id;
}

static void forget(change_t* stack, int* n) {
    for (int i = 0; i < *n; i++) free(stack[i].text);
    *n = 0;
}

static void push_undo(change_t c) {
    if (n_undo == UNDO_MAX) {
        free(undos[0].text);
        memmove(&undos[0], &undos[1], (UNDO_MAX - 1) * sizeof *undos);
        n_undo--;
    }
    undos[n_undo++] = c;
}

static void insert(const char* text, int len, int typing) {
    pos_t at = cur;
    pos_t end = insert_raw(at, text, len);
    change_t* last = n_undo ? &undos[n_undo - 1] : NULL;
    forget(redos, &n_redo);
    if (typing && !no_join && last && last->insert && last->typing && same(last->end, at)) {
        last->text = realloc(last->text, last->len + len + 1);
        memcpy(last->text + last->len, text, len);
        last->len += len;
        last->end = end;
    } else {
        char* copy = malloc(len + 1);
        memcpy(copy, text, len);
        push_undo((change_t){ 1, at, end, copy, len, cur, next_id++, typing });
    }
    no_join = 0;
    cur = end;
    has_anchor = 0;
    want_vcol = -1;
}

static void delete(pos_t a, pos_t b, int typing) {
    if (!before(a, b)) return;
    int len;
    char* text = text_of(a, b, &len);
    delete_raw(a, b);
    change_t* last = n_undo ? &undos[n_undo - 1] : NULL;
    forget(redos, &n_redo);
    if (typing && !no_join && last && !last->insert && last->typing && same(last->at, b)) {
        // Another Backspace: the text goes before what was deleted so far.
        last->text = realloc(last->text, last->len + len + 1);
        memmove(last->text + len, last->text, last->len);
        memcpy(last->text, text, len);
        last->len += len;
        last->at = a;
        free(text);
    } else {
        push_undo((change_t){ 0, a, a, text, len, cur, next_id++, typing });
    }
    no_join = 0;
    cur = a;
    has_anchor = 0;
    want_vcol = -1;
}

static void undo(void) {
    if (!n_undo) {
        set_status(DIM, "nothing to undo");
        return;
    }
    change_t c = undos[--n_undo];
    if (c.insert) delete_raw(c.at, c.end);
    else c.end = insert_raw(c.at, c.text, c.len);
    cur = c.cursor;
    has_anchor = 0;
    want_vcol = -1;
    redos[n_redo++] = c;
    no_join = 1;
}

static void redo(void) {
    if (!n_redo) {
        set_status(DIM, "nothing to redo");
        return;
    }
    change_t c = redos[--n_redo];
    if (c.insert) {
        c.end = insert_raw(c.at, c.text, c.len);
        cur = c.end;
    } else {
        delete_raw(c.at, c.end);
        cur = c.at;
    }
    has_anchor = 0;
    want_vcol = -1;
    push_undo(c);
    no_join = 1;
}

// ---- selection ---------------------------------------------------------------

static int selection(pos_t* a, pos_t* b) {
    if (!has_anchor || same(anchor, cur)) return 0;
    *a = before(anchor, cur) ? anchor : cur;
    *b = before(anchor, cur) ? cur : anchor;
    return 1;
}

static int delete_selection(void) {
    pos_t a, b;
    if (!selection(&a, &b)) return 0;
    delete(a, b, 0);
    no_join = 1;
    return 1;
}

static void set_clipboard(char* text, int len, int whole_lines) {
    free(clipboard);
    clipboard = text;
    clip_len = len;
    clip_lines = whole_lines;
}

// Copies the selection, or the cursor's whole line if there isn't one.
// Returns where the copied text was, for cut.
static void copy(pos_t* from, pos_t* to) {
    pos_t a, b;
    int len;
    if (selection(&a, &b)) {
        set_clipboard(text_of(a, b, &len), len, 0);
    } else {
        a = (pos_t){ cur.line, 0 };
        b = cur.line + 1 < nlines ? (pos_t){ cur.line + 1, 0 } : (pos_t){ cur.line, lines[cur.line].len };
        char* text = text_of(a, b, &len);
        if (cur.line + 1 == nlines) {
            text = realloc(text, len + 2);
            text[len++] = '\n';
            text[len] = '\0';
        }
        set_clipboard(text, len, 1);
    }
    if (from) *from = a;
    if (to) *to = b;
}

static void cut(void) {
    pos_t a, b;
    copy(&a, &b);
    if (clip_lines && a.line == b.line && a.line > 0) {
        // The last line: take the line end before it instead.
        a = (pos_t){ a.line - 1, lines[a.line - 1].len };
    }
    delete(a, b, 0);
    no_join = 1;
}

static void paste(void) {
    if (!clipboard) {
        set_status(DIM, "nothing to paste: Ctrl+C or Ctrl+X copies first");
        return;
    }
    delete_selection();
    if (clip_lines) {
        // A whole line goes above the cursor's, which keeps its column.
        int col = cur.col;
        cur.col = 0;
        insert(clipboard, clip_len, 0);
        cur.col = col < lines[cur.line].len ? col : lines[cur.line].len;
    } else {
        insert(clipboard, clip_len, 0);
    }
    no_join = 1;
}

// ---- files -------------------------------------------------------------------

static void join_path(char* out, size_t size, const char* folder, const char* name) {
    if (name[0] == '/') strlcpy(out, name, size);
    else if (!strcmp(folder, "/")) snprintf(out, size, "/%s", name);
    else snprintf(out, size, "%s/%s", folder, name);
}

static const char* base_name(const char* p) {
    const char* slash = strrchr(p, '/');
    return slash ? slash + 1 : p;
}

static int is_text(const char* s, long len) {
    long odd = 0;
    for (long i = 0; i < len; i++) {
        unsigned char c = s[i];
        if (c == 0) return 0;
        if (c < 32 && c != '\n' && c != '\r' && c != '\t') odd++;
    }
    return odd * 20 <= len;
}

static void clear_text(void) {
    remove_lines(0, nlines);
    add_line(0);
}

// Reads `path` into the editor. A file that isn't there starts empty.
static void load(void) {
    clear_text();
    if (!path[0]) return;
    if (fs_size(path) < 0) {
        set_status(DIM, "new file: Ctrl+S makes it");
        return;
    }
    size_t len;
    char* data = fs_read_all(path, &len);
    if (!data) {
        set_status(ERR, "can't read %s: %s", path, last_error());
        path[0] = '\0';
        return;
    }
    if (!is_text(data, len)) {
        set_status(ERR, "%s isn't text; Ctrl+S saves under a new name", base_name(path));
        path[0] = '\0';
        free(data);
        return;
    }
    // \r\n becomes \n while editing, and \r\n again when saved.
    size_t n = 0;
    for (size_t i = 0; i < len; i++) {
        if (data[i] == '\r' && i + 1 < len && data[i + 1] == '\n') {
            crlf = 1;
            continue;
        }
        data[n++] = data[i];
    }
    insert_raw((pos_t){ 0, 0 }, data, n);
    free(data);
}

static int save(void) {
    if (!path[0]) {
        answer[0] = '\0';
        asking = ASK_SAVE_AS;
        return -1;
    }
    size_t size = 0;
    for (int i = 0; i < nlines; i++) size += lines[i].len + (crlf ? 2 : 1);
    char* data = malloc(size + 1);
    size_t n = 0;
    for (int i = 0; i < nlines; i++) {
        memcpy(data + n, lines[i].s, lines[i].len);
        n += lines[i].len;
        if (i + 1 < nlines) {
            if (crlf) data[n++] = '\r';
            data[n++] = '\n';
        }
    }
    long r = fs_write(path, data, n);
    free(data);
    if (r < 0) {
        set_status(ERR, "couldn't save %s: %s", path, last_error());
        return -1;
    }
    saved_id = n_undo ? undos[n_undo - 1].id : 0;
    no_join = 1;
    set_status(EXEC, "saved %s (%ld bytes)", path, (long)n);
    return 0;
}

static void save_as(const char* name) {
    if (!name[0]) return;
    char full[256];
    join_path(full, sizeof full, dir, name);
    int count;
    fs_entry_t* is_dir = fs_list(full, &count);
    if (is_dir) {
        free(is_dir);
        set_status(ERR, "%s is a folder", full);
        return;
    }
    strlcpy(path, full, sizeof path);
    save();
}

// ---- finding -----------------------------------------------------------------

// Selects the next match of `found` after the cursor, wrapping round.
static void find_next(void) {
    int n = strlen(found);
    if (!n) return;
    pos_t from = cur;
    for (int k = 0; k <= nlines; k++) {
        int i = (from.line + k) % nlines;
        line_t* l = &lines[i];
        int c = k == 0 ? from.col : 0;
        // Back on the cursor's line after going round: only before it.
        int end = k == nlines ? from.col : l->len;
        for (; c + n <= l->len && c < end + (k == nlines ? 0 : 1); c++) {
            if (strncasecmp(l->s + c, found, n)) continue;
            anchor = (pos_t){ i, c };
            cur = (pos_t){ i, c + n };
            has_anchor = 1;
            want_vcol = -1;
            if (from.line + k >= nlines) set_status(DIM, "back at the top");
            return;
        }
    }
    set_status(WARN, "no \"%s\"", found);
}

// ---- drawing -----------------------------------------------------------------

static int gutter_w(void) {
    int digits = 1;
    for (int n = nlines; n >= 10; n /= 10) digits++;
    if (digits < 3) digits = 3;
    return PAD + (digits + 1) * CHAR_W;
}

static int text_x(void) {
    return gutter_w() + 4;
}

static int cols(void) {
    return (WIN_W - text_x() - PAD - 6) / CHAR_W;
}

// Scrolls so the cursor shows.
static void follow_cursor(void) {
    if (cur.line < top) top = cur.line;
    if (cur.line >= top + ROWS) top = cur.line - ROWS + 1;
    int v = vcol(cur.line, cur.col);
    if (v < left) left = v > 8 ? v - 8 : 0;
    if (v >= left + cols()) left = v - cols() + 8;
}

// Draws text cut to `max` pixels, with ".." if it didn't fit.
static void text_fit(int x, int y, const char* s, uint32_t color, int max) {
    char buf[256];
    int n = max / CHAR_W;
    if (n <= 0) return;
    if ((int)strlen(s) <= n) {
        canvas_text(&win, x, y, s, color, 1);
        return;
    }
    if (n > (int)sizeof buf - 1) n = sizeof buf - 1;
    memcpy(buf, s, n);
    buf[n] = '\0';
    if (n >= 2) buf[n - 1] = buf[n - 2] = '.';
    canvas_text(&win, x, y, buf, color, 1);
}

static void draw_text(void) {
    int tx = text_x(), ncols = cols();
    pos_t a, b;
    int sel = selection(&a, &b);
    canvas_rect(&win, 0, TEXT_TOP - 4, gutter_w(), ROWS * LINE_H + 8, PANEL);
    for (int row = 0; row < ROWS && top + row < nlines; row++) {
        int i = top + row;
        int y = TEXT_TOP + row * LINE_H;
        if (i == cur.line && !sel) canvas_rect(&win, gutter_w(), y, WIN_W - gutter_w(), LINE_H, PANEL);

        char num[16];
        snprintf(num, sizeof num, "%d", i + 1);
        canvas_text(&win, gutter_w() - CHAR_W - canvas_text_width(num, 1), y + 2, num, i == cur.line ? TEXT : DIM, 1);

        if (sel && i >= a.line && i <= b.line) {
            int from = i == a.line ? vcol(i, a.col) : 0;
            int to = i == b.line ? vcol(i, b.col) : vcol(i, lines[i].len) + 1;
            from -= left;
            to -= left;
            if (from < 0) from = 0;
            if (to > ncols) to = ncols;
            if (to > from) canvas_rect(&win, tx + from * CHAR_W, y, (to - from) * CHAR_W, LINE_H, SELECT);
        }

        // The visible part, tabs as spaces.
        char buf[160];
        int n = 0, v = 0;
        line_t* l = &lines[i];
        for (int c = 0; c < l->len && v < left + ncols && n < (int)sizeof buf - 1; c++) {
            unsigned char ch = l->s[c];
            int next = ch == '\t' ? (v / TAB + 1) * TAB : v + 1;
            for (; v < next; v++)
                if (v >= left && v < left + ncols && n < (int)sizeof buf - 1)
                    buf[n++] = ch == '\t' ? ' ' : ch < 32 || ch > 126 ? '?' : ch;
        }
        buf[n] = '\0';
        canvas_text(&win, tx, y + 2, buf, TEXT, 1);
        if (left > 0 && l->len) canvas_text(&win, tx - 6, y + 2, "<", DIM, 1);
    }

    int v = vcol(cur.line, cur.col) - left;
    if (cur.line >= top && cur.line < top + ROWS && v >= 0 && v <= ncols)
        canvas_rect(&win, tx + v * CHAR_W - 1, TEXT_TOP + (cur.line - top) * LINE_H, 2, LINE_H, ACCENT);

    // A scroll bar, when there's more than fits.
    if (nlines > ROWS) {
        int track = ROWS * LINE_H;
        int h = track * ROWS / nlines;
        if (h < 10) h = 10;
        int y = TEXT_TOP + (track - h) * top / (nlines - ROWS);
        canvas_rect(&win, WIN_W - 6, y, 4, h, DIM);
    }
}

static void render(void) {
    canvas_clear(&win, BG);
    canvas_rect(&win, 0, 0, WIN_W, HEADER_H, HEADER);
    const char* name = path[0] ? path : "untitled";
    int mod = modified();
    text_fit(PAD, 10, name, ACCENT, SAVE_X - 3 * PAD - (mod ? 12 * CHAR_W : 0));
    if (mod) {
        int w = canvas_text_width(name, 1);
        if (w > SAVE_X - 3 * PAD - 12 * CHAR_W) w = SAVE_X - 3 * PAD - 12 * CHAR_W;
        canvas_text(&win, PAD + w + CHAR_W, 10, "(unsaved)", WARN, 1);
    }
    canvas_rect(&win, SAVE_X, 5, SAVE_W, HEADER_H - 10, mod ? SELECT : PANEL);
    canvas_text(&win, SAVE_X + (SAVE_W - 32) / 2, 10, "Save", mod ? TEXT : DIM, 1);
    draw_text();

    int y = WIN_H - FOOTER_H;
    canvas_rect(&win, 0, y, WIN_W, FOOTER_H, HEADER);
    char where[48];
    snprintf(where, sizeof where, "Ln %d, Col %d", cur.line + 1, vcol(cur.line, cur.col) + 1);
    int where_w = canvas_text_width(where, 1);
    canvas_text(&win, WIN_W - PAD - where_w, y + 6, where, DIM, 1);
    int room = WIN_W - 3 * PAD - where_w;
    char line[256];
    if (asking == ASK_QUIT) {
        text_fit(PAD, y + 6, "unsaved changes: s save and quit, d discard them, Esc keep editing", WARN, room);
    } else if (asking) {
        const char* q = asking == ASK_FIND ? "find" : asking == ASK_LINE ? "go to line" : "save as";
        snprintf(line, sizeof line, "%s: %s_", q, answer);
        text_fit(PAD, y + 6, line, WARN, room);
    } else if (status[0]) {
        text_fit(PAD, y + 6, status, status_color, room);
    } else {
        text_fit(PAD, y + 6, "^S save  ^F find  ^G line  ^Z undo  ^Y redo  ^X ^C ^V cut copy paste  ^Q quit", DIM, room);
    }
    canvas_present(&win);
}

// ---- input -------------------------------------------------------------------

static __attribute__((noreturn)) void quit(void) {
    canvas_close(&win);
    exit();
}

// Moves the cursor; the selection goes, unless the mouse is extending it.
static void move_to(pos_t p) {
    cur = p;
    has_anchor = 0;
    no_join = 1;
}

static void move_vertically(int by) {
    if (want_vcol < 0) want_vcol = vcol(cur.line, cur.col);
    int line = cur.line + by;
    if (line < 0) line = 0;
    if (line >= nlines) line = nlines - 1;
    int keep = want_vcol;
    move_to((pos_t){ line, col_at(line, keep) });
    want_vcol = keep;
}

static void home(void) {
    // To the first non-blank, or the very start if already there.
    line_t* l = &lines[cur.line];
    int first = 0;
    while (first < l->len && (l->s[first] == ' ' || l->s[first] == '\t')) first++;
    move_to((pos_t){ cur.line, cur.col == first ? 0 : first });
}

static void on_answer_key(int key) {
    if (asking == ASK_QUIT) {
        if (key == 's' || key == 'S') {
            asking = ASK_NONE;
            if (save() == 0) quit();
        } else if (key == 'd' || key == 'D') {
            quit();
        } else if (key == 27) {
            asking = ASK_NONE;
        }
        return;
    }
    size_t n = strlen(answer);
    if (key == 27) {
        asking = ASK_NONE;
    } else if (key == '\n') {
        if (asking == ASK_FIND) {
            // Stay open, so Enter again finds the next one.
            strlcpy(found, answer, sizeof found);
            status[0] = '\0';
            find_next();
            follow_cursor();
            if (status[0] && status_color == WARN) asking = ASK_NONE;
            return;
        }
        int was = asking;
        asking = ASK_NONE;
        if (was == ASK_LINE) {
            int line = atoi(answer);
            if (line < 1) line = 1;
            if (line > nlines) line = nlines;
            move_to((pos_t){ line - 1, 0 });
            top = cur.line > ROWS / 2 ? cur.line - ROWS / 2 : 0;
            follow_cursor();
        } else if (was == ASK_SAVE_AS) {
            save_as(answer);
        }
    } else if (key == 8) {
        if (n) answer[n - 1] = '\0';
    } else if (key >= 32 && key < 127 && n < sizeof answer - 1) {
        answer[n] = (char)key;
        answer[n + 1] = '\0';
    }
}

static void on_key(int key) {
    if (asking) {
        on_answer_key(key);
        return;
    }
    status[0] = '\0';
    pos_t a, b;
    line_t* l = &lines[cur.line];
    switch (key) {
    case CANVAS_KEY_UP: move_vertically(-1); break;
    case CANVAS_KEY_DOWN: move_vertically(1); break;
    case CANVAS_KEY_PAGE_UP: move_vertically(-ROWS); break;
    case CANVAS_KEY_PAGE_DOWN: move_vertically(ROWS); break;
    case CANVAS_KEY_LEFT:
        if (selection(&a, &b)) move_to(a);
        else if (cur.col > 0) move_to((pos_t){ cur.line, cur.col - 1 });
        else if (cur.line > 0) move_to((pos_t){ cur.line - 1, lines[cur.line - 1].len });
        want_vcol = -1;
        break;
    case CANVAS_KEY_RIGHT:
        if (selection(&a, &b)) move_to(b);
        else if (cur.col < l->len) move_to((pos_t){ cur.line, cur.col + 1 });
        else if (cur.line + 1 < nlines) move_to((pos_t){ cur.line + 1, 0 });
        want_vcol = -1;
        break;
    case CANVAS_KEY_HOME: home(); want_vcol = -1; break;
    case CANVAS_KEY_END: move_to((pos_t){ cur.line, l->len }); want_vcol = -1; break;
    case 8:
        if (delete_selection()) break;
        if (cur.col > 0) delete((pos_t){ cur.line, cur.col - 1 }, cur, 1);
        else if (cur.line > 0) delete((pos_t){ cur.line - 1, lines[cur.line - 1].len }, cur, 1);
        break;
    case CANVAS_KEY_DELETE:
        if (delete_selection()) break;
        if (cur.col < l->len) delete(cur, (pos_t){ cur.line, cur.col + 1 }, 0);
        else if (cur.line + 1 < nlines) delete(cur, (pos_t){ cur.line + 1, 0 }, 0);
        no_join = 1;
        break;
    case '\n': {
        // The new line starts as indented as this one.
        delete_selection();
        l = &lines[cur.line];
        char indent[81] = "\n";
        int n = 1;
        while (n < 80 && n - 1 < cur.col && (l->s[n - 1] == ' ' || l->s[n - 1] == '\t')) {
            indent[n] = l->s[n - 1];
            n++;
        }
        insert(indent, n, 0);
        no_join = 1;
        break;
    }
    case '\t': {
        delete_selection();
        int v = vcol(cur.line, cur.col);
        insert("    ", TAB - v % TAB, 1);
        break;
    }
    case 27: has_anchor = 0; break;
    case CANVAS_CTRL('s'): save(); break;
    case CANVAS_CTRL('q'):
        if (modified()) asking = ASK_QUIT;
        else quit();
        break;
    case CANVAS_CTRL('f'):
        if (selection(&a, &b) && a.line == b.line && b.col - a.col < (int)sizeof answer) {
            int len;
            char* s = text_of(a, b, &len);
            strlcpy(answer, s, sizeof answer);
            free(s);
        } else {
            strlcpy(answer, found, sizeof answer);
        }
        asking = ASK_FIND;
        break;
    case CANVAS_CTRL('g'):
        answer[0] = '\0';
        asking = ASK_LINE;
        break;
    case CANVAS_CTRL('z'): undo(); break;
    case CANVAS_CTRL('y'): redo(); break;
    case CANVAS_CTRL('a'):
        anchor = (pos_t){ 0, 0 };
        cur = (pos_t){ nlines - 1, lines[nlines - 1].len };
        has_anchor = 1;
        break;
    case CANVAS_CTRL('c'): copy(NULL, NULL); break;
    case CANVAS_CTRL('x'): cut(); break;
    case CANVAS_CTRL('v'): paste(); break;
    default:
        if (key >= 32 && key < 127) {
            char c = (char)key;
            if (delete_selection()) no_join = 1;
            insert(&c, 1, c != ' ');
            // A space ends the word: the next one is a new undo step.
            if (c == ' ') no_join = 1;
        }
        break;
    }
    follow_cursor();
}

// The text position under (x, y), clamped to the text.
static pos_t pos_at(int x, int y) {
    int row = y < TEXT_TOP ? -1 : (y - TEXT_TOP) / LINE_H;
    int line = top + row;
    if (line < 0) line = 0;
    if (line >= nlines) line = nlines - 1;
    int v = left + (x - text_x() + CHAR_W / 2) / CHAR_W;
    if (x < text_x()) v = left;
    if (v < 0) v = 0;
    return (pos_t){ line, col_at(line, v) };
}

static int is_word(char c) {
    return isalnum((unsigned char)c) || c == '_';
}

static void on_mouse_down(int x, int y) {
    static uint64_t last_at;
    static pos_t last_pos;
    if (y < HEADER_H) {
        if (x >= SAVE_X && x < SAVE_X + SAVE_W) save();
        return;
    }
    if (y >= WIN_H - FOOTER_H) return;
    if (asking != ASK_QUIT) asking = ASK_NONE;
    status[0] = '\0';
    pos_t p = pos_at(x, y);
    uint64_t now = uptime_ms();
    if (same(p, last_pos) && now - last_at < DOUBLE_CLICK_MS) {
        // A double-click selects the word.
        line_t* l = &lines[p.line];
        int a = p.col, b = p.col;
        while (a > 0 && is_word(l->s[a - 1])) a--;
        while (b < l->len && is_word(l->s[b])) b++;
        anchor = (pos_t){ p.line, a };
        cur = (pos_t){ p.line, b };
        has_anchor = 1;
        last_at = 0;
        return;
    }
    last_at = now;
    last_pos = p;
    move_to(p);
    anchor = p;
    has_anchor = 1;
    dragging = 1;
    want_vcol = -1;
}

static void on_mouse_move(int x, int y) {
    if (!dragging) return;
    cur = pos_at(x, y);
    follow_cursor();
}

static void on_scroll(int clicks) {
    top -= clicks * SCROLL_LINES;
    if (top > nlines - ROWS) top = nlines - ROWS;
    if (top < 0) top = 0;
}

// ---- start -------------------------------------------------------------------

static __attribute__((noreturn)) void editor(void) {
    char title[64];
    snprintf(title, sizeof title, "Edit - %s", path[0] ? base_name(path) : "untitled");
    if (canvas_open(&win, title, WIN_W, WIN_H) < 0) {
        printf("edit: couldn't open a window\n");
        exit();
    }
    load();
    int dirty = 1;
    for (;;) {
        canvas_event_t e;
        while (canvas_poll(&win, &e) > 0) {
            if (e.kind == CANVAS_CLOSED) quit();
            if (e.kind == CANVAS_KEY) on_key(e.key);
            else if (e.kind == CANVAS_MOUSE_DOWN && e.buttons == CANVAS_LEFT) on_mouse_down(e.x, e.y);
            else if (e.kind == CANVAS_MOUSE_MOVE && (e.buttons & CANVAS_LEFT)) on_mouse_move(e.x, e.y);
            else if (e.kind == CANVAS_MOUSE_UP) dragging = 0;
            else if (e.kind == CANVAS_SCROLL) on_scroll(e.key);
            else continue;
            dirty = 1;
        }
        if (dirty) {
            render();
            dirty = 0;
        }
        sleep_ms(15);
    }
}

void _start(void) {
    char* argv[4];
    int argc = get_argv(argv, 4);
    if (argc > 1 && !strcmp(argv[0], "--window")) {
        // --window <folder> [file]
        strlcpy(dir, argv[1], sizeof dir);
        if (argc > 2) strlcpy(path, argv[2], sizeof path);
        editor();
    }

    // Started from a shell, which waits for us: check the file here, where
    // we can still say what's wrong, then start the real editor as our own
    // child with absolute paths, and end so the Terminal gets its prompt
    // back.
    char cwd[256], file[256] = "";
    if (fs_cwd(cwd, sizeof cwd) < 0) strcpy(cwd, "/");
    if (argc > 0) {
        join_path(file, sizeof file, cwd, argv[0]);
        int n;
        fs_entry_t* is_dir = fs_list(file, &n);
        if (is_dir) {
            free(is_dir);
            printf("edit: %s is a folder\n", file);
            exit();
        }
        long size = fs_size(file);
        if (size > 0) {
            char head[4096];
            long got = fs_read(file, head, sizeof head);
            if (got >= 0 && !is_text(head, got < (long)sizeof head ? got : (long)sizeof head)) {
                printf("edit: %s isn't a text file\n", file);
                exit();
            }
        }
    }
    char args[600];
    snprintf(args, sizeof args, "--window %s %s", cwd, file);
    if (spawn(SELF, args) < 0) printf("edit: couldn't start: %s\n", last_error());
    exit();
}
