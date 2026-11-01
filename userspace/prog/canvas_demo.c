// A sketch pad on a canvas: draw with the mouse, pick colors from the
// palette, and watch a ball bounce over your drawing.
// Keys: 1-6 pick a color, c clears, q or Escape quits.
#include "../libc/sys.h"
#include "../libc/canvas.h"

#define W 480
#define H 320
#define BAR 32  // palette strip at the top
#define SWATCH 24

static const uint32_t palette[6] = {
    0x00202020, 0x00d03030, 0x0030a040, 0x003060d0, 0x00e0a020, 0x00a040c0,
};

// The drawing, kept apart from the canvas so the ball can move over it
// without erasing it: each frame starts as a copy of the paper.
static uint32_t* paper;

static void paper_clear(void) {
    for (int i = 0; i < W * H; i++) paper[i] = 0x00ffffff;
}

// A thick stroke: a filled circle at every point of the line.
static void stroke(canvas_t* c, int x0, int y0, int x1, int y1, uint32_t color) {
    canvas_t on_paper = *c;
    on_paper.pixels = paper;
    int steps = (x1 > x0 ? x1 - x0 : x0 - x1) + (y1 > y0 ? y1 - y0 : y0 - y1);
    if (steps == 0) steps = 1;
    for (int i = 0; i <= steps; i++) {
        int x = x0 + (x1 - x0) * i / steps, y = y0 + (y1 - y0) * i / steps;
        if (y >= BAR) canvas_circle(&on_paper, x, y, 3, color);
    }
}

static void quit(canvas_t* c) {
    canvas_close(c);
    writeln("canvas_demo: bye");
    exit();
}

void _start(void) {
    canvas_t c;
    if (canvas_open(&c, "Canvas Demo", W, H) != 0) {
        writeln("canvas_demo: couldn't open a canvas");
        exit();
    }
    paper = alloc_z(W * H * sizeof(uint32_t));
    paper_clear();
    writeln("canvas_demo: draw with the mouse; 1-6 colors, c clears, q quits");

    int color = 3, drawing = 0, last_x = 0, last_y = 0;
    int ball_x = W / 2, ball_y = H / 2, ball_dx = 3, ball_dy = 2;
    long frame = 0;
    for (;;) {
        canvas_event_t e;
        while (canvas_poll(&c, &e) > 0) {
            if (e.kind == CANVAS_CLOSED) quit(&c);
            if (e.kind == CANVAS_KEY) {
                if (e.key == 'q' || e.key == 27) quit(&c);
                if (e.key == 'c') paper_clear();
                if (e.key >= '1' && e.key <= '6') color = e.key - '1';
            }
            if (e.kind == CANVAS_MOUSE_DOWN && e.buttons == CANVAS_LEFT) {
                if (e.y < BAR) {
                    int i = (e.x - 8) / (SWATCH + 6);
                    if (i >= 0 && i < 6) color = i;
                } else {
                    drawing = 1;
                    last_x = e.x;
                    last_y = e.y;
                    stroke(&c, e.x, e.y, e.x, e.y, palette[color]);
                }
            }
            if (e.kind == CANVAS_MOUSE_UP) drawing = 0;
            if (e.kind == CANVAS_MOUSE_MOVE && drawing && (e.buttons & CANVAS_LEFT)) {
                stroke(&c, last_x, last_y, e.x, e.y, palette[color]);
                last_x = e.x;
                last_y = e.y;
            }
        }

        // Move the ball, bouncing off the edges below the palette.
        ball_x += ball_dx;
        ball_y += ball_dy;
        if (ball_x < 12 || ball_x > W - 12) ball_dx = -ball_dx;
        if (ball_y < BAR + 12 || ball_y > H - 12) ball_dy = -ball_dy;

        // Compose the frame: paper, ball, then the palette strip.
        for (int i = 0; i < W * H; i++) c.pixels[i] = paper[i];
        canvas_circle(&c, ball_x + 2, ball_y + 3, 10, 0x00b0b0b0);
        canvas_circle(&c, ball_x, ball_y, 10, palette[(frame / 30) % 6]);
        canvas_circle_outline(&c, ball_x, ball_y, 10, 0x00000000);
        canvas_rect(&c, 0, 0, W, BAR, 0x00e8e8ec);
        canvas_line(&c, 0, BAR - 1, W - 1, BAR - 1, 0x00a0a0a8);
        for (int i = 0; i < 6; i++) {
            int x = 8 + i * (SWATCH + 6);
            canvas_rect(&c, x, 4, SWATCH, SWATCH, palette[i]);
            canvas_rect_outline(&c, x - 2, 2, SWATCH + 4, SWATCH + 4, i == color ? 0x00000000 : 0x00e8e8ec);
        }
        canvas_text(&c, 200, 12, "draw!  c: clear  q: quit", 0x00404040, 1);
        canvas_present(&c);
        frame++;
        sleep_ms(20);
    }
}
