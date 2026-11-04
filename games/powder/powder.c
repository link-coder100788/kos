// Powder Run: a 3D downhill ski game for KOS, in the style of an early-80s
// arcade cabinet.
//
// Left and right (or A and D, or the mouse) turn your skis; Down points them
// straight downhill and Up turns them across the slope to stop. Space or a
// click jumps, which clears rocks but not trees. Ski between the flags of
// each gate to win time and build a combo; when the clock runs out the run
// is over.
//
// The world is drawn with libc's render3d.h at half the window's size,
// doubled up with dimmed scanlines like a cabinet's monitor.
//
// Escape quits. The best score is kept in /powder.hi.
#include "sys.h"
#include "canvas.h"
#include "math.h"
#include "render3d.h"
#include "print/print.h"

#define W 640              // the window; the 3D view is half this
#define H 480

#define CELL 4.0f           // terrain grid spacing, in metres
#define ROWS 20            // terrain rows drawn ahead of the camera
#define COLS 11            // terrain cells drawn either side of it
#define FOG_NEAR 22.0
#define FOG_FAR 78.0

#define TRAIL 18.0         // half the width of the groomed run
#define MAX_OBJ 400
#define MAX_FLAKES 70
#define AHEAD 110.0        // how far ahead the course is built

#define START_TIME 30.0
#define HISCORE_FILE "/powder.hi"

enum { TREE, ROCK, GATE };
enum { PENDING, PASSED, MISSED };

typedef struct {
    int kind, alive;
    double x, y, z;        // on the ground; a gate's x is its middle
    double size;           // a gate's is its half-width
    double angle;          // its turn about the vertical
    int state, red;        // gates only
} obj_t;

typedef struct { double x, y, speed; } flake_t;

enum { TITLE, PLAYING, OVER };

static canvas_t c;
static r3d_t r;
static obj_t objs[MAX_OBJ];
static flake_t flakes[MAX_FLAKES];
static double ridge[1024];  // the mountains on the horizon

static int state = TITLE;
static uint64_t seed = 0x2545f4914f6cdd1dull;

// The skier.
static double sx, sz, heading, speed, air, air_v, lean, crash_t, crash_side;
static int steer;          // -4..4: how far the skis are turned, in 22.5 degree steps

// The camera: its pitch is how far it looks down.
static double cam_yaw, cam_pitch;

// The course as it's built.
static double built_z, prev_gate_z, prev_gate_x, next_gate_z, next_gate_x;
static int gate_count;

static double time_left, ready_t, over_t, msg_t;
static long score, best, gates_passed, combo;
static const char* msg = "";
static uint32_t msg_color;
static int new_best;

#define HAZE_R 205
#define HAZE_G 216
#define HAZE_B 238

// ---- small helpers ------------------------------------------------------------

static uint32_t rand32(void) {
    seed ^= seed << 13;
    seed ^= seed >> 7;
    seed ^= seed << 17;
    return (uint32_t)(seed >> 16);
}

static double rand_range(double lo, double hi) {
    return lo + (hi - lo) * (rand32() % 10000) / 10000.0;
}

static double clamp(double v, double lo, double hi) {
    return v < lo ? lo : v > hi ? hi : v;
}

static long read_best(void) {
    char buf[24];
    long n = fs_read(HISCORE_FILE, buf, sizeof(buf) - 1);
    if (n <= 0) return 0;
    if (n > (long)sizeof(buf) - 1) n = sizeof(buf) - 1;
    long v = 0;
    for (long i = 0; i < n && buf[i] >= '0' && buf[i] <= '9'; i++) v = v * 10 + (buf[i] - '0');
    return v;
}

// Writes label followed by v into out; `width` pads with leading zeros.
static void number_text(char* out, const char* label, long v, int width) {
    char digits[24];
    int n = 0;
    while (*label) *out++ = *label++;
    do { digits[n++] = '0' + v % 10; v /= 10; } while (v > 0);
    while (n < width) digits[n++] = '0';
    while (n > 0) *out++ = digits[--n];
    *out = '\0';
}

