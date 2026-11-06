#ifndef C_CANVAS_H
#define C_CANVAS_H

// Canvases: windows you draw pixels into, for graphics and games.
//
// Draw into the canvas's pixel buffer with the functions below (or by
// writing c.pixels[y * c.width + x] yourself), then call canvas_present to
// show the frame. Nothing appears until you present, so you never see a
// half-drawn frame. Read the mouse and keyboard with canvas_poll.
//
//     canvas_t c;
//     canvas_open(&c, "Demo", 320, 200);
//     for (;;) {
//         canvas_event_t e;
//         while (canvas_poll(&c, &e) > 0)
//             if (e.kind == CANVAS_CLOSED) { canvas_close(&c); exit(); }
//         canvas_clear(&c, rgb(0, 0, 40));
//         canvas_text(&c, 10, 10, "Hello!", rgb(255, 255, 255), 2);
//         canvas_present(&c);
//         sleep_ms(20);
//     }
//
// See userspace/prog/canvas_demo.c for a complete program.

#include <stdint.h>
#include "sys.h"

typedef struct {
    long id;
    int width, height;
    // width * height pixels, row by row, each 0x00RRGGBB.
    uint32_t* pixels;
} canvas_t;

// A color from red, green and blue (0-255).
static inline uint32_t rgb(uint8_t r, uint8_t g, uint8_t b) {
    return ((uint32_t)r << 16) | ((uint32_t)g << 8) | b;
}

// ---- events ------------------------------------------------------------------

enum {
    CANVAS_NONE = 0,
    CANVAS_MOUSE_DOWN = 1,  // x, y; buttons is the button pressed
    CANVAS_MOUSE_UP = 2,    // x, y; buttons is the button released
    CANVAS_MOUSE_MOVE = 3,  // x, y; buttons has every button held down
    CANVAS_KEY = 4,         // key
    CANVAS_CLOSED = 5,      // the user closed the window
    CANVAS_SCROLL = 6,      // the mouse wheel turned: key is the clicks, positive away from you
};

// Mouse buttons, as bits in `buttons`.
#define CANVAS_LEFT   1
#define CANVAS_RIGHT  2
#define CANVAS_MIDDLE 4

// Keys: characters are their ASCII code, Enter is '\n', Backspace 8, Tab
// '\t' and Escape 27. Ctrl+letter is 1-26, like a terminal: CANVAS_CTRL('s')
// is Ctrl+S. Keys without one:
#define CANVAS_KEY_UP        0x100
#define CANVAS_KEY_DOWN      0x101
#define CANVAS_KEY_LEFT      0x102
#define CANVAS_KEY_RIGHT     0x103
#define CANVAS_KEY_HOME      0x104
#define CANVAS_KEY_END       0x105
#define CANVAS_KEY_PAGE_UP   0x106
#define CANVAS_KEY_PAGE_DOWN 0x107
#define CANVAS_KEY_DELETE    0x108

#define CANVAS_CTRL(letter) ((letter) - 'a' + 1)

typedef struct {
    int kind;
    // Mouse position, relative to the canvas's top-left corner. During a
    // drag it can be outside the canvas.
    int x, y;
    int buttons;
    int key;
} canvas_event_t;

// ---- the canvas -----------------------------------------------------------------

// Opens a width x height canvas window (at most 1000 x 700) with a black
// buffer. Returns 0, or -1 if the size is out of range.
int canvas_open(canvas_t* c, const char* title, int width, int height);

// Shows the buffer in the window.
void canvas_present(canvas_t* c);

// Takes the next input event into *e. Returns 1 if there was one, 0 if
// not, or -1 if the canvas isn't open.
int canvas_poll(canvas_t* c, canvas_event_t* e);

// Closes the window and frees its pixel buffer.
void canvas_close(canvas_t* c);

// ---- drawing ------------------------------------------------------------------
// Everything is clipped to the canvas, so drawing partly (or entirely)
// off the edge is fine.

void canvas_clear(canvas_t* c, uint32_t color);

void canvas_pixel(canvas_t* c, int x, int y, uint32_t color);

// The color at (x, y), or 0 outside the canvas.
uint32_t canvas_get_pixel(const canvas_t* c, int x, int y);

void canvas_rect(canvas_t* c, int x, int y, int w, int h, uint32_t color);

// A 1-pixel outline just inside the rectangle.
void canvas_rect_outline(canvas_t* c, int x, int y, int w, int h, uint32_t color);

void canvas_line(canvas_t* c, int x0, int y0, int x1, int y1, uint32_t color);

void canvas_circle(canvas_t* c, int cx, int cy, int r, uint32_t color);

void canvas_circle_outline(canvas_t* c, int cx, int cy, int r, uint32_t color);

// Draws text in the 8x8 font, each font pixel `scale` x `scale` screen
// pixels (scale 1 is 8 pixels tall). '\n' starts a new line. Returns the
// width of the widest line, in pixels.
int canvas_text(canvas_t* c, int x, int y, const char* text, uint32_t color, int scale);

// How wide canvas_text would draw `text` (its widest line).
int canvas_text_width(const char* text, int scale);

#endif //C_CANVAS_H
