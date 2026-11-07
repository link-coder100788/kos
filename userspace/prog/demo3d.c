// A tour of render3d.h: a little world with every kind of shape in it, a
// windmill built from parts with push and pop, and a camera to fly round
// it.
//
// Drag with the mouse or use the arrow keys to circle the scene; the wheel
// or + and - zoom. f turns the fog on and off, l the scanlines, 1-4 change
// the pixel size, space pauses, and q or Escape quits.
#include "../libc/sys.h"
#include "../libc/canvas.h"
#include "../libc/render3d.h"
#include "../libc/math.h"
#include "../libc/stdio.h"

#define W 640
#define H 480
#define LAND 16            // the ground is LAND x LAND cells
#define CELL 2.0f

static canvas_t c;
static r3d_t r;
static float spin, orbit = 0.6f, tilt = 0.45f, distance = 22;
static int paused, fog = 1, scale = 2;

// Rolling hills, flattened in the middle where the things stand.
static float hills(float x, float z) {
    float d = (float)sqrt(x * x + z * z);
    float flat = d < 9 ? 0 : (d - 9) / 7;
    return flat * (float)(1.5 * sin(x * 0.4) * cos(z * 0.3) + 1.0);
}

static uint32_t grass(float x, float z) {
    int check = ((int)floor(x / CELL) + (int)floor(z / CELL)) & 1;
    return check ? rgb(110, 170, 90) : rgb(95, 155, 80);
}

// A gem: two pyramids base to base, every face its own color.
static const r3d_vec gem_points[] = {
    { 0, 1.2f, 0 }, { 0.8f, 0, 0 }, { 0, 0, 0.8f }, { -0.8f, 0, 0 }, { 0, 0, -0.8f }, { 0, -1.2f, 0 },
};
static const r3d_face gem_faces[] = {
    { 0, 1, 2, 0xe04060 }, { 0, 2, 3, 0xc03050 }, { 0, 3, 4, 0xe04060 }, { 0, 4, 1, 0xc03050 },
    { 5, 2, 1, 0xa02040 }, { 5, 3, 2, 0x902038 }, { 5, 4, 3, 0xa02040 }, { 5, 1, 4, 0x902038 },
};
static const r3d_mesh gem = { gem_points, 6, gem_faces, 8 };

static void tree(float x, float z) {
    r3d_push(&r);
    r3d_translate(&r, x, hills(x, z), z);
    r3d_cylinder(&r, 0.25f, 0.2f, 1, 6, rgb(110, 75, 45));
    r3d_translate(&r, 0, 0.8f, 0);
    r3d_cone(&r, 1.3f, 2.2f, 7, rgb(40, 120, 60));
    r3d_translate(&r, 0, 1.2f, 0);
    r3d_cone(&r, 0.9f, 1.8f, 7, rgb(50, 140, 70));
    r3d_pop(&r);
}

// The windmill: its sails turn about the hub, wherever the mill stands.
static void windmill(float x, float z) {
    r3d_push(&r);
    r3d_translate(&r, x, hills(x, z), z);
    r3d_rotate_y(&r, 0.5f);
    r3d_cylinder(&r, 1.4f, 0.9f, 5, 8, rgb(235, 225, 205));
    r3d_translate(&r, 0, 5, 0);
    r3d_cone(&r, 1.2f, 1.4f, 8, rgb(170, 60, 50));
    r3d_translate(&r, 0, -0.4f, -1.1f);
    r3d_box(&r, 0.4f, 0.4f, 0.6f, rgb(90, 70, 50));
    r3d_rotate_z(&r, spin * 0.8f);
    for (int i = 0; i < 4; i++) {
        r3d_rotate_z(&r, (float)M_PI / 2);
        r3d_push(&r);
        r3d_translate(&r, 0, 1.9f, -0.3f);
        r3d_box(&r, 0.7f, 3.4f, 0.08f, rgb(240, 240, 230));
        r3d_pop(&r);
    }
    r3d_pop(&r);
}

static void scene(void) {
    r3d_heightfield(&r, -LAND * CELL / 2, -LAND * CELL / 2, LAND, LAND, CELL, hills, grass);

    // Axes, for getting your bearings: x red, y green, z blue.
    r3d_line(&r, r3d_v(0, 0.02f, 0), r3d_v(3, 0.02f, 0), rgb(255, 60, 60));
    r3d_line(&r, r3d_v(0, 0, 0), r3d_v(0, 3, 0), rgb(60, 255, 60));
    r3d_line(&r, r3d_v(0, 0.02f, 0), r3d_v(0, 0.02f, 3), rgb(60, 120, 255));

    r3d_push(&r);
    r3d_translate(&r, -4, 1, 0);
    r3d_rotate_y(&r, spin);
    r3d_rotate_x(&r, spin * 0.6f);
    r3d_box(&r, 1.6f, 1.6f, 1.6f, rgb(230, 170, 40));
    r3d_pop(&r);

    r3d_push(&r);
    r3d_translate(&r, 4, 1.2f, 0);
    r3d_sphere(&r, 1.2f, 8, rgb(70, 130, 220));
    r3d_pop(&r);

    r3d_push(&r);
    r3d_translate(&r, 0, 1.6f + 0.3f * (float)sin(spin * 2), 4);
    r3d_rotate_y(&r, -spin * 1.5f);
    r3d_draw_mesh(&r, &gem);
    r3d_pop(&r);

    windmill(0, -6);
    tree(-9, -8);
    tree(-11, 3);
    tree(9, 7);
    tree(11, -4);
    tree(-7, 10);
}

