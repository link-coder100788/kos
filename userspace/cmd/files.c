// files: a file explorer, in a window of its own.
//
//     files [folder]
//
// Typed in the Terminal, it opens the Files window and gives the Terminal
// back. Click a file to see what's in it; double-click (or Enter) opens a
// folder, runs a program or edits a text file. Backspace goes up a folder, Delete deletes
// (after a y), m renames, c copies, n makes a new folder, r reloads, and a
// letter jumps to the next name starting with it.

#include "../libc/sys.h"
#include "../libc/stdio.h"
#include "../libc/stdlib.h"
#include "../libc/string.h"
#include "../libc/ctype.h"
#include "../libc/canvas.h"
#include <stdarg.h>

#define SELF "/bin/files.elf.b64"
#define EDITOR "/bin/edit.elf.b64"

// ---- look --------------------------------------------------------------------

#define WIN_W 760
#define WIN_H 460
#define HEADER_H 28
#define FOOTER_H 20
#define LIST_W 360
#define ROW_H 18
#define PAD 8
#define LIST_TOP (HEADER_H + 4)
#define ROWS ((WIN_H - LIST_TOP - FOOTER_H - 4) / ROW_H)
#define UP_X (WIN_W - 50)
#define UP_W 42

#define BG      0x14122a
#define PANEL   0x1c1a38
#define HEADER  0x0d0b1e
#define TEXT    0xd6dcf0
#define DIM     0x6b7193
#define DIR     0x7aa2ff
#define EXEC    0x5fe3a1
#define ACCENT  0x2ee6d6
#define SELECT  0x5b3fd9
#define ERR     0xff5d7a
#define WARN    0xffc857

#define DOUBLE_CLICK_MS 400
#define PREVIEW_MAX 4096

// ---- state -------------------------------------------------------------------

static canvas_t win;
static char cwd[256] = "/";
static fs_entry_t* entries;
static int count;
static int selected, top;

// What the right-hand pane shows: the selected file's first bytes.
static char preview[PREVIEW_MAX + 1];
static long preview_len = -1;   // -1: nothing to show
static long preview_size;
static int preview_for = -1;    // the entry it was read for

// A question in the footer that the next keys answer.
enum { ASK_NONE, ASK_DELETE, ASK_MKDIR, ASK_RENAME };
static int asking;
static char answer[128];
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

// ---- paths -------------------------------------------------------------------

static void join(char* out, size_t size, const char* dir, const char* name) {
    if (!strcmp(dir, "/")) snprintf(out, size, "/%s", name);
    else snprintf(out, size, "%s/%s", dir, name);
}

static int ends_with(const char* s, const char* end) {
    size_t a = strlen(s), b = strlen(end);
    return a >= b && !strcasecmp(s + a - b, end);
}

static int is_program(const char* name) {
    return ends_with(name, ".elf") || ends_with(name, ".elf.b64") || ends_with(name, ".kbin");
}

// Folders first, then by name.
static int by_kind_and_name(const void* a, const void* b) {
    const fs_entry_t* x = a;
    const fs_entry_t* y = b;
    if (x->is_dir != y->is_dir) return y->is_dir - x->is_dir;
    return strcasecmp(x->name, y->name);
}

// Lists cwd, keeping the selection on `keep` if it's still there.
static void load(const char* keep) {
    free(entries);
    entries = fs_list(cwd, &count);
    if (!entries) {
        count = 0;
        set_status(ERR, "can't list %s: %s", cwd, last_error());
    }
    qsort(entries, count, sizeof *entries, by_kind_and_name);
    selected = 0;
    if (keep)
        for (int i = 0; i < count; i++)
            if (!strcmp(entries[i].name, keep)) selected = i;
    top = 0;
    preview_for = -1;
}

static void go_to(const char* path) {
    strlcpy(cwd, path, sizeof cwd);
    status[0] = '\0';
    load(NULL);
}

