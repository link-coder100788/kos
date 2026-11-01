// Canvases: the syscalls, and drawing into the pixel buffer. See canvas.h.

#include "canvas.h"

extern const uint8_t font8x8[128][8];

// Syscall 6 is the window syscall; the sub-command goes in x16.
static uint64_t ui_svc(uint64_t sub, uint64_t a0, uint64_t a1, uint64_t a2) {
    register uint64_t x0 __asm("x0") = a0;
    register uint64_t x1 __asm("x1") = a1;
    register uint64_t x2 __asm("x2") = a2;
    register uint64_t x16 __asm("x16") = sub;
    __asm volatile("svc #6" : "+r"(x0) : "r"(x1), "r"(x2), "r"(x16) : "memory");
    return x0;
}

enum { UI_CANVAS_OPEN = 4, UI_CANVAS_PRESENT = 5, UI_CANVAS_POLL = 6, UI_CANVAS_CLOSE = 7 };

int canvas_open(canvas_t* c, const char* title, int width, int height) {
    if (width <= 0 || height <= 0 || width > 1000 || height > 700) return -1;
    uint64_t id = ui_svc(UI_CANVAS_OPEN, (uint64_t)title, (uint64_t)width, (uint64_t)height);
    if (id == (uint64_t)-1) return -1;
    c->id = (long)id;
    c->width = width;
    c->height = height;
    c->pixels = alloc_z((size_t)width * (size_t)height * sizeof(uint32_t));
    return 0;
}

void canvas_present(canvas_t* c) {
    ui_svc(UI_CANVAS_PRESENT, (uint64_t)c->id, (uint64_t)c->pixels, (uint64_t)c->width * (uint64_t)c->height);
}

int canvas_poll(canvas_t* c, canvas_event_t* e) {
    return (int)(long)ui_svc(UI_CANVAS_POLL, (uint64_t)c->id, (uint64_t)e, 0);
}

void canvas_close(canvas_t* c) {
    ui_svc(UI_CANVAS_CLOSE, (uint64_t)c->id, 0, 0);
    free(c->pixels);
    c->pixels = 0;
}

// ---- drawing --------------------------------------------------------------------

void canvas_clear(canvas_t* c, uint32_t color) {
    size_t n = (size_t)c->width * (size_t)c->height;
    for (size_t i = 0; i < n; i++) c->pixels[i] = color;
}

void canvas_pixel(canvas_t* c, int x, int y, uint32_t color) {
    if (x >= 0 && y >= 0 && x < c->width && y < c->height) c->pixels[y * c->width + x] = color;
}

uint32_t canvas_get_pixel(const canvas_t* c, int x, int y) {
    if (x >= 0 && y >= 0 && x < c->width && y < c->height) return c->pixels[y * c->width + x];
    return 0;
}

void canvas_rect(canvas_t* c, int x, int y, int w, int h, uint32_t color) {
    // Clip once, then fill row by row.
    int x0 = x < 0 ? 0 : x, y0 = y < 0 ? 0 : y;
    int x1 = x + w > c->width ? c->width : x + w;
    int y1 = y + h > c->height ? c->height : y + h;
    for (int row = y0; row < y1; row++) {
        uint32_t* p = c->pixels + row * c->width;
        for (int col = x0; col < x1; col++) p[col] = color;
    }
}

void canvas_rect_outline(canvas_t* c, int x, int y, int w, int h, uint32_t color) {
    canvas_rect(c, x, y, w, 1, color);
    canvas_rect(c, x, y + h - 1, w, 1, color);
    canvas_rect(c, x, y, 1, h, color);
    canvas_rect(c, x + w - 1, y, 1, h, color);
}

// Bresenham's line algorithm: step along the longer axis one pixel at a
// time, and along the shorter one whenever the error builds up.
void canvas_line(canvas_t* c, int x0, int y0, int x1, int y1, uint32_t color) {
    int dx = x1 > x0 ? x1 - x0 : x0 - x1, sx = x0 < x1 ? 1 : -1;
    int dy = y1 > y0 ? y0 - y1 : y1 - y0, sy = y0 < y1 ? 1 : -1;
    int err = dx + dy;
    for (;;) {
        canvas_pixel(c, x0, y0, color);
        if (x0 == x1 && y0 == y1) break;
        int e2 = 2 * err;
        if (e2 >= dy) { err += dy; x0 += sx; }
        if (e2 <= dx) { err += dx; y0 += sy; }
    }
}

void canvas_circle(canvas_t* c, int cx, int cy, int r, uint32_t color) {
    if (r < 0) return;
    // For each row, find how far the circle reaches either side.
    int dx = r;
    for (int dy = 0; dy <= r; dy++) {
        while (dx > 0 && dx * dx + dy * dy > r * r) dx--;
        canvas_rect(c, cx - dx, cy + dy, 2 * dx + 1, 1, color);
        if (dy > 0) canvas_rect(c, cx - dx, cy - dy, 2 * dx + 1, 1, color);
    }
}

// The midpoint circle algorithm: walk one eighth of the circle and mirror
// each point into the other seven.
void canvas_circle_outline(canvas_t* c, int cx, int cy, int r, uint32_t color) {
    if (r < 0) return;
    int x = r, y = 0, err = 1 - r;
    while (x >= y) {
        canvas_pixel(c, cx + x, cy + y, color);
        canvas_pixel(c, cx - x, cy + y, color);
        canvas_pixel(c, cx + x, cy - y, color);
        canvas_pixel(c, cx - x, cy - y, color);
        canvas_pixel(c, cx + y, cy + x, color);
        canvas_pixel(c, cx - y, cy + x, color);
        canvas_pixel(c, cx + y, cy - x, color);
        canvas_pixel(c, cx - y, cy - x, color);
        y++;
        if (err < 0) {
            err += 2 * y + 1;
        } else {
            x--;
            err += 2 * (y - x) + 1;
        }
    }
}

int canvas_text(canvas_t* c, int x, int y, const char* text, uint32_t color, int scale) {
    if (scale < 1) scale = 1;
    int cx = x, widest = 0;
    for (const char* p = text; *p; p++) {
        if (*p == '\n') {
            if (cx - x > widest) widest = cx - x;
            cx = x;
            y += 8 * scale + scale;
            continue;
        }
        unsigned char ch = (unsigned char)*p;
        const uint8_t* glyph = font8x8[ch < 128 ? ch : '?'];
        for (int row = 0; row < 8; row++) {
            for (int col = 0; col < 8; col++) {
                if (glyph[row] & (1 << col)) {
                    canvas_rect(c, cx + col * scale, y + row * scale, scale, scale, color);
                }
            }
        }
        cx += 8 * scale;
    }
    return cx - x > widest ? cx - x : widest;
}

int canvas_text_width(const char* text, int scale) {
    if (scale < 1) scale = 1;
    int line = 0, widest = 0;
    for (const char* p = text; *p; p++) {
        if (*p == '\n') {
            line = 0;
            continue;
        }
        line += 8 * scale;
        if (line > widest) widest = line;
    }
    return widest;
}