static void write_best(long v) {
    char buf[24];
    number_text(buf, "", v, 0);
    int n = 0;
    while (buf[n]) n++;
    fs_write(HISCORE_FILE, buf, n);
}

// Arcade text: a drop shadow keeps it readable over the snow.
static void text(int x, int y, const char* s, uint32_t color, int scale) {
    canvas_text(&c, x + scale, y + scale, s, rgb(20, 24, 60), scale);
    canvas_text(&c, x, y, s, color, scale);
}

static void text_centered(int y, const char* s, uint32_t color, int scale) {
    text((W - canvas_text_width(s, scale)) / 2, y, s, color, scale);
}

// ---- sound --------------------------------------------------------------------
// snd() holds one tone until told otherwise, so short tunes are stepped
// through a note at a time from the main loop instead of blocking it.

typedef struct { uint16_t hz, ms; } note_t;

static note_t tune[12];
static int tune_len, tune_pos;
static uint64_t note_end;

static void sound(const note_t* notes, int n) {
    if (state == TITLE) return;
    for (int i = 0; i < n && i < 12; i++) tune[i] = notes[i];
    tune_len = n;
    tune_pos = 0;
    snd(tune[0].hz);
    note_end = uptime_ms() + tune[0].ms;
}

static void sound_tick(void) {
    if (tune_pos >= tune_len || uptime_ms() < note_end) return;
    if (++tune_pos >= tune_len) {
        snd(0);
        return;
    }
    snd(tune[tune_pos].hz);
    note_end = uptime_ms() + tune[tune_pos].ms;
}

// ---- the slope ----------------------------------------------------------------

// The run falls away downhill (+z), steeper and gentler by turns, and rises
// into banks either side of the groomed trail.
static float slope_height(float x, float z) {
    double h = -0.28 * z + 3.0 * sin(z * 0.02);
    h += 0.45 * sin(x * 0.37 + z * 0.11) * sin(z * 0.29);
    double out = fabs(x) - (TRAIL - 2);
    if (out > 0) h += 0.5 * out + 0.6 * sin(x * 0.9 + z * 0.4);
    return (float)h;
}

// How far downhill the run falls per metre at z: what drives the skier on.
static double grade(double z) {
    return 0.28 - 0.06 * cos(z * 0.02);
}

// The height of the snow at (x, z), exactly as the grid of CELL-sized
// cells it's drawn as.
static double ground_height(double x, double z) {
    return r3d_heightfield_at(0, 0, CELL, slope_height, (float)x, (float)z);
}

// ---- the course ---------------------------------------------------------------

static obj_t* add_obj(int kind, double x, double z, double size) {
    for (int i = 0; i < MAX_OBJ; i++) {
        obj_t* o = &objs[i];
        if (o->alive) continue;
        *o = (obj_t){ kind, 1, x, ground_height(x, z), z, size, rand_range(0, 6.28), PENDING, 0 };
        return o;
    }
    return 0;
}

// The line from one gate to the next: obstacles stay clear of it, so there's
// always a way through.
static double racing_line(double z) {
    double t = clamp((z - prev_gate_z) / (next_gate_z - prev_gate_z), 0, 1);
    return prev_gate_x + t * (next_gate_x - prev_gate_x);
}

static void plan_gate(void) {
    double hard = clamp(next_gate_z / 3000, 0, 1);
    prev_gate_z = next_gate_z;
    prev_gate_x = next_gate_x;
    next_gate_z += rand_range(42, 56);
    double swing = 6 + 8 * hard;
    next_gate_x = clamp(prev_gate_x + rand_range(-swing, swing), -TRAIL + 6, TRAIL - 6);
}