static void go_up(void) {
    if (!strcmp(cwd, "/")) return;
    char* slash = strrchr(cwd, '/');
    char was[256];
    strlcpy(was, slash + 1, sizeof was);
    if (slash == cwd) slash[1] = '\0';
    else *slash = '\0';
    status[0] = '\0';
    load(was);
}

// ---- actions -----------------------------------------------------------------

static void load_preview(void);
static int is_text(const char* s, long len);

static void open_selected(void) {
    if (selected >= count) return;
    fs_entry_t* e = &entries[selected];
    char path[512];
    join(path, sizeof path, cwd, e->name);
    if (e->is_dir) {
        go_to(path);
    } else if (is_program(e->name)) {
        long id = spawn(path, NULL);
        if (id < 0) set_status(ERR, "couldn't run %s: %s", e->name, last_error());
        else set_status(EXEC, "started %s (id %ld)", e->name, id);
    } else {
        load_preview();
        if (preview_len < 0 || !is_text(preview, preview_len)) {
            set_status(DIM, "%s isn't a program or text", e->name);
            return;
        }
        char args[600];
        snprintf(args, sizeof args, "--window %s %s", cwd, path);
        if (spawn(EDITOR, args) < 0) set_status(ERR, "couldn't edit %s: %s", e->name, last_error());
        else set_status(EXEC, "editing %s", e->name);
    }
}

static void delete_selected(void) {
    fs_entry_t* e = &entries[selected];
    char path[512];
    join(path, sizeof path, cwd, e->name);
    char name[256];
    strlcpy(name, e->name, sizeof name);
    if (fs_remove(path) < 0) {
        set_status(ERR, "couldn't delete %s: %s", name, last_error());
        return;
    }
    int at = selected;
    load(NULL);
    selected = at < count ? at : count - 1;
    if (selected < 0) selected = 0;
    set_status(DIM, "deleted %s", name);
}

static void make_folder(void) {
    if (!answer[0]) return;
    if (strchr(answer, '/')) {
        set_status(ERR, "a name can't have a /");
        return;
    }
    char path[512];
    join(path, sizeof path, cwd, answer);
    if (fs_mkdir(path) < 0) set_status(ERR, "couldn't make %s: %s", answer, last_error());
    else {
        load(answer);
        set_status(DIM, "made %s", answer);
    }
}

