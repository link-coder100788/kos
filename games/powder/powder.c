// Powder Run: a 3D downhill ski game for KOS, in the style of an early-80s
// arcade cabinet.
//
// Left and right (or A and D, or the mouse) turn your skis; Down points them
// straight downhill and Up turns them across the slope to stop. Space or a
// click jumps, which clears rocks but not trees. Ski between the flags of
// each gate to win time and build a combo; when the clock runs out the run
// is over.
//
// The world is drawn by a small software renderer: flat-shaded polygons,
// clipped against the near plane and filled scanline by scanline into a
// 320x240 buffer, which is doubled up to the window with dimmed scanlines.
// The terrain is drawn row by row from the horizon in, each row's objects
// straight after it, so nearer things cover farther ones without a depth
// buffer.
//
// Escape quits. The best score is kept in /powder.hi.
#include "sys.h"
#include "canvas.h"
#include "math.h"
#include "print/print.h"

#define RW 320             // the 3D view's buffer
#define RH 240
#define W (RW * 2)         // the window
#define H (RH * 2)

#define FOCAL 230.0        // pixels: about a 70 degree view
#define CX (RW / 2.0)
#define CY (RH * 0.42)
#define NEAR 0.25

#define CELL 4.0           // terrain grid spacing, in metres
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

typedef struct { double x, y, z; } vec3;

enum { TREE, ROCK, GATE };
enum { PENDING, PASSED, MISSED };

typedef struct {
    int kind, alive;
    double x, y, z;        // on the ground; a gate's x is its middle
    double size;           // a gate's is its half-width
    double ca, sa;         // cos and sin of its turn about the vertical
    int state, red;        // gates only
} obj_t;

typedef struct { double x, y, speed; } flake_t;

enum { TITLE, PLAYING, OVER };

static canvas_t c;
static uint32_t fb[RW * RH];
static obj_t objs[MAX_OBJ];
static flake_t flakes[MAX_FLAKES];
static double ridge[1024];  // the mountains on the horizon

static int state = TITLE;
static uint64_t seed = 0x2545f4914f6cdd1dull;

// The skier.
static double sx, sz, heading, speed, air, air_v, lean, crash_t, crash_side;
static int steer;          // -4..4: how far the skis are turned, in 22.5 degree steps

// The camera.
static vec3 cam;
static double cam_yaw, cam_pitch, cyaw, syaw, cpitch, spitch;

// The course as it's built.
static double built_z, prev_gate_z, prev_gate_x, next_gate_z, next_gate_x;
static int gate_count;

static double time_left, ready_t, over_t, msg_t;
static long score, best, gates_passed, combo;
static const char* msg = "";
static uint32_t msg_color;
static int new_best;

static const vec3 SUN = { -0.45, 0.8, -0.4 };
static const vec3 HAZE = { 205, 216, 238 };

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

static vec3 v3(double x, double y, double z) { return (vec3){ x, y, z }; }
static vec3 sub(vec3 a, vec3 b) { return v3(a.x - b.x, a.y - b.y, a.z - b.z); }
static double dot(vec3 a, vec3 b) { return a.x * b.x + a.y * b.y + a.z * b.z; }

static vec3 cross(vec3 a, vec3 b) {
    return v3(a.y * b.z - a.z * b.y, a.z * b.x - a.x * b.z, a.x * b.y - a.y * b.x);
}