// Lays out one row of the course, CELL metres deep, at built_z.
static void build_row(void) {
    double z = built_z;
    double hard = clamp(z / 3000, 0, 1);

    // Forest on the banks either side of the trail.
    for (int side = -1; side <= 1; side += 2) {
        for (int k = 0; k < 3; k++) {
            if (rand32() % 10 < (k == 2 ? 4 : 8)) {
                double x = side * (TRAIL + 1 + k * 5 + rand_range(-1.5, 1.5));
                add_obj(TREE, x, z + rand_range(0, CELL), rand_range(0.85, 1.3));
            }
        }
    }

    if (z >= next_gate_z) {
        obj_t* g = add_obj(GATE, next_gate_x, next_gate_z, 3.6 - 1.3 * hard);
        if (g) g->red = gate_count++ % 2;
        plan_gate();
    }

    // Trees and rocks on the trail, clear of the line between gates.
    if (z > 40 && rand32() % 100 < 22 + 30 * hard) {
        double x = rand_range(-TRAIL + 1, TRAIL - 1);
        double oz = z + rand_range(0, CELL);
        if (fabs(x - racing_line(oz)) > 4.5) {
            if (rand32() % 3) add_obj(TREE, x, oz, rand_range(0.7, 1.1));
            else add_obj(ROCK, x, oz, rand_range(0.6, 1.0));
        }
    }
    built_z += CELL;
}

static void reset(void) {
    for (int i = 0; i < MAX_OBJ; i++) objs[i].alive = 0;
    sx = 0;
    sz = 0;
    heading = speed = air = air_v = lean = crash_t = 0;
    steer = 0;
    cam_yaw = 0;
    cam_pitch = 0.4;
    built_z = -20;
    gate_count = 0;
    prev_gate_z = 0;
    prev_gate_x = 0;
    next_gate_z = 50;
    next_gate_x = rand_range(-6, 6);
    while (built_z < sz + AHEAD) build_row();
    time_left = START_TIME;
    score = gates_passed = combo = 0;
    msg_t = 0;
    new_best = 0;
}

// ---- the game -----------------------------------------------------------------

static void show(const char* text, uint32_t color) {
    msg = text;
    msg_color = color;
    msg_t = 1.0;
}

static void crash(const obj_t* o, double radius) {
    static const note_t notes[] = { { 300, 60 }, { 220, 60 }, { 160, 80 }, { 110, 160 } };
    // Push the skier back out of whatever they hit.
    double dx = sx - o->x, dz = sz - o->z, d = sqrt(dx * dx + dz * dz);
    if (d < 0.01) { dx = 0; dz = -1; d = 1; }
    sx = o->x + dx / d * (radius + 0.05);
    sz = o->z + dz / d * (radius + 0.05);
    crash_t = 1.4;
    crash_side = heading >= 0 ? 1 : -1;
    speed = 0;
    combo = 0;
    show("WIPEOUT!", rgb(255, 90, 70));
    sound(notes, 4);
}

static void pass_gate(obj_t* g) {
    if (state == OVER) return;
    if (fabs(sx - g->x) < g->size) {
        g->state = PASSED;
        gates_passed++;
        if (combo < 5) combo++;
        score += 50 * combo;
        time_left += 3.0 - clamp(sz / 4000, 0, 1.2);
        static char buf[24];
        number_text(buf, "GATE x", combo, 0);
        show(buf, rgb(120, 255, 140));
        note_t notes[] = { { (uint16_t)(660 + combo * 110), 50 }, { (uint16_t)(880 + combo * 147), 90 } };
        sound(notes, 2);
    } else {
        static const note_t notes[] = { { 150, 180 } };
        g->state = MISSED;
        combo = 0;
        show("MISSED", rgb(255, 120, 90));
        sound(notes, 1);
    }
}

static void jump(void) {
    static const note_t notes[] = { { 400, 30 }, { 600, 30 }, { 800, 40 } };
    if (air > 0 || crash_t > 0 || ready_t > 0) return;
    air_v = 5.0;
    air = 0.001;
    sound(notes, 3);
}

// The attract mode's skier aims for the next gate.
static void autopilot(void) {
    obj_t* next = 0;
    for (int i = 0; i < MAX_OBJ; i++) {
        obj_t* o = &objs[i];
        if (o->alive && o->kind == GATE && o->z > sz && (!next || o->z < next->z)) next = o;
    }
    if (!next) return;
    double want = atan2(next->x - sx, next->z - sz + 6);
    steer = (int)clamp(round(want / (M_PI / 8)), -3, 3);
}