static void label(float x, float y, float z, const char* text) {
    int sx, sy;
    if (r3d_project(&r, r3d_v(x, y, z), &sx, &sy) > 0) {
        canvas_text(&c, sx - canvas_text_width(text, 1) / 2, sy, text, rgb(255, 255, 255), 1);
    }
}

static void setup(int new_scale) {
    r3d_free(&r);
    if (r3d_init(&r, &c, new_scale) != 0) {
        writeln("demo3d: out of memory");
        exit(0);
    }
    scale = new_scale;
    r.sky_top = rgb(70, 120, 210);
    r.sky_horizon = rgb(200, 220, 240);
    r.ground = rgb(95, 155, 80);
    r.fog_color = r.sky_horizon;
}

void _start(void) {
    if (canvas_open(&c, "3D demo", W, H) != 0) {
        writeln("demo3d: couldn't open a window");
        exit(0);
    }
    setup(scale);

    int dragging = 0, last_x = 0, last_y = 0;
    long frames = 0, fps = 0;
    uint64_t fps_at = uptime_ms(), last = fps_at;
    for (;;) {
        canvas_event_t e;
        while (canvas_poll(&c, &e) > 0) {
            if (e.kind == CANVAS_CLOSED) goto quit;
            if (e.kind == CANVAS_KEY) {
                if (e.key == 27 || e.key == 'q') goto quit;
                if (e.key == CANVAS_KEY_LEFT) orbit -= 0.1f;
                if (e.key == CANVAS_KEY_RIGHT) orbit += 0.1f;
                if (e.key == CANVAS_KEY_UP) tilt += 0.05f;
                if (e.key == CANVAS_KEY_DOWN) tilt -= 0.05f;
                if (e.key == '+' || e.key == '=') distance -= 2;
                if (e.key == '-') distance += 2;
                if (e.key == 'f') fog = !fog;
                if (e.key == 'l') r.scanlines = !r.scanlines;
                if (e.key == ' ') paused = !paused;
                if (e.key >= '1' && e.key <= '4') setup(e.key - '0');
            }
            if (e.kind == CANVAS_SCROLL) distance -= e.key * 1.5f;
            if (e.kind == CANVAS_MOUSE_DOWN) { dragging = 1; last_x = e.x; last_y = e.y; }
            if (e.kind == CANVAS_MOUSE_UP) dragging = 0;
            if (e.kind == CANVAS_MOUSE_MOVE && dragging) {
                orbit += (e.x - last_x) * 0.01f;
                tilt += (e.y - last_y) * 0.005f;
                last_x = e.x;
                last_y = e.y;
            }
        }
        tilt = tilt < 0.05f ? 0.05f : tilt > 1.4f ? 1.4f : tilt;
        distance = distance < 6 ? 6 : distance > 60 ? 60 : distance;

        uint64_t now = uptime_ms();
        if (!paused) spin += (now - last) / 1000.0f;
        last = now;

        r.fog_near = 15;
        r.fog_far = fog ? 45 : 0;
        r3d_vec eye = r3d_v(distance * (float)(sin(orbit) * cos(tilt)), distance * (float)sin(tilt),
                            -distance * (float)(cos(orbit) * cos(tilt)));
        r3d_look_at(&r, eye, r3d_v(0, 1, 0));
        r3d_begin(&r);
        scene();
        r3d_end(&r);

        label(-4, 2.6f, 0, "box");
        label(4, 2.9f, 0, "sphere");
        label(0, 3.4f, 4, "mesh");
        label(0, 8, -6, "push/pop");
        char buf[80];
        snprintf(buf, sizeof buf, "%ld fps  %d polygons  1/%d size", fps, r.polygons, scale);
        canvas_text(&c, 8, 8, buf, rgb(255, 255, 255), 1);
        canvas_text(&c, 8, H - 16, "drag/arrows turn  wheel/+- zoom  f fog  l lines  1-4 size", rgb(230, 230, 240), 1);
        canvas_present(&c);

        frames++;
        if (now - fps_at >= 1000) {
            fps = frames * 1000 / (long)(now - fps_at);
            frames = 0;
            fps_at = now;
        }
        sleep_ms(10);
    }
quit:
    r3d_free(&r);
    canvas_close(&c);
    writeln("demo3d: bye");
    exit(0);
}