// Reads the selected file's start for the preview, once per selection.
static void load_preview(void) {
    if (preview_for == selected) return;
    preview_for = selected;
    preview_len = -1;
    if (selected >= count || entries[selected].is_dir) return;
    char path[512];
    join(path, sizeof path, cwd, entries[selected].name);
    long size = fs_read(path, preview, PREVIEW_MAX);
    if (size < 0) return;
    preview_size = size;
    preview_len = size < PREVIEW_MAX ? size : PREVIEW_MAX;
    preview[preview_len] = '\0';
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

static void select_row(int i);

// Copies the selected file to `to` in cwd. There's no rename call, so a
// rename is this plus deleting the old one. Returns 0, or -1 (with the
// status set).
static int copy_to(const char* to) {
    fs_entry_t* e = &entries[selected];
    if (e->is_dir) {
        set_status(ERR, "can't copy or rename a folder yet");
        return -1;
    }
    if (strchr(to, '/')) {
        set_status(ERR, "a name can't have a /");
        return -1;
    }
    char from[512], path[512];
    join(from, sizeof from, cwd, e->name);
    join(path, sizeof path, cwd, to);
    if (fs_size(path) >= 0) {
        set_status(ERR, "%s is already there", to);
        return -1;
    }
    size_t len;
    char* data = fs_read_all(from, &len);
    if (!data) {
        set_status(ERR, "couldn't read %s: %s", e->name, last_error());
        return -1;
    }
    long r = fs_write(path, data, len);
    free(data);
    if (r < 0) {
        set_status(ERR, "couldn't write %s: %s", to, last_error());
        return -1;
    }
    return 0;
}

static void rename_selected(const char* to) {
    if (!to[0] || !strcmp(to, entries[selected].name)) return;
    char old[256], from[512];
    strlcpy(old, entries[selected].name, sizeof old);
    if (copy_to(to) < 0) return;
    join(from, sizeof from, cwd, old);
    if (fs_remove(from) < 0) set_status(WARN, "copied to %s, but couldn't delete %s: %s", to, old, last_error());
    else set_status(DIM, "renamed %s to %s", old, to);
    load(to);
    select_row(selected);
}

static void copy_selected(void) {
    char to[256];
    snprintf(to, sizeof to, "copy_%s", entries[selected].name);
    if (copy_to(to) < 0) return;
    load(to);
    select_row(selected);
    set_status(DIM, "copied to %s", to);
}

// ---- drawing -----------------------------------------------------------------

static void human_size(char* out, size_t size, long bytes) {
    if (bytes < 1024) snprintf(out, size, "%ld B", bytes);
    else if (bytes < 1024 * 1024) snprintf(out, size, "%ld.%ld K", bytes / 1024, bytes % 1024 * 10 / 1024);
    else snprintf(out, size, "%ld.%ld M", bytes >> 20, (bytes & 0xfffff) * 10 >> 20);
}

// Draws text cut to `max` pixels, with ".." if it didn't fit.
static void text_fit(int x, int y, const char* s, uint32_t color, int max) {
    char buf[256];
    int cols = max / 8;
    if (cols <= 0) return;
    if ((int)strlen(s) <= cols) {
        canvas_text(&win, x, y, s, color, 1);
        return;
    }
    if (cols > (int)sizeof buf - 1) cols = sizeof buf - 1;
    memcpy(buf, s, cols);
    buf[cols] = '\0';
    if (cols >= 2) buf[cols - 1] = buf[cols - 2] = '.';
    canvas_text(&win, x, y, buf, color, 1);
}

static void draw_list(void) {
    canvas_rect(&win, 0, LIST_TOP - 4, LIST_W, WIN_H - LIST_TOP - FOOTER_H + 4, PANEL);
    if (count == 0) {
        canvas_text(&win, PAD, LIST_TOP + 4, "(empty)", DIM, 1);
        return;
    }
    for (int row = 0; row < ROWS && top + row < count; row++) {
        int i = top + row;
        fs_entry_t* e = &entries[i];
        int y = LIST_TOP + row * ROW_H;
        if (i == selected) canvas_rect(&win, 2, y, LIST_W - 4, ROW_H, SELECT);
        uint32_t color = e->is_dir ? DIR : is_program(e->name) ? EXEC : TEXT;
        canvas_text(&win, PAD, y + 5, e->is_dir ? ">" : " ", color, 1);
        text_fit(PAD + 14, y + 5, e->name, color, LIST_W - 100);
        if (!e->is_dir) {
            char size[24];
            human_size(size, sizeof size, e->size);
            int w = canvas_text_width(size, 1);
            canvas_text(&win, LIST_W - PAD - 8 - w, y + 5, size, i == selected ? TEXT : DIM, 1);
        }
    }
    // A scroll bar, when there's more than fits.
    if (count > ROWS) {
        int track = ROWS * ROW_H;
        int h = track * ROWS / count;
        if (h < 10) h = 10;
        int y = LIST_TOP + (track - h) * top / (count - ROWS);
        canvas_rect(&win, LIST_W - 6, y, 4, h, DIM);
    }
}

static void draw_preview(void) {
    int x = LIST_W + PAD, y = LIST_TOP + 4;
    int w = WIN_W - x - PAD;
    if (count == 0) return;
    fs_entry_t* e = &entries[selected];
    text_fit(x, y, e->name, e->is_dir ? DIR : ACCENT, w);
    y += 14;
    char info[64];
    if (e->is_dir) {
        canvas_text(&win, x, y, "folder - Enter opens it", DIM, 1);
        return;
    }
    load_preview();
    human_size(info, sizeof info, e->size);
    if (is_program(e->name)) strncat(info, " - program, Enter runs it", sizeof info - strlen(info) - 1);
    else if (preview_len >= 0 && is_text(preview, preview_len)) strncat(info, " - Enter edits it", sizeof info - strlen(info) - 1);
    canvas_text(&win, x, y, info, DIM, 1);
    y += 18;
    canvas_line(&win, x, y - 6, x + w, y - 6, PANEL);
    if (preview_len < 0) {
        canvas_text(&win, x, y, "can't read it", ERR, 1);
        return;
    }
    int bottom = WIN_H - FOOTER_H - 10;
    int cols = w / 8;
    if (!is_text(preview, preview_len)) {
        // Hex, 8 bytes a line.
        for (long i = 0; i < preview_len && y < bottom; i += 8, y += 10) {
            char line[64];
            int n = snprintf(line, sizeof line, "%04lx ", i);
            for (long j = i; j < i + 8 && j < preview_len; j++)
                n += snprintf(line + n, sizeof line - n, " %02x", (unsigned char)preview[j]);
            canvas_text(&win, x, y, line, DIM, 1);
        }
        return;
    }
    // Text, wrapped to the pane.
    char line[128];
    int n = 0;
    for (long i = 0; i <= preview_len && y < bottom; i++) {
        char c = i < preview_len ? preview[i] : '\n';
        if (c == '\r') continue;
        if (c == '\t') c = ' ';
        if (c != '\n') line[n++] = (unsigned char)c < 32 || (unsigned char)c > 126 ? '?' : c;
        if (c == '\n' || n == cols || n == (int)sizeof line - 1) {
            line[n] = '\0';
            canvas_text(&win, x, y, line, TEXT, 1);
            y += 10;
            n = 0;
        }
    }
    if (preview_size > preview_len && y < bottom + 10)
        canvas_text(&win, x, bottom, "...", DIM, 1);
}

static void render(void) {
    canvas_clear(&win, BG);
    canvas_rect(&win, 0, 0, WIN_W, HEADER_H, HEADER);
    text_fit(PAD, 10, cwd, ACCENT, UP_X - 2 * PAD);
    canvas_rect(&win, UP_X, 5, UP_W, HEADER_H - 10, !strcmp(cwd, "/") ? PANEL : SELECT);
    canvas_text(&win, UP_X + (UP_W - 16) / 2, 10, "Up", TEXT, 1);
    draw_list();
    draw_preview();

    int y = WIN_H - FOOTER_H;
    canvas_rect(&win, 0, y, WIN_W, FOOTER_H, HEADER);
    char line[256];
    if (asking == ASK_DELETE) {
        snprintf(line, sizeof line, "delete %s? y / n", entries[selected].name);
        canvas_text(&win, PAD, y + 6, line, WARN, 1);
    } else if (asking == ASK_MKDIR || asking == ASK_RENAME) {
        snprintf(line, sizeof line, "%s: %s_", asking == ASK_MKDIR ? "new folder" : "rename to", answer);
        canvas_text(&win, PAD, y + 6, line, WARN, 1);
    } else if (status[0]) {
        text_fit(PAD, y + 6, status, status_color, WIN_W - 2 * PAD);
    } else {
        snprintf(line, sizeof line, "%d item%s   Enter open   Bksp up   Del delete   m rename   c copy   n new folder   r reload",
                 count, count == 1 ? "" : "s");
        canvas_text(&win, PAD, y + 6, line, DIM, 1);
    }
    canvas_present(&win);
}

// ---- input -------------------------------------------------------------------

static void select_row(int i) {
    if (count == 0) return;
    if (i < 0) i = 0;
    if (i >= count) i = count - 1;
    selected = i;
    if (selected < top) top = selected;
    if (selected >= top + ROWS) top = selected - ROWS + 1;
}

// The next entry after the selection whose name starts with c.
static void jump_to_letter(int c) {
    for (int k = 1; k <= count; k++) {
        int i = (selected + k) % count;
        if (tolower((unsigned char)entries[i].name[0]) == tolower(c)) {
            select_row(i);
            return;
        }
    }
}

static void on_answer_key(int key) {
    if (asking == ASK_DELETE) {
        asking = ASK_NONE;
        if (key == 'y' || key == 'Y') delete_selected();
        return;
    }
    size_t n = strlen(answer);
    if (key == 27) asking = ASK_NONE;
    else if (key == '\n') {
        int was = asking;
        asking = ASK_NONE;
        if (was == ASK_RENAME) rename_selected(answer);
        else make_folder();
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
    switch (key) {
    case CANVAS_KEY_UP: select_row(selected - 1); break;
    case CANVAS_KEY_DOWN: select_row(selected + 1); break;
    case CANVAS_KEY_PAGE_UP: select_row(selected - ROWS); break;
    case CANVAS_KEY_PAGE_DOWN: select_row(selected + ROWS); break;
    case CANVAS_KEY_HOME: select_row(0); break;
    case CANVAS_KEY_END: select_row(count - 1); break;
    case '\n': case CANVAS_KEY_RIGHT: open_selected(); break;
    case 8: case CANVAS_KEY_LEFT: go_up(); break;
    case CANVAS_KEY_DELETE:
        if (count) asking = ASK_DELETE;
        break;
    case 'n':
        answer[0] = '\0';
        asking = ASK_MKDIR;
        break;
    case 'm':
        if (count) {
            strlcpy(answer, entries[selected].name, sizeof answer);
            asking = ASK_RENAME;
        }
        break;
    case 'c':
        if (count) copy_selected();
        break;
    case 'r':
        if (count) {
            char keep[256];
            strlcpy(keep, entries[selected].name, sizeof keep);
            load(keep);
        } else load(NULL);
        select_row(selected);
        break;
    default:
        if (isalnum(key)) jump_to_letter(key);
    }
}

static void on_click(int x, int y) {
    static uint64_t last_at;
    static int last_row = -1;
    if (y < HEADER_H) {
        if (x >= UP_X && x < UP_X + UP_W) go_up();
        return;
    }
    asking = ASK_NONE;
    if (x >= LIST_W || y < LIST_TOP) return;
    int row = (y - LIST_TOP) / ROW_H;
    if (row >= ROWS || top + row >= count) return;
    int i = top + row;
    uint64_t now = uptime_ms();
    status[0] = '\0';
    select_row(i);
    if (i == last_row && now - last_at < DOUBLE_CLICK_MS) {
        last_row = -1;
        open_selected();
        return;
    }
    last_row = i;
    last_at = now;
}

// ---- start -------------------------------------------------------------------

static __attribute__((noreturn)) void explorer(const char* start) {
    if (canvas_open(&win, "Files", WIN_W, WIN_H) < 0) {
        printf("files: couldn't open a window\n");
        exit(0);
    }
    if (start) strlcpy(cwd, start, sizeof cwd);
    load(NULL);
    int dirty = 1;
    for (;;) {
        canvas_event_t e;
        while (canvas_poll(&win, &e) > 0) {
            if (e.kind == CANVAS_CLOSED) {
                canvas_close(&win);
                exit(0);
            }
            if (e.kind == CANVAS_KEY) on_key(e.key);
            else if (e.kind == CANVAS_MOUSE_DOWN && e.buttons == CANVAS_LEFT) on_click(e.x, e.y);
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
    if (argc > 0 && !strcmp(argv[0], "--window")) explorer(argc > 1 ? argv[1] : NULL);

    // Started from a shell, which waits for us: start the real explorer as
    // our own child, at an absolute path, and end so the Terminal gets its
    // prompt back.
    char start[256];
    if (fs_cwd(start, sizeof start) < 0) strcpy(start, "/");
    if (argc > 0) {
        char rel[256];
        strlcpy(rel, start, sizeof rel);
        if (argv[0][0] == '/') strlcpy(start, argv[0], sizeof start);
        else join(start, sizeof start, rel, argv[0]);
    }
    int n;
    fs_entry_t* check = fs_list(start, &n);
    if (!check) {
        printf("files: no folder %s\n", start);
        exit(0);
    }
    free(check);
    char args[300];
    snprintf(args, sizeof args, "--window %s", start);
    if (spawn(SELF, args) < 0) printf("files: couldn't start: %s\n", last_error());
    exit(0);
}