static void update(double dt) {
    if (state == TITLE) autopilot();
    if (ready_t > 0) {
        ready_t -= dt;
        if (ready_t <= 0) show("GO!", rgb(255, 230, 80));
        return;
    }
    double prev_z = sz;
    double target = steer * M_PI / 8;
    double turn = (target - heading) * clamp(dt * 9, 0, 1);
    heading += turn;
    lean += (-turn / dt * 0.12 - lean) * clamp(dt * 8, 0, 1);
    lean = clamp(lean, -0.5, 0.5);

    if (crash_t > 0) {
        crash_t -= dt;
    } else {
        double a = 18 * grade(sz) * cos(heading) - 0.011 * speed * speed;
        if (air <= 0) {
            a -= 0.8 + 3.5 * fabs(sin(heading));
            if (fabs(sx) > TRAIL) a -= 4;  // deep powder off the trail
        }
        speed = fmax(0, speed + a * dt);
        sx += speed * sin(heading) * dt;
        sz += speed * cos(heading) * dt;
    }
    if (air > 0) {
        air_v -= 14 * dt;
        air += air_v * dt;
        if (air <= 0) air = 0;
    }

    for (int i = 0; i < MAX_OBJ; i++) {
        obj_t* o = &objs[i];
        if (!o->alive) continue;
        if (o->z < sz - 12) { o->alive = 0; continue; }
        if (o->kind == GATE) {
            if (o->state == PENDING && prev_z < o->z && sz >= o->z) pass_gate(o);
            continue;
        }
        if (crash_t > 0 || fabs(o->z - sz) > 2 || fabs(o->x - sx) > 2) continue;
        double r = o->kind == TREE ? 0.75 * o->size : 0.8 * o->size;
        double dx = o->x - sx, dz = o->z - sz;
        if (dx * dx + dz * dz < r * r && (o->kind == TREE || air < 0.6 * o->size)) crash(o, r);
    }
    while (built_z < sz + AHEAD) build_row();

    if (state == PLAYING) {
        long secs = (long)ceil(time_left);
        time_left -= dt;
        if (time_left < 5.5 && (long)ceil(time_left) != secs && time_left > 0) {
            static const note_t notes[] = { { 1200, 60 } };
            sound(notes, 1);
        }
        if (time_left <= 0) {
            static const note_t notes[] = { { 523, 150 }, { 392, 150 }, { 330, 150 }, { 262, 400 } };
            time_left = 0;
            state = OVER;
            steer = heading < 0 ? -4 : 4;  // skid to a stop
            over_t = 0;
            if (score > best) { best = score; write_best(best); new_best = 1; }
            sound(notes, 4);
        }
    }
    if (msg_t > 0) msg_t -= dt;
}

// The score counts every metre run as well as the gates.
static long total_score(void) {
    return score + (long)fmax(0, sz);
}

// ---- drawing ------------------------------------------------------------------

static void place_camera(double dt) {
    cam_yaw += (heading * 0.45 - cam_yaw) * clamp(dt * 3, 0, 1);
    double back = 6.0;
    r3d_vec cam;
    cam.x = (float)(sx - sin(cam_yaw) * back);
    cam.z = (float)(sz - cos(cam_yaw) * back);
    cam.y = (float)fmax(ground_height(sx, sz) + 2.9, ground_height(cam.x, cam.z) + 1.5);
    if (crash_t > 0) cam.y += (float)(0.12 * sin(crash_t * 40) * crash_t);

    double ahead = 8.0;
    double ty = ground_height(sx + sin(cam_yaw) * ahead, sz + cos(cam_yaw) * ahead) + 0.4;
    double want = atan2(cam.y - ty, back + ahead);
    cam_pitch += (want - cam_pitch) * clamp(dt * 4, 0, 1);
    // Aim a little above the skier, so they're low on the screen with
    // plenty of slope ahead.
    r3d_camera(&r, cam, (float)cam_yaw, (float)-(cam_pitch - 0.08));
}

static uint32_t snow(float x, float z) {
    // The groomer leaves the trail in stripes of corduroy.
    if (fabs(x) < TRAIL - 1 && (int)floor(z / CELL) % 2) return rgb(225, 231, 244);
    return rgb(236, 242, 255);
}

