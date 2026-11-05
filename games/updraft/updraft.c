// Updraft: a 3D paper glider game for KOS. Glide down a winding desert
// canyon at dusk, trading height for speed: diving speeds you up, climbing
// slows you down, and too slow, you stall and the nose drops.
//
// The mouse flies the glider: left and right bank it into a turn, up and
// down point the nose. The arrow keys (or WASD) do the same a step at a
// time. Fly through the rings for points and a burst of speed, ride the
// thermals (rising dust) for height, and duck under or hop over the stone
// arches. Touching rock ends the flight.
//
// The world is drawn with libc's render3d.h at half the window's size.
//
// Escape quits. The best score is kept in /games/updraft.hi.
#include "sys.h"
#include "canvas.h"
#include "math.h"
#include "render3d.h"

#define W 640              // the window; the 3D view is half this
#define H 480

#define CELL 4.0f          // terrain grid spacing, in metres
#define ROWS 23            // terrain rows drawn ahead of the camera
#define COLS 13            // terrain cells drawn either side of it
#define FOG_NEAR 34.0
#define FOG_FAR 88.0

#define CEILING 26.0       // the wind over the rim won't let you higher
#define RIM 34.0           // how high the canyon walls rise above the floor
#define MAX_OBJ 160
#define AHEAD 110.0        // how far ahead the course is built
#define STEP 8.0           // the course is built this many metres at a time

#define GRAVITY 9.8
#define STALL 7.0          // slower than this, the glider stalls
#define START_SPEED 15.0
#define MAX_SPEED 34.0

#define HISCORE_FILE "/games/updraft.hi"

enum { RING, THERMAL, ARCH, SPIRE };
enum { PENDING, PASSED, MISSED };

typedef struct {
    int kind, alive;
    // A ring's middle; a thermal's or spire's foot on the floor; the middle
    // of an arch's underside.
    double x, y, z;
    double size;           // a ring's or thermal's radius, a spire's height, an arch's thickness
    double angle;          // a spire's turn about the vertical
    int state;             // rings only
} obj_t;

enum { TITLE, PLAYING, OVER };

static canvas_t c;
static r3d_t r;
static obj_t objs[MAX_OBJ];

static int state = TITLE;
static uint64_t seed = 0x9e3779b97f4a7c15ull;

// The glider: yaw turns it right, gamma is the angle it climbs at (negative
// is a dive) and roll how far it banks right.
static double gx, gy, gz, yaw, gamma_, roll, speed;
static double want_roll, want_gamma;
static double lift;        // how hard a thermal is pushing it up, for the HUD
static int stalled;

static double cam_yaw;
static r3d_vec cam_eye;   // where the camera stays once you've crashed

// The course as it's built: rings are planned one ahead, so obstacles can
// keep clear of the line between them.
static double built_z, prev_ring_x, prev_ring_y, prev_ring_z, next_ring_x, next_ring_y, next_ring_z;

static double ready_t, over_t, msg_t, crash_t, clock_t_;
static long score, best, rings_passed, combo;
static const char* msg = "";
static uint32_t msg_color;
static int new_best;

#define DUSK_R 236
#define DUSK_G 150
#define DUSK_B 110

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