static vec3 normalize(vec3 a) {
    double l = sqrt(dot(a, a));
    return l > 0 ? v3(a.x / l, a.y / l, a.z / l) : a;
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
static double vertex_height(int i, int j) {
    double x = i * CELL, z = j * CELL;
    double h = -0.28 * z + 3.0 * sin(z * 0.02);
    h += 0.45 * sin(x * 0.37 + z * 0.11) * sin(z * 0.29);
    double out = fabs(x) - (TRAIL - 2);
    if (out > 0) h += 0.5 * out + 0.6 * sin(x * 0.9 + z * 0.4);
    return h;
}

// How far downhill the run falls per metre at z: what drives the skier on.
static double grade(double z) {
    return 0.28 - 0.06 * cos(z * 0.02);
}

// The height of the drawn terrain at (x, z): each grid cell is split into
// two flat triangles along its diagonal, as it's drawn.
static double ground_height(double x, double z) {
    int i = (int)floor(x / CELL), j = (int)floor(z / CELL);
    double fx = x / CELL - i, fz = z / CELL - j;
    double h00 = vertex_height(i, j), h11 = vertex_height(i + 1, j + 1);
    if (fx >= fz) {
        double h10 = vertex_height(i + 1, j);
        return h00 + fx * (h10 - h00) + fz * (h11 - h10);
    }
    double h01 = vertex_height(i, j + 1);
    return h00 + fz * (h01 - h00) + fx * (h11 - h01);
}

// ---- the course ---------------------------------------------------------------

static obj_t* add_obj(int kind, double x, double z, double size) {
    for (int i = 0; i < MAX_OBJ; i++) {
        obj_t* o = &objs[i];
        if (o->alive) continue;
        double a = rand_range(0, 6.28);
        *o = (obj_t){ kind, 1, x, ground_height(x, z), z, size, cos(a), sin(a), PENDING, 0 };
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

// ---- the renderer ---------------------------------------------------------------

static void place_camera(double dt) {
    cam_yaw += (heading * 0.45 - cam_yaw) * clamp(dt * 3, 0, 1);
    double back = 6.0;
    cam.x = sx - sin(cam_yaw) * back;
    cam.z = sz - cos(cam_yaw) * back;
    cam.y = fmax(ground_height(sx, sz) + 2.9, ground_height(cam.x, cam.z) + 1.5);
    if (crash_t > 0) cam.y += 0.12 * sin(crash_t * 40) * crash_t;

    double ahead = 8.0;
    double ty = ground_height(sx + sin(cam_yaw) * ahead, sz + cos(cam_yaw) * ahead) + 0.4;
    double want = atan2(cam.y - ty, back + ahead);
    cam_pitch += (want - cam_pitch) * clamp(dt * 4, 0, 1);

    cyaw = cos(cam_yaw);
    syaw = sin(cam_yaw);
    cpitch = cos(cam_pitch);
    spitch = sin(cam_pitch);
}

// A point in the camera's frame: x right, y up, z into the screen.
static vec3 view(vec3 p) {
    double dx = p.x - cam.x, dy = p.y - cam.y, dz = p.z - cam.z;
    double x = dx * cyaw - dz * syaw, z = dx * syaw + dz * cyaw;
    return v3(x, dy * cpitch + z * spitch, z * cpitch - dy * spitch);
}

static int iceil(double v) {
    int i = (int)v;
    return i < v ? i + 1 : i;
}

// Fills a convex polygon given in screen coordinates.
static void fill_poly(const double* px, const double* py, int n, uint32_t color) {
    double top = py[0], bottom = py[0];
    for (int i = 1; i < n; i++) {
        if (py[i] < top) top = py[i];
        if (py[i] > bottom) bottom = py[i];
    }
    int y0 = iceil(clamp(top, -1, RH + 1) - 0.5), y1 = iceil(clamp(bottom, -1, RH + 1) - 0.5);
    if (y0 < 0) y0 = 0;
    if (y1 > RH) y1 = RH;
    for (int y = y0; y < y1; y++) {
        double yc = y + 0.5, lo = 1e9, hi = -1e9;
        for (int i = 0, j = n - 1; i < n; j = i++) {
            if ((py[j] <= yc) == (py[i] <= yc)) continue;
            double x = px[j] + (yc - py[j]) * (px[i] - px[j]) / (py[i] - py[j]);
            if (x < lo) lo = x;
            if (x > hi) hi = x;
        }
        if (lo > hi) continue;
        int x0 = iceil(clamp(lo, -1, RW + 1) - 0.5), x1 = iceil(clamp(hi, -1, RW + 1) - 0.5);
        if (x0 < 0) x0 = 0;
        if (x1 > RW) x1 = RW;
        uint32_t* row = fb + y * RW;
        for (int x = x0; x < x1; x++) row[x] = color;
    }
}

// Draws a polygon given in the camera's frame, cutting away whatever is
// behind the near plane.
static void draw_view_poly(const vec3* v, int n, uint32_t color) {
    double px[16], py[16];
    int m = 0;
    for (int i = 0; i < n; i++) {
        vec3 a = v[i], b = v[(i + 1) % n];
        int a_in = a.z >= NEAR, b_in = b.z >= NEAR;
        if (a_in) {
            px[m] = CX + FOCAL * a.x / a.z;
            py[m] = CY - FOCAL * a.y / a.z;
            m++;
        }
        if (a_in != b_in) {
            double t = (NEAR - a.z) / (b.z - a.z);
            px[m] = CX + FOCAL * (a.x + t * (b.x - a.x)) / NEAR;
            py[m] = CY - FOCAL * (a.y + t * (b.y - a.y)) / NEAR;
            m++;
        }
    }
    if (m >= 3) fill_poly(px, py, m, color);
}

// A colour lit by the sun from normal n, fading into the haze with depth.
static uint32_t shade(double r, double g, double b, vec3 n, double depth) {
    double d = dot(n, SUN);
    double l = d > 0 ? 0.68 + 0.32 * d : 0.68 + 0.22 * d;
    r *= l;
    g *= l;
    b *= 0.25 + 0.75 * l;  // shadows go blue, like snow
    double t = clamp((depth - FOG_NEAR) / (FOG_FAR - FOG_NEAR), 0, 1);
    r += (HAZE.x - r) * t;
    g += (HAZE.y - g) * t;
    b += (HAZE.z - b) * t;
    return rgb((uint8_t)clamp(r, 0, 255), (uint8_t)clamp(g, 0, 255), (uint8_t)clamp(b, 0, 255));
}

// Draws one face of a solid, convex object (given in world space) if it
// faces the camera. `inside` is any point within the object.
static void solid_face(const vec3* v, int n, vec3 inside, double r, double g, double b) {
    vec3 normal = normalize(cross(sub(v[1], v[0]), sub(v[2], v[0])));
    if (dot(normal, sub(v[0], inside)) < 0) normal = v3(-normal.x, -normal.y, -normal.z);
    if (dot(normal, sub(v[0], cam)) >= 0) return;
    vec3 vv[8];
    for (int i = 0; i < n; i++) vv[i] = view(v[i]);
    draw_view_poly(vv, n, shade(r, g, b, normal, vv[0].z));
}

// A face seen from both sides, like a flag.
static void flat_face(const vec3* v, int n, double r, double g, double b) {
    vec3 normal = normalize(cross(sub(v[1], v[0]), sub(v[2], v[0])));
    if (dot(normal, sub(v[0], cam)) > 0) normal = v3(-normal.x, -normal.y, -normal.z);
    vec3 vv[8];
    for (int i = 0; i < n; i++) vv[i] = view(v[i]);
    draw_view_poly(vv, n, shade(r, g, b, normal, vv[0].z));
}

// Where a model's local axes point, for the boxes the skier is built from.
static vec3 m_pos, m_x, m_y, m_z;

static vec3 model(double x, double y, double z) {
    return v3(m_pos.x + m_x.x * x + m_y.x * y + m_z.x * z,
              m_pos.y + m_x.y * x + m_y.y * y + m_z.y * z,
              m_pos.z + m_x.z * x + m_y.z * y + m_z.z * z);
}

// A box centred on (x, y, z) in model space, hx by hy by hz from centre to side.
static void box(double x, double y, double z, double hx, double hy, double hz, double r, double g, double b) {
    static const int faces[6][4] = {
        { 0, 2, 6, 4 }, { 1, 3, 7, 5 }, { 0, 1, 5, 4 }, { 2, 3, 7, 6 }, { 0, 1, 3, 2 }, { 4, 5, 7, 6 },
    };
    vec3 p[8];
    for (int k = 0; k < 8; k++) {
        p[k] = model(x + (k & 1 ? hx : -hx), y + (k & 2 ? hy : -hy), z + (k & 4 ? hz : -hz));
    }
    vec3 inside = model(x, y, z);
    for (int f = 0; f < 6; f++) {
        vec3 quad[4] = { p[faces[f][0]], p[faces[f][1]], p[faces[f][2]], p[faces[f][3]] };
        solid_face(quad, 4, inside, r, g, b);
    }
}

// The model axes for something standing at p, turned by yaw and rolled.
static void set_model(vec3 p, double yaw, double roll) {
    double cy = cos(yaw), sy = sin(yaw), cr = cos(roll), sr = sin(roll);
    m_pos = p;
    m_x = v3(cr * cy, sr, -cr * sy);
    m_y = v3(-sr * cy, cr, sr * sy);
    m_z = v3(sy, 0, cy);
}

// A pine tier: a five-sided cone standing on (x, y, z).
static void cone(const obj_t* o, double y, double h, double radius, double r, double g, double b) {
    vec3 apex = v3(o->x, y + h, o->z), inside = v3(o->x, y + h * 0.3, o->z);
    vec3 ring[5];
    for (int k = 0; k < 5; k++) {
        static const double ck[5] = { 1, 0.309017, -0.809017, -0.809017, 0.309017 };
        static const double sk[5] = { 0, 0.951057, 0.587785, -0.587785, -0.951057 };
        double cx = ck[k] * o->ca - sk[k] * o->sa, sz_ = sk[k] * o->ca + ck[k] * o->sa;
        ring[k] = v3(o->x + radius * cx, y, o->z + radius * sz_);
    }
    for (int k = 0; k < 5; k++) {
        vec3 tri[3] = { apex, ring[k], ring[(k + 1) % 5] };
        solid_face(tri, 3, inside, r, g, b);
    }
}

static void draw_tree(const obj_t* o, double depth) {
    double s = o->size, y = o->y;
    double tint = (o->ca + 1) * 10;
    if (depth > 48) {
        cone(o, y + 0.3 * s, 4.4 * s, 1.5 * s, 30, 90 + tint, 62);
        return;
    }
    vec3 p = v3(o->x, y, o->z);
    set_model(p, 0, 0);
    box(0, 0.4 * s, 0, 0.18 * s, 0.4 * s, 0.18 * s, 110, 74, 50);
    cone(o, y + 0.6 * s, 2.2 * s, 1.5 * s, 30, 88 + tint, 60);
    cone(o, y + 1.7 * s, 1.9 * s, 1.15 * s, 36, 102 + tint, 68);
    cone(o, y + 2.8 * s, 1.7 * s, 0.75 * s, 200, 220, 230);  // a snowy top
}

static void draw_rock(const obj_t* o) {
    double s = o->size;
    vec3 lo[5], hi[5];
    vec3 inside = v3(o->x, o->y + 0.2 * s, o->z);
    for (int k = 0; k < 5; k++) {
        double a = k * 1.2566 + o->ca;
        double r = s * (0.9 + 0.15 * ((k * 7 + (int)(o->sa * 10)) % 3));
        lo[k] = v3(o->x + r * cos(a), o->y - 0.15, o->z + r * sin(a));
        hi[k] = v3(o->x + 0.55 * r * cos(a + 0.3), o->y + 0.7 * s, o->z + 0.55 * r * sin(a + 0.3));
    }
    for (int k = 0; k < 5; k++) {
        int n = (k + 1) % 5;
        vec3 side[4] = { lo[k], lo[n], hi[n], hi[k] };
        solid_face(side, 4, inside, 120, 116, 124);
    }
    solid_face(hi, 5, inside, 236, 240, 255);
}

static void draw_gate(const obj_t* o) {
    double r = o->red ? 230 : 40, g = 50, b = o->red ? 50 : 210;
    if (o->state == PASSED) { r = 255; g = 205; b = 40; }
    if (o->state == MISSED) { r = 120; g = 120; b = 130; }
    for (int side = -1; side <= 1; side += 2) {
        double x = o->x + side * o->size;
        double y = ground_height(x, o->z);
        set_model(v3(x, y, o->z), 0, 0);
        box(0, 1.0, 0, 0.06, 1.0, 0.06, 250, 250, 250);
        vec3 flag[3] = { v3(x, y + 2.0, o->z), v3(x, y + 1.4, o->z), v3(x + side * 0.8, y + 1.75, o->z + 0.1) };
        flat_face(flag, 3, r, g, b);
    }
}

static void draw_skier(void) {
    double ground = ground_height(sx, sz);
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
    vec3 shadow[4];
    static const double corners[4][2] = { { -0.45, -0.9 }, { 0.45, -0.9 }, { 0.45, 0.9 }, { -0.45, 0.9 } };
    double spread = 1 + air * 0.4;
    for (int k = 0; k < 4; k++) {
        double lx = corners[k][0] * spread, lz = corners[k][1];
        double x = sx + lx * cy + lz * sy, z = sz - lx * sy + lz * cy;
        shadow[k] = v3(x, ground_height(x, z) + 0.04, z);
    }
    flat_face(shadow, 4, 170, 180, 215);

    set_model(v3(sx, ground + air - sink, sz), yaw, roll);
    box(-0.15, 0.03, 0.15, 0.05, 0.03, 0.85, 250, 200, 30);   // skis
    box(0.15, 0.03, 0.15, 0.05, 0.03, 0.85, 250, 200, 30);
    box(0, 0.45, 0, 0.2, 0.4, 0.14, 40, 50, 120);             // legs
    box(0, 1.1, 0.06, 0.25, 0.28, 0.16, 225, 40, 45);         // jacket
    box(-0.33, 0.9, 0.0, 0.03, 0.4, 0.03, 60, 60, 70);         // poles
    box(0.33, 0.9, 0.0, 0.03, 0.4, 0.03, 60, 60, 70);
    box(0, 1.53, 0.06, 0.13, 0.13, 0.13, 240, 196, 160);      // head
    box(0, 1.69, 0.06, 0.14, 0.06, 0.14, 250, 250, 250);      // hat
}

// The sky, and mountains along the horizon that slide as the camera turns.
static void draw_sky(void) {
    int horizon = (int)(CY - FOCAL * tan(cam_pitch));
    for (int y = 0; y < RH; y++) {
        uint32_t color;
        if (y < horizon) {
            double t = clamp((double)y / (horizon > 1 ? horizon : 1), 0, 1);
            color = rgb((uint8_t)(70 + t * 135), (uint8_t)(110 + t * 106), (uint8_t)(200 + t * 38));
        } else {
            color = rgb((uint8_t)HAZE.x, (uint8_t)HAZE.y, (uint8_t)HAZE.z);
        }
        uint32_t* row = fb + y * RW;
        for (int x = 0; x < RW; x++) row[x] = color;
    }
    int shift = (int)(cam_yaw * FOCAL * 0.5);
    for (int x = 0; x < RW; x++) {
        double h = ridge[(x + shift) & 1023];
        int top = horizon - (int)h;
        for (int y = top < 0 ? 0 : top; y < horizon + 12 && y < RH; y++) {
            double t = clamp((y - top) / (h + 12), 0, 1);
            int snowy = y - top < h * 0.35;
            double r = snowy ? 235 : 120, g = snowy ? 240 : 135, b = snowy ? 252 : 175;
            r += (HAZE.x - r) * t * 0.8;
            g += (HAZE.y - g) * t * 0.8;
            b += (HAZE.z - b) * t * 0.8;
            fb[y * RW + x] = rgb((uint8_t)r, (uint8_t)g, (uint8_t)b);
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

static vec3 grid_world[ROWS + 1][2 * COLS + 1];
static vec3 grid_view[ROWS + 1][2 * COLS + 1];
static int row_head[ROWS], obj_next[MAX_OBJ + 1];
static double obj_depth[MAX_OBJ + 1];
#define SKIER MAX_OBJ      // the skier's slot in the draw lists

static void draw_terrain_cell(int r, int col, int j, int i) {
    for (int t = 0; t < 2; t++) {
        // The two triangles of the cell, split the same way as ground_height.
        int a[3][2] = { { 0, 0 }, { 1, 0 }, { 1, 1 } };
        if (t) { a[1][0] = 1; a[1][1] = 1; a[2][0] = 0; a[2][1] = 1; }
        vec3 w[3], v[3];
        for (int k = 0; k < 3; k++) {
            w[k] = grid_world[r + a[k][1]][col + a[k][0]];
            v[k] = grid_view[r + a[k][1]][col + a[k][0]];
        }
        if (v[0].z < NEAR && v[1].z < NEAR && v[2].z < NEAR) continue;
        vec3 normal = normalize(cross(sub(w[2], w[0]), sub(w[1], w[0])));
        if (normal.y < 0) normal = v3(-normal.x, -normal.y, -normal.z);
        double depth = (v[0].z + v[1].z + v[2].z) / 3;
        double x = (i + 0.5) * CELL;
        double l = 1.0;
        if (fabs(x) < TRAIL - 1 && (j & 1)) l = 0.955;  // the groomer's corduroy
        draw_view_poly(v, 3, shade(236 * l, 242 * l, 255 * l, normal, depth));
    }
}

static void draw_objects_in_row(int r) {
    for (int k = row_head[r]; k >= 0; k = obj_next[k]) {
        if (k == SKIER) { draw_skier(); continue; }
        obj_t* o = &objs[k];
        if (o->kind == TREE) draw_tree(o, obj_depth[k]);
        else if (o->kind == ROCK) draw_rock(o);
        else draw_gate(o);
    }
}

// Files object k under the terrain row its nearest edge is in, keeping
// each row's list sorted far to near. It reaches `reach` metres either way
// across the slope and `depth` metres up and down it.
static void file_object(int k, double x, double y, double z, double reach, double depth, int j0) {
    vec3 v = view(v3(x, y, z));
    if (v.z < -reach - 1) return;
    // Don't let something right by the camera block the view of the skier.
    if (k != SKIER && v.z < 2.5 && fabs(v.x) < reach + 1) return;
    if (fabs(v.x) > (v.z + reach) * (CX / FOCAL) + reach + 1) return;
    int r = (int)floor((z - depth) / CELL) - j0;
    if (r < 0) r = 0;
    if (r >= ROWS) return;
    obj_depth[k] = v.z;
    int* p = &row_head[r];
    while (*p >= 0 && obj_depth[*p] > v.z) p = &obj_next[*p];
    obj_next[k] = *p;
    *p = k;
}

static void draw_world(void) {
    draw_sky();

    int j0 = (int)floor(cam.z / CELL), i0 = (int)floor(cam.x / CELL) - COLS;
    for (int r = 0; r <= ROWS; r++) {
        for (int col = 0; col <= 2 * COLS; col++) {
            int i = i0 + col, j = j0 + r;
            vec3 w = v3(i * CELL, vertex_height(i, j), j * CELL);
            grid_world[r][col] = w;
            grid_view[r][col] = view(w);
        }
    }

    for (int r = 0; r < ROWS; r++) row_head[r] = -1;
    for (int k = 0; k < MAX_OBJ; k++) {
        obj_t* o = &objs[k];
        if (!o->alive) continue;
        if (o->kind == GATE) {
            file_object(k, o->x, o->y + 1, o->z, o->size + 1, 0.2, j0);
        } else {
            double reach = o->kind == TREE ? 1.6 * o->size : 1.2 * o->size;
            file_object(k, o->x, o->y + 1, o->z, reach, reach, j0);
        }
    }
    file_object(SKIER, sx, ground_height(sx, sz) + 1, sz, 1.3, 1.3, j0);

    for (int r = ROWS - 1; r >= 0; r--) {
        // Outside in, so the banks are drawn before the trail they look down on.
        for (int col = 0; col < COLS; col++) draw_terrain_cell(r, col, j0 + r, i0 + col);
        for (int col = 2 * COLS - 1; col >= COLS; col--) draw_terrain_cell(r, col, j0 + r, i0 + col);
        draw_objects_in_row(r);
    }
}

static void draw_snowfall(double dt) {
    for (int i = 0; i < MAX_FLAKES; i++) {
        flake_t* f = &flakes[i];
        f->y += f->speed * dt * (1 + speed * 0.05);
        f->x += (sin(f->y * 0.05 + i) * 6 - heading * 25 * f->speed / 40) * dt;
        if (f->y >= RH) { f->y -= RH; f->x = rand_range(0, RW); }
        if (f->x < 0) f->x += RW;
        if (f->x >= RW) f->x -= RW;
        int x = (int)f->x, y = (int)f->y;
        fb[y * RW + x] = rgb(255, 255, 255);
        if (f->speed > 45 && x + 1 < RW && y + 1 < RH) {
            fb[y * RW + x + 1] = fb[(y + 1) * RW + x] = fb[(y + 1) * RW + x + 1] = rgb(255, 255, 255);
        }
    }
}

// Doubles the buffer up into the window, every other line dimmed like a
// cabinet's monitor.
static void blit(void) {
    for (int y = 0; y < RH; y++) {
        const uint32_t* src = fb + y * RW;
        uint32_t* a = c.pixels + (2 * y) * W;
        uint32_t* b = a + W;
        for (int x = 0; x < RW; x++) {
            uint32_t p = src[x];
            uint32_t dim = ((p >> 1) & 0x7f7f7f) + ((p >> 2) & 0x3f3f3f);
            a[2 * x] = a[2 * x + 1] = p;
            b[2 * x] = b[2 * x + 1] = dim;
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
    blit();

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
    if (canvas_open(&c, "Powder Run", W, H) != 0) {
        writeln("powder: couldn't open a window");
        exit();
    }
    writeln("powder: arrows or mouse to turn, space to jump; escape quits");
    best = read_best();
    seed ^= uptime_ms() * 0x9e3779b97f4a7c15ull;
    make_ridge();
    for (int i = 0; i < MAX_FLAKES; i++) {
        flakes[i] = (flake_t){ rand_range(0, RW), rand_range(0, RH), rand_range(18, 60) };
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