// A piece of a model, w by h by d, centred on (x, y, z) in the model.
static void part(float x, float y, float z, float w, float h, float d, uint32_t color) {
    r3d_push(&r);
    r3d_translate(&r, x, y, z);
    r3d_box(&r, w, h, d, color);
    r3d_pop(&r);
}

// A tier of a pine: a five-sided cone standing at height y in the model.
static void tier(float y, float radius, float height, uint32_t color) {
    r3d_push(&r);
    r3d_translate(&r, 0, y, 0);
    r3d_cone(&r, radius, height, 5, color);
    r3d_pop(&r);
}

static void draw_tree(const obj_t* o, float depth) {
    int tint = (int)(10 + 10 * sin(o->angle * 3));
    r3d_push(&r);
    r3d_translate(&r, (float)o->x, (float)o->y, (float)o->z);
    r3d_rotate_y(&r, (float)o->angle);
    r3d_scale(&r, (float)o->size, (float)o->size, (float)o->size);
    if (depth > 48) {
        tier(0.3f, 1.5f, 4.4f, rgb(30, 90 + tint, 62));  // far off, one cone will do
    } else {
        part(0, 0.4f, 0, 0.36f, 0.8f, 0.36f, rgb(110, 74, 50));
        tier(0.6f, 1.5f, 2.2f, rgb(30, 88 + tint, 60));
        tier(1.7f, 1.15f, 1.9f, rgb(36, 102 + tint, 68));
        tier(2.8f, 0.75f, 1.7f, rgb(200, 220, 230));  // a snowy top
    }
    r3d_pop(&r);
}

static void draw_rock(const obj_t* o) {
    r3d_push(&r);
    r3d_translate(&r, (float)o->x, (float)o->y, (float)o->z);
    r3d_rotate_y(&r, (float)o->angle);
    r3d_scale(&r, (float)o->size, (float)o->size * 0.6f, (float)o->size);
    r3d_sphere(&r, 0.9f, 3, rgb(120, 116, 124));
    r3d_translate(&r, 0, 0.32f, 0);
    r3d_scale(&r, 1, 0.5f, 1);
    r3d_sphere(&r, 0.6f, 3, rgb(236, 240, 255));  // snow on top
    r3d_pop(&r);
}

static void draw_gate(const obj_t* o) {
    uint32_t color = o->red ? rgb(230, 50, 50) : rgb(40, 50, 210);
    if (o->state == PASSED) color = rgb(255, 205, 40);
    if (o->state == MISSED) color = rgb(120, 120, 130);
    for (int side = -1; side <= 1; side += 2) {
        float x = (float)(o->x + side * o->size), z = (float)o->z;
        float y = (float)ground_height(x, z);
        part(x, y + 1, z, 0.12f, 2, 0.12f, rgb(250, 250, 250));
        r3d_triangle(&r, r3d_v(x, y + 2, z), r3d_v(x, y + 1.4f, z), r3d_v(x + side * 0.8f, y + 1.75f, z + 0.1f), color);
    }
}