// Text with a drop shadow, to read over the sky and the rock.
static void text(int x, int y, const char* s, uint32_t color, int scale) {
    canvas_text(&c, x + scale, y + scale, s, rgb(50, 20, 40), scale);
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

// ---- the canyon ---------------------------------------------------------------

// Where the middle of the canyon is at z: it winds left and right.
static double canyon_x(double z) {
    return 22 * sin(z * 0.009) + 9 * sin(z * 0.023 + 1.3);
}

// Half the width of the canyon's floor, narrowing the further you get.
static double canyon_half(double z) {
    return 15 - 6 * clamp(z / 4000, 0, 1) + 3 * sin(z * 0.017);
}

static double floor_y(double z) {
    return 2 * sin(z * 0.013);
}

// A sandy floor between walls that rise steeply to the rim, in ledges.
static float canyon_height(float x, float z) {
    double h = floor_y(z) + 0.5 * sin(x * 0.31 + z * 0.17) * sin(z * 0.23);
    double d = fabs(x - canyon_x(z)) - canyon_half(z);
    if (d > 0) {
        double wall = fmin(d * 2.6, RIM);
        h += wall + 1.6 * sin(x * 0.7 + z * 0.3) * fmin(1, d / 3);
    }
    return (float)h;
}

// The height of the rock at (x, z), exactly as the grid of CELL-sized cells
// it's drawn as.
static double ground_height(double x, double z) {
    return r3d_heightfield_at(0, 0, CELL, canyon_height, (float)x, (float)z);
}

// Sandstone, in strata by height, with a pale sandy floor.
static uint32_t rock_color(float x, float z) {
    static const uint32_t strata[5] = {
        0xc8643c, 0xd88a52, 0xb4503a, 0xe0a468, 0xa84a3c,
    };
    double h = canyon_height(x, z) - floor_y(z);
    if (h < 1.2) return ((int)floor(x / CELL) + (int)floor(z / CELL)) & 1 ? 0xe2b88a : 0xdcb084;
    int band = (int)floor((h + 1.3 * sin(z * 0.05)) / 4.5);
    return strata[((band % 5) + 5) % 5];
}

// ---- the course ---------------------------------------------------------------

static obj_t* add_obj(int kind, double x, double y, double z, double size) {
    for (int i = 0; i < MAX_OBJ; i++) {
        obj_t* o = &objs[i];
        if (o->alive) continue;
        *o = (obj_t){ kind, 1, x, y, z, size, rand_range(0, 6.28), PENDING };
        return o;
    }
    return 0;
}

static void plan_ring(void) {
    double hard = clamp(next_ring_z / 4000, 0, 1);
    prev_ring_x = next_ring_x;
    prev_ring_y = next_ring_y;
    prev_ring_z = next_ring_z;
    next_ring_z += rand_range(55, 70) + 25 * hard;
    double half = canyon_half(next_ring_z) - 4;
    next_ring_x = canyon_x(next_ring_z) + rand_range(-half, half) * (0.4 + 0.5 * hard);
    double fl = floor_y(next_ring_z);
    next_ring_y = clamp(prev_ring_y + rand_range(-7, 7), fl + 4, CEILING - 3);
}

// The line from one ring to the next, which obstacles stay clear of.
static double line_t(double z) {
    return clamp((z - prev_ring_z) / (next_ring_z - prev_ring_z), 0, 1);
}

// Lays out STEP metres of the course at built_z.
static void build_step(void) {
    double z = built_z;
    double hard = clamp(z / 4000, 0, 1);

    if (z + STEP > next_ring_z) {
        add_obj(RING, next_ring_x, next_ring_y, next_ring_z, 2.9 - 0.8 * hard);
        plan_ring();
    }
    if (z < 120) { built_z += STEP; return; }

    double oz = z + rand_range(0, STEP);
    double t = line_t(oz);
    double lx = prev_ring_x + t * (next_ring_x - prev_ring_x);
    double ly = prev_ring_y + t * (next_ring_y - prev_ring_y);
    double fl = floor_y(oz), half = canyon_half(oz), cx = canyon_x(oz);
    int clear_of_rings = oz - prev_ring_z > 14 && next_ring_z - oz > 14;
    int roll = rand32() % 100;

    if (roll < 7 + 6 * hard && clear_of_rings) {
        // An arch across the canyon, under or over the line between rings.
        double th = 3;
        double ay = ly - fl > 11 ? fmax(fl - 2, ly - 4.5 - th) : ly + 4.5;
        if (ay + th < CEILING + 6) add_obj(ARCH, cx, ay, oz, th);
    } else if (roll < 30 + 20 * hard) {
        double x = cx + rand_range(-half + 2, half - 2);
        if (fabs(x - lx) > 6) add_obj(SPIRE, x, ground_height(x, oz), oz, rand_range(9, CEILING + 6));
    } else if (roll < 42) {
        double x = cx + rand_range(-half + 4, half - 4);
        add_obj(THERMAL, x, fl, oz, 5.5);
    }
    built_z += STEP;
}

static void reset(void) {
    for (int i = 0; i < MAX_OBJ; i++) objs[i].alive = 0;
    gz = 0;
    gx = canyon_x(gz);
    gy = 14;
    yaw = 0;
    gamma_ = want_gamma = -0.1;
    roll = want_roll = 0;
    speed = START_SPEED;
    lift = 0;
    stalled = 0;
    cam_yaw = 0;
    crash_t = 0;
    built_z = -16;
    prev_ring_z = 0;
    next_ring_x = canyon_x(50);
    next_ring_y = 12;
    next_ring_z = 50;
    while (built_z < gz + AHEAD) build_step();
    score = rings_passed = combo = 0;
    msg_t = 0;
    new_best = 0;
}

// ---- the game -----------------------------------------------------------------

static void show(const char* text, uint32_t color) {
    msg = text;
    msg_color = color;
    msg_t = 1.0;
}

// The score counts every metre flown as well as the rings.
static long total_score(void) {
    return score + (long)fmax(0, gz);
}

static void crash(void) {
    static const note_t notes[] = { { 320, 70 }, { 240, 70 }, { 170, 90 }, { 110, 260 } };
    crash_t = 0.001;
    speed = 0;
    combo = 0;
    if (state == TITLE) return;
    state = OVER;
    over_t = 0;
    show("CRASHED!", rgb(255, 90, 70));
    if (total_score() > best) { best = total_score(); write_best(best); new_best = 1; }
    sound(notes, 4);
}

static void pass_ring(obj_t* o) {
    double dx = gx - o->x, dy = gy - o->y;
    if (dx * dx + dy * dy < o->size * o->size) {
        o->state = PASSED;
        rings_passed++;
        if (combo < 5) combo++;
        score += 100 * combo;
        speed = fmin(MAX_SPEED, speed + 3.5);
        static char buf[24];
        number_text(buf, "RING x", combo, 0);
        show(buf, rgb(255, 240, 120));
        note_t notes[] = { { (uint16_t)(587 + combo * 98), 50 }, { (uint16_t)(784 + combo * 131), 50 },
                           { (uint16_t)(1175 + combo * 196), 90 } };
        sound(notes, 3);
    } else {
        static const note_t notes[] = { { 180, 160 } };
        o->state = MISSED;
        combo = 0;
        show("MISSED", rgb(255, 140, 110));
        sound(notes, 1);
    }
}

// The attract mode's glider aims for the next ring.
static void autopilot(void) {
    obj_t* next = 0;
    for (int i = 0; i < MAX_OBJ; i++) {
        obj_t* o = &objs[i];
        if (o->alive && o->kind == RING && o->z > gz + 2 && (!next || o->z < next->z)) next = o;
    }
    double tx = next ? next->x : canyon_x(gz + 30), ty = next ? next->y : 12, tz = next ? next->z : gz + 30;
    double want_yaw = atan2(tx - gx, tz - gz);
    want_roll = clamp((want_yaw - yaw) * 2.2, -0.8, 0.8);
    want_gamma = clamp(atan2(ty - gy, tz - gz) * 1.2, -0.5, 0.35);
}

static void fly(double dt) {
    double prev_z = gz;

    stalled = speed < STALL;
    double tg = stalled ? -0.5 : want_gamma;
    gamma_ += (tg - gamma_) * clamp(dt * 2.5, 0, 1);
    roll += (want_roll - roll) * clamp(dt * 4, 0, 1);

    // A banked wing turns the glider, tighter the slower it goes.
    yaw += GRAVITY * tan(roll) / fmax(speed, 5) * dt;
    yaw = clamp(yaw, -1.25, 1.25);

    // Diving trades height for speed and climbing speed for height, while
    // drag (worse in a turn) slowly bleeds it away.
    double a = -GRAVITY * sin(gamma_) - 0.0025 * speed * speed - 0.1 - 0.5 * fabs(sin(roll));
    speed = clamp(speed + a * dt, 3, MAX_SPEED);

    double horiz = speed * cos(gamma_);
    gx += horiz * sin(yaw) * dt;
    gz += horiz * cos(yaw) * dt;
    gy += speed * sin(gamma_) * dt;

    lift = 0;
    for (int i = 0; i < MAX_OBJ; i++) {
        obj_t* o = &objs[i];
        if (!o->alive || o->kind != THERMAL) continue;
        double dx = gx - o->x, dz = gz - o->z, d = sqrt(dx * dx + dz * dz);
        if (d < o->size) lift = fmax(lift, 7.5 * (1 - d / o->size * 0.6));
    }
    gy += lift * dt;

    if (gy > CEILING) {
        gy = CEILING;
        if (gamma_ > 0) gamma_ *= 1 - clamp(dt * 4, 0, 1);
    }

    for (int i = 0; i < MAX_OBJ; i++) {
        obj_t* o = &objs[i];
        if (!o->alive) continue;
        if (o->z < gz - 16) { o->alive = 0; continue; }
        if (o->kind == RING) {
            if (o->state == PENDING && prev_z < o->z && gz >= o->z) pass_ring(o);
        } else if (o->kind == ARCH) {
            if (fabs(gz - o->z) < o->size / 2 + 0.4 && gy > o->y - 0.4 && gy < o->y + o->size + 0.4) crash();
        } else if (o->kind == SPIRE) {
            double dx = gx - o->x, dz = gz - o->z;
            if (dx * dx + dz * dz < 2.1 * 2.1 && gy < o->y + o->size) crash();
        }
        if (crash_t > 0) return;
    }
    if (gy < ground_height(gx, gz) + 0.35) crash();
}

static void update(double dt) {
    clock_t_ += dt;
    if (crash_t > 0) {
        crash_t += dt;
        // The attract mode just starts again.
        if (state == TITLE && crash_t > 1.5) reset();
    } else {
        if (state == TITLE) autopilot();
        if (ready_t > 0) {
            ready_t -= dt;
            if (ready_t <= 0) show("GO!", rgb(255, 240, 120));
        } else {
            fly(dt);
        }
        if (state == PLAYING && stalled && (int)(clock_t_ * 4) % 2 == 0 && tune_pos >= tune_len) {
            static const note_t notes[] = { { 90, 60 } };
            sound(notes, 1);
        }
    }
    while (built_z < gz + AHEAD) build_step();
    if (msg_t > 0) msg_t -= dt;
}

// ---- drawing ------------------------------------------------------------------

static void place_camera(double dt) {
    if (crash_t > 0) {
        // Stay put and watch the wreck.
        r3d_look_at(&r, cam_eye, r3d_v((float)gx, (float)gy, (float)gz));
        return;
    }
    cam_yaw += (yaw - cam_yaw) * clamp(dt * 3, 0, 1);
    double back = 6.0;
    r3d_vec eye;
    eye.x = (float)(gx - sin(cam_yaw) * back);
    eye.z = (float)(gz - cos(cam_yaw) * back);
    eye.y = (float)(gy + 1.5);
    eye.y = (float)fmax(eye.y, ground_height(eye.x, eye.z) + 1.2);
    cam_eye = eye;
    r3d_vec at = r3d_v((float)(gx + sin(cam_yaw) * 3), (float)(gy + 0.4), (float)(gz + cos(cam_yaw) * 3));
    r3d_look_at(&r, eye, at);
}

// The low sun, ahead of you down the canyon.
static void draw_sun(void) {
    r3d_vec dir = r3d_normalize(r.sun);
    r3d_vec far = r3d_add(r.eye, r3d_mul(dir, 400));
    int x, y;
    if (r3d_project(&r, far, &x, &y) <= 0) return;
    x /= 2;
    y /= 2;
    canvas_circle(&r.screen, x, y, 16, rgb(255, 200, 140));
    canvas_circle(&r.screen, x, y, 12, rgb(255, 230, 170));
    canvas_circle(&r.screen, x, y, 9, rgb(255, 248, 220));
}

static void draw_ring(const obj_t* o) {
    uint32_t color = rgb(250, 200, 40);
    if (o->state == PASSED) color = rgb(140, 255, 160);
    if (o->state == MISSED) color = rgb(130, 110, 110);
    int n = 12;
    float seg = (float)(2 * o->size * sin(M_PI / n)) + 0.08f;
    // Unlit, so they glow against the canyon.
    r.unlit = 1;
    r3d_push(&r);
    r3d_translate(&r, (float)o->x, (float)o->y, (float)o->z);
    if (o->state == PENDING) r3d_rotate_z(&r, (float)(clock_t_ * 0.8));
    for (int k = 0; k < n; k++) {
        r3d_push(&r);
        r3d_rotate_z(&r, (float)(k * 2 * M_PI / n));
        r3d_translate(&r, (float)o->size, 0, 0);
        r3d_box(&r, 0.32f, seg, 0.32f, k % 2 ? color : rgb(255, 250, 220));
        r3d_pop(&r);
    }
    r3d_pop(&r);
    r.unlit = 0;
}

static void draw_arch(const obj_t* o) {
    double half = canyon_half(o->z);
    float w = (float)(2 * (half + (o->y - floor_y(o->z) + o->size) / 2.6 + 4));
    r3d_push(&r);
    r3d_translate(&r, (float)o->x, (float)(o->y + o->size / 2), (float)o->z);
    r3d_box(&r, w, (float)o->size, (float)o->size, rgb(176, 78, 58));
    r3d_translate(&r, 0, (float)(o->size / 2 + 0.25), 0);
    r3d_box(&r, w, 0.5f, (float)o->size * 0.8f, rgb(214, 140, 90));  // a paler cap of rock
    r3d_pop(&r);
}

static void draw_spire(const obj_t* o) {
    r3d_push(&r);
    r3d_translate(&r, (float)o->x, (float)o->y - 1, (float)o->z);
    r3d_rotate_y(&r, (float)o->angle);
    float h = (float)o->size + 1;
    r3d_cylinder(&r, 2.1f, 1.4f, h * 0.55f, 6, rgb(190, 92, 60));
    r3d_translate(&r, 0, h * 0.55f, 0);
    r3d_cylinder(&r, 1.4f, 1.1f, h * 0.45f, 6, rgb(212, 128, 80));
    r3d_translate(&r, 0, h * 0.45f, 0);
    r3d_scale(&r, 1, 0.45f, 1);
    r3d_sphere(&r, 1.9f, 3, rgb(150, 70, 52));  // a hoodoo's cap stone
    r3d_pop(&r);
}

// Dust spiralling up a thermal.
static void draw_thermal(const obj_t* o) {
    double top = CEILING + 4 - o->y;
    for (int k = 0; k < 18; k++) {
        double a = k * 2.4 + clock_t_ * (1.2 + (k % 3) * 0.3);
        double rad = o->size * (0.25 + 0.7 * ((k * 37 % 10) / 10.0));
        double y = o->y + fmod(clock_t_ * 5 + k * 3.7, top);
        r3d_vec p = r3d_v((float)(o->x + cos(a) * rad), (float)y, (float)(o->z + sin(a) * rad));
        r3d_line(&r, p, r3d_add(p, r3d_v(0, 1.1f, 0)), k % 2 ? rgb(255, 236, 200) : rgb(250, 210, 160));
    }
}

static void draw_glider(void) {
    // The shadow, flat on the rock: it shows how high you are.
    double cy = cos(yaw), sy = sin(yaw);
    static const double corners[3][2] = { { 0, 1.1 }, { 1.0, -0.6 }, { -1.0, -0.6 } };
    r3d_vec shadow[3];
    for (int k = 0; k < 3; k++) {
        double lx = corners[k][0], lz = corners[k][1];
        float x = (float)(gx + lx * cy + lz * sy), z = (float)(gz - lx * sy + lz * cy);
        shadow[k] = r3d_v(x, (float)ground_height(x, z) + 0.08f, z);
    }
    r.unlit = 1;
    r3d_polygon(&r, shadow, 3, rgb(150, 80, 70));
    r.unlit = 0;

    double tumble = crash_t > 0 ? fmin(crash_t * 6, 1.2) : 0;
    r3d_push(&r);
    r3d_translate(&r, (float)gx, (float)gy, (float)gz);
    r3d_rotate_y(&r, (float)yaw);
    r3d_rotate_x(&r, (float)(gamma_ - tumble));
    r3d_rotate_z(&r, (float)(-roll + tumble * 0.7));
    r3d_vec nose = r3d_v(0, 0, 1.2f), tail = r3d_v(0, 0.02f, -0.7f);
    r3d_vec left = r3d_v(-1.05f, 0.12f, -0.7f), right = r3d_v(1.05f, 0.12f, -0.7f);
    r3d_vec keel = r3d_v(0, -0.28f, -0.6f);
    r3d_triangle(&r, nose, left, tail, rgb(250, 248, 240));
    r3d_triangle(&r, nose, tail, right, rgb(236, 234, 226));
    r3d_triangle(&r, nose, tail, keel, rgb(70, 140, 220));
    // Stripes down the wings.
    r3d_line(&r, r3d_v(-0.5f, 0.07f, -0.7f), r3d_v(-0.1f, 0.02f, 0.8f), rgb(70, 140, 220));
    r3d_line(&r, r3d_v(0.5f, 0.07f, -0.7f), r3d_v(0.1f, 0.02f, 0.8f), rgb(70, 140, 220));
    r3d_pop(&r);
}

static void draw_world(void) {
    r3d_begin(&r);
    draw_sun();

    int i0 = (int)floor(r.eye.x / CELL) - COLS, j0 = (int)floor((r.eye.z - 8) / CELL);
    r3d_heightfield(&r, i0 * CELL, j0 * CELL, 2 * COLS, ROWS, CELL, canyon_height, rock_color);

    for (int k = 0; k < MAX_OBJ; k++) {
        obj_t* o = &objs[k];
        if (!o->alive) continue;
        if (o->kind == ARCH) {
            if (!r3d_visible(&r, r3d_v((float)o->x, (float)o->y, (float)o->z), 30)) continue;
            draw_arch(o);
            continue;
        }
        float rad = o->kind == SPIRE ? (float)o->size : (float)o->size + 1;
        float mid = o->kind == RING ? 0 : rad / 2;
        if (o->kind == THERMAL) { rad = (float)CEILING; mid = rad / 2; }
        if (!r3d_visible(&r, r3d_v((float)o->x, (float)o->y + mid, (float)o->z), rad)) continue;
        if (o->kind == RING) draw_ring(o);
        else if (o->kind == SPIRE) draw_spire(o);
        else draw_thermal(o);
    }
    draw_glider();
}

static void draw_hud(void) {
    char buf[40];
    uint32_t gold = rgb(255, 240, 120), white = rgb(255, 255, 255);

    text(16, 12, "SCORE", rgb(255, 190, 150), 2);
    number_text(buf, "", total_score(), 6);
    text(16, 32, buf, white, 3);
    if (combo > 1) {
        number_text(buf, "COMBO x", combo, 0);
        text(16, 60, buf, gold, 2);
    }

    text(W - 16 - canvas_text_width("HI", 2) - 96, 12, "HI", rgb(255, 150, 130), 2);
    number_text(buf, "", best > total_score() ? best : total_score(), 6);
    text(W - 16 - canvas_text_width(buf, 2), 12, buf, white, 2);

    // The altimeter: from the floor up to the ceiling.
    int ax = W - 30, top = 110, bottom = 360;
    canvas_rect(&c, ax, top, 10, bottom - top, rgb(60, 30, 50));
    canvas_rect_outline(&c, ax - 1, top - 1, 12, bottom - top + 2, rgb(255, 210, 170));
    double above = clamp((gy - floor_y(gz)) / CEILING, 0, 1);
    int my = bottom - (int)(above * (bottom - top));
    canvas_rect(&c, ax - 5, my - 2, 20, 5, lift > 0 ? rgb(140, 255, 160) : gold);
    text(ax - 4, bottom + 8, "ALT", rgb(255, 210, 170), 1);

    number_text(buf, "", (long)(speed * 3.6), 3);
    int bx = 16, by = H - 40;
    text(bx, by, buf, stalled ? rgb(255, 80, 60) : white, 3);
    text(bx + canvas_text_width(buf, 3) + 6, by + 8, "KM/H", rgb(255, 210, 170), 2);
    number_text(buf, "", (long)fmax(0, gz), 0);
    int dn = 0;
    while (buf[dn]) dn++;
    buf[dn] = 'M';
    buf[dn + 1] = '\0';
    text(W - 50 - canvas_text_width(buf, 3), by, buf, white, 3);

    if (stalled && crash_t == 0 && (int)(clock_t_ * 4) % 2 == 0) text_centered(130, "STALL - DIVE!", rgb(255, 80, 60), 3);
    else if (lift > 0 && crash_t == 0) text_centered(130, "THERMAL", rgb(140, 255, 160), 2);
}

static void draw(double dt) {
    place_camera(dt);
    draw_world();
    r3d_end(&r);

    char buf[40];
    if (state == TITLE) {
        text_centered(28, "UPDRAFT", rgb(255, 240, 120), 8);
        text_centered(98, "A CANYON GLIDER", rgb(255, 200, 170), 2);
        number_text(buf, "HI SCORE ", best, 6);
        text_centered(122, buf, rgb(255, 150, 130), 2);
        text_centered(326, "MOUSE OR ARROWS TO FLY", rgb(255, 255, 255), 2);
        text_centered(348, "DIVE FOR SPEED, CLIMB FOR HEIGHT", rgb(255, 255, 255), 2);
        text_centered(370, "FLY THROUGH THE RINGS", rgb(255, 255, 255), 2);
        if ((int)(uptime_ms() / 500) % 2) text_centered(416, "PRESS SPACE TO START", rgb(255, 255, 255), 3);
        canvas_present(&c);
        return;
    }
    draw_hud();
    if (ready_t > 0) text_centered(190, "READY", rgb(255, 240, 120), 6);
    else if (msg_t > 0 && state == PLAYING) text_centered(190, msg, msg_color, 4);
    if (state == OVER && over_t > 0.8) {
        int bx = 120, by = 150, bw = W - 240, bh = 190;
        canvas_rect(&c, bx, by, bw, bh, rgb(60, 24, 44));
        canvas_rect_outline(&c, bx, by, bw, bh, rgb(255, 240, 120));
        canvas_rect_outline(&c, bx + 3, by + 3, bw - 6, bh - 6, rgb(255, 240, 120));
        text_centered(by + 18, "CRASHED", rgb(255, 240, 120), 4);
        number_text(buf, "SCORE ", total_score(), 6);
        text_centered(by + 66, buf, rgb(255, 255, 255), 3);
        number_text(buf, "RINGS ", rings_passed, 0);
        text_centered(by + 100, buf, rgb(255, 200, 170), 2);
        if (new_best && (int)(uptime_ms() / 300) % 2) text_centered(by + 126, "NEW HI SCORE!", rgb(255, 150, 130), 2);
        if (over_t > 1.8) text_centered(by + 156, "SPACE TO FLY AGAIN", rgb(255, 220, 200), 2);
    }
    canvas_present(&c);
}

// ---- input --------------------------------------------------------------------

static void quit(void) {
    snd(0);
    canvas_close(&c);
    writeln("updraft: happy landings");
    exit();
}

static void start(void) {
    static const note_t notes[] = { { 440, 120 }, { 0, 380 }, { 440, 120 }, { 0, 380 }, { 880, 400 } };
    reset();
    state = PLAYING;
    ready_t = 1.0;
    show("", 0);
    sound(notes, 5);
}

static void press_action(void) {
    if (state == TITLE || (state == OVER && over_t > 1.8)) start();
}

void _start(void) {
    if (canvas_open(&c, "Updraft", W, H) != 0 || r3d_init(&r, &c, 2) != 0) {
        writeln("updraft: couldn't open a window");
        exit();
    }
    // Dusk: a low sun down the canyon, warm light, purple shadows.
    r.sun = r3d_v(0.35f, 0.32f, 1.0f);
    r.sun_light = r3d_v(0.55f, 0.42f, 0.3f);
    r.ambient = r3d_v(0.5f, 0.42f, 0.55f);
    r.sky_top = rgb(70, 50, 120);
    r.sky_horizon = r.ground = r.fog_color = rgb(DUSK_R, DUSK_G, DUSK_B);
    r.fog_near = FOG_NEAR;
    r.fog_far = FOG_FAR;
    r.near = 0.25f;
    writeln("updraft: mouse or arrows to fly; escape quits");
    best = read_best();
    seed ^= uptime_ms() * 0x2545f4914f6cdd1dull;
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
                if (state == PLAYING) {
                    if (k == CANVAS_KEY_LEFT || k == 'a') want_roll = clamp(want_roll - 0.25, -0.9, 0.9);
                    if (k == CANVAS_KEY_RIGHT || k == 'd') want_roll = clamp(want_roll + 0.25, -0.9, 0.9);
                    if (k == CANVAS_KEY_UP || k == 'w') want_gamma = clamp(want_gamma + 0.12, -0.6, 0.45);
                    if (k == CANVAS_KEY_DOWN || k == 's') want_gamma = clamp(want_gamma - 0.12, -0.6, 0.45);
                }
            }
            if (e.kind == CANVAS_MOUSE_DOWN && e.buttons == CANVAS_LEFT) press_action();
            if (e.kind == CANVAS_MOUSE_MOVE && state == PLAYING) {
                want_roll = clamp((e.x - W / 2.0) / (W / 2.0) * 1.1, -0.9, 0.9);
                // The mouse above the middle points the nose up.
                want_gamma = clamp(-0.08 - (e.y - H / 2.0) / (H / 2.0) * 0.55, -0.6, 0.45);
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