static void draw_skier(void) {
    float ground = (float)ground_height(sx, sz);
    double yaw = heading, roll = lean, sink = 0;
    if (crash_t > 0) {
        double t = 1.4 - crash_t;
        roll = crash_side * fmin(1.45, t * 8);
        yaw += crash_side * fmin(1.2, t * 5);
        sink = 0.15;
        if (crash_t < 0.35) roll *= crash_t / 0.35;  // back on their feet
    }

    // The shadow, flat on the snow: it shows how high a jump is.
    double cy = cos(heading), sy = sin(heading);
    r3d_vec shadow[4];
    static const double corners[4][2] = { { -0.45, -0.9 }, { 0.45, -0.9 }, { 0.45, 0.9 }, { -0.45, 0.9 } };
    double spread = 1 + air * 0.4;
    for (int k = 0; k < 4; k++) {
        double lx = corners[k][0] * spread, lz = corners[k][1];
        float x = (float)(sx + lx * cy + lz * sy), z = (float)(sz - lx * sy + lz * cy);
        shadow[k] = r3d_v(x, (float)ground_height(x, z) + 0.06f, z);
    }
    r.unlit = 1;
    r3d_polygon(&r, shadow, 4, rgb(165, 175, 210));
    r.unlit = 0;

    r3d_push(&r);
    r3d_translate(&r, (float)sx, (float)(ground + air - sink), (float)sz);
    r3d_rotate_y(&r, (float)yaw);
    r3d_rotate_z(&r, (float)roll);
    part(-0.15f, 0.03f, 0.15f, 0.1f, 0.06f, 1.7f, rgb(250, 200, 30));   // skis
    part(0.15f, 0.03f, 0.15f, 0.1f, 0.06f, 1.7f, rgb(250, 200, 30));
    part(0, 0.45f, 0, 0.4f, 0.8f, 0.28f, rgb(40, 50, 120));             // legs
    part(0, 1.1f, 0.06f, 0.5f, 0.56f, 0.32f, rgb(225, 40, 45));         // jacket
    part(-0.33f, 0.9f, 0, 0.06f, 0.8f, 0.06f, rgb(60, 60, 70));          // poles
    part(0.33f, 0.9f, 0, 0.06f, 0.8f, 0.06f, rgb(60, 60, 70));
    part(0, 1.53f, 0.06f, 0.26f, 0.26f, 0.26f, rgb(240, 196, 160));     // head
    part(0, 1.69f, 0.06f, 0.28f, 0.12f, 0.28f, rgb(250, 250, 250));     // hat
    r3d_pop(&r);
}

// Mountains along the horizon, sliding as the camera turns.
static void draw_mountains(void) {
    int horizon = r3d_horizon(&r), w = r.screen.width, h = r.screen.height;
    int shift = (int)(cam_yaw * r.focal * 0.5);
    for (int x = 0; x < w; x++) {
        double ht = ridge[(x + shift) & 1023];
        int top = horizon - (int)ht;
        for (int y = top < 0 ? 0 : top; y < horizon + 12 && y < h; y++) {
            double t = clamp((y - top) / (ht + 12), 0, 1) * 0.8;
            int snowy = y - top < ht * 0.35;
            double cr = snowy ? 235 : 120, cg = snowy ? 240 : 135, cb = snowy ? 252 : 175;
            r.screen.pixels[y * w + x] = rgb((uint8_t)(cr + (HAZE_R - cr) * t), (uint8_t)(cg + (HAZE_G - cg) * t),
                                             (uint8_t)(cb + (HAZE_B - cb) * t));
        }
    }
}

static void make_ridge(void) {
    for (int i = 0; i < 1024; i++) {
        double a = i * 6.2831853 / 1024;
        double h = 16 + 9 * sin(a * 3) + 6 * sin(a * 7 + 1) + 3 * sin(a * 17 + 2);
        h += 4 * fabs(sin(a * 11));
        ridge[i] = h;
    }
}

static void draw_world(void) {
    r3d_begin(&r);
    draw_mountains();

    // The slope, from just behind the camera to the fog.
    int i0 = (int)floor(sx / CELL) - COLS, j0 = (int)floor((sz - 6) / CELL);
    r3d_heightfield(&r, i0 * CELL, j0 * CELL, 2 * COLS, ROWS, CELL, slope_height, snow);

    for (int k = 0; k < MAX_OBJ; k++) {
        obj_t* o = &objs[k];
        if (!o->alive) continue;
        float s = (float)o->size;
        r3d_vec at = r3d_v((float)o->x, (float)o->y + 1, (float)o->z);
        if (!r3d_visible(&r, at, o->kind == GATE ? s + 1 : 3 * s)) continue;
        int px, py;
        float depth = r3d_project(&r, at, &px, &py);
        // Don't let a tree right by the camera block the view of the skier.
        if (o->kind == TREE && depth > 0 && depth < 2.5 && px > 0 && px < W) continue;
        if (o->kind == TREE) draw_tree(o, depth);
        else if (o->kind == ROCK) draw_rock(o);
        else draw_gate(o);
    }
    draw_skier();
}

static void draw_snowfall(double dt) {
    int w = r.screen.width, h = r.screen.height;
    uint32_t* fb = r.screen.pixels;
    for (int i = 0; i < MAX_FLAKES; i++) {
        flake_t* f = &flakes[i];
        f->y += f->speed * dt * (1 + speed * 0.05);
        f->x += (sin(f->y * 0.05 + i) * 6 - heading * 25 * f->speed / 40) * dt;
        if (f->y >= h) { f->y -= h; f->x = rand_range(0, w); }
        if (f->x < 0) f->x += w;
        if (f->x >= w) f->x -= w;
        int x = (int)f->x, y = (int)f->y;
        fb[y * w + x] = rgb(255, 255, 255);
        if (f->speed > 45 && x + 1 < w && y + 1 < h) {
            fb[y * w + x + 1] = fb[(y + 1) * w + x] = fb[(y + 1) * w + x + 1] = rgb(255, 255, 255);
        }
    }
}

static void draw_hud(void) {
    char buf[40];
    uint32_t yellow = rgb(255, 230, 80), white = rgb(255, 255, 255);

    text(16, 12, "SCORE", rgb(120, 220, 255), 2);
    number_text(buf, "", total_score(), 6);
    text(16, 32, buf, white, 3);
    if (combo > 1) {
        number_text(buf, "COMBO x", combo, 0);
        text(16, 60, buf, rgb(120, 255, 140), 2);
    }

    text(W - 16 - canvas_text_width("HI", 2) - 96, 12, "HI", rgb(255, 140, 120), 2);
    number_text(buf, "", best > total_score() ? best : total_score(), 6);
    text(W - 16 - canvas_text_width(buf, 2), 12, buf, white, 2);

    long tenths = (long)(time_left * 10);
    number_text(buf, "", tenths / 10, 2);
    int n = 0;
    while (buf[n]) n++;
    buf[n] = '.';
    buf[n + 1] = '0' + tenths % 10;
    buf[n + 2] = '\0';
    int warn = time_left < 6 && (int)(time_left * 4) % 2;
    text_centered(8, "TIME", yellow, 2);
    text_centered(28, buf, warn ? rgb(255, 70, 60) : yellow, 4);

    number_text(buf, "", (long)(speed * 3.6), 3);
    int bx = 16, by = H - 40;
    text(bx, by, buf, white, 3);
    text(bx + canvas_text_width(buf, 3) + 6, by + 8, "KM/H", rgb(200, 210, 230), 2);
    number_text(buf, "", (long)fmax(0, sz), 0);
    int dn = 0;
    while (buf[dn]) dn++;
    buf[dn] = 'M';
    buf[dn + 1] = '\0';
    text(W - 16 - canvas_text_width(buf, 3), by, buf, white, 3);
}

static void draw(double dt) {
    place_camera(dt);
    draw_world();
    draw_snowfall(dt);
    r3d_end(&r);

    char buf[40];
    if (state == TITLE) {
        text_centered(28, "POWDER RUN", rgb(255, 230, 80), 7);
        text_centered(90, "DOWNHILL CHALLENGE", rgb(120, 220, 255), 2);
        number_text(buf, "HI SCORE ", best, 6);
        text_centered(114, buf, rgb(255, 140, 120), 2);
        text_centered(326, "LEFT/RIGHT OR MOUSE TO TURN", rgb(255, 255, 255), 2);
        text_centered(348, "DOWN: STRAIGHT   UP: STOP", rgb(255, 255, 255), 2);
        text_centered(370, "SPACE OR CLICK TO JUMP", rgb(255, 255, 255), 2);
        if ((int)(uptime_ms() / 500) % 2) text_centered(416, "PRESS SPACE TO START", rgb(255, 255, 255), 3);
        canvas_present(&c);
        return;
    }
    draw_hud();
    if (ready_t > 0) text_centered(190, "READY", rgb(255, 230, 80), 6);
    else if (msg_t > 0) text_centered(190, msg, msg_color, 4);
    if (state == OVER) {
        int bx = 120, by = 150, bw = W - 240, bh = 190;
        canvas_rect(&c, bx, by, bw, bh, rgb(16, 20, 56));
        canvas_rect_outline(&c, bx, by, bw, bh, rgb(255, 230, 80));
        canvas_rect_outline(&c, bx + 3, by + 3, bw - 6, bh - 6, rgb(255, 230, 80));
        text_centered(by + 18, "TIME UP", rgb(255, 230, 80), 4);
        number_text(buf, "SCORE ", total_score(), 6);
        text_centered(by + 66, buf, rgb(255, 255, 255), 3);
        number_text(buf, "GATES ", gates_passed, 0);
        text_centered(by + 100, buf, rgb(120, 220, 255), 2);
        if (new_best && (int)(uptime_ms() / 300) % 2) text_centered(by + 126, "NEW HI SCORE!", rgb(255, 140, 120), 2);
        if (over_t > 1.5) text_centered(by + 156, "SPACE TO RACE AGAIN", rgb(200, 210, 230), 2);
    }
    canvas_present(&c);
}

// ---- input --------------------------------------------------------------------

static void quit(void) {
    canvas_close(&c);
    writeln("powder: see you on the slopes");
    exit();
}

static void start(void) {
    static const note_t notes[] = { { 523, 120 }, { 0, 380 }, { 523, 120 }, { 0, 380 }, { 1046, 400 } };
    reset();
    state = PLAYING;
    ready_t = 1.0;
    show("", 0);
    sound(notes, 5);
}

static void press_action(void) {
    if (state == PLAYING) jump();
    else if (state == TITLE || over_t > 1.5) start();
}

static void turn(int by) {
    if (state != PLAYING) return;
    steer = (int)clamp(steer + by, -4, 4);
}

void _start(void) {
    if (canvas_open(&c, "Powder Run", W, H) != 0 || r3d_init(&r, &c, 2) != 0) {
        writeln("powder: couldn't open a window");
        exit();
    }
    // A bright winter's day: blue shadows on the snow, and haze far off.
    r.sun = r3d_v(-0.45f, 0.8f, -0.4f);
    r.ambient = r3d_v(0.6f, 0.6f, 0.72f);
    r.sun_light = r3d_v(0.4f, 0.4f, 0.28f);
    r.sky_top = rgb(70, 110, 200);
    r.sky_horizon = r.ground = r.fog_color = rgb(HAZE_R, HAZE_G, HAZE_B);
    r.fog_near = FOG_NEAR;
    r.fog_far = FOG_FAR;
    r.near = 0.25f;
    r.scanlines = 1;
    writeln("powder: arrows or mouse to turn, space to jump; escape quits");
    best = read_best();
    seed ^= uptime_ms() * 0x9e3779b97f4a7c15ull;
    make_ridge();
    for (int i = 0; i < MAX_FLAKES; i++) {
        flakes[i] = (flake_t){ rand_range(0, W / 2), rand_range(0, H / 2), rand_range(18, 60) };
    }
    reset();

    uint64_t last = uptime_ms();
    for (;;) {
        canvas_event_t e;
        while (canvas_poll(&c, &e) > 0) {
            if (e.kind == CANVAS_CLOSED) quit();
            if (e.kind == CANVAS_KEY) {
                int k = e.key;
                if (k == 27) quit();
                if (k == ' ' || k == '\n') press_action();
                if (k == CANVAS_KEY_LEFT || k == 'a') turn(-1);
                if (k == CANVAS_KEY_RIGHT || k == 'd') turn(1);
                if ((k == CANVAS_KEY_DOWN || k == 's') && state == PLAYING) steer = 0;
                if ((k == CANVAS_KEY_UP || k == 'w') && state == PLAYING) steer = heading < 0 ? -4 : 4;
            }
            if (e.kind == CANVAS_MOUSE_DOWN && e.buttons == CANVAS_LEFT) press_action();
            if (e.kind == CANVAS_MOUSE_MOVE && state == PLAYING) {
                steer = (int)clamp(round((e.x - W / 2.0) / (W / 2.0) * 5), -4, 4);
            }
        }

        uint64_t now = uptime_ms();
        double dt = clamp((now - last) / 1000.0, 0.001, 0.05);
        last = now;
        if (state == OVER) over_t += dt;
        update(dt);
        draw(dt);
        sound_tick();

        uint64_t spent = uptime_ms() - now;
        sleep_ms(spent < 25 ? 25 - spent : 10);
    }
}
