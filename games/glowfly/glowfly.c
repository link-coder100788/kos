// Glowfly: guide a firefly through a forest at night.
//
// Space, Up or a click makes it flutter upwards; gravity does the rest.
// Vines hang from the canopy and thorns grow from the ground, and the gaps
// between them sway. Your glow fades as you fly, and the forest beyond it
// is dark, so catch the drifting spores to keep your light up. Hitting a
// plant or letting your light go out ends the flight.
//
// Escape quits. The best score is kept in /glowfly.hi.
#include "sys.h"
#include "canvas.h"
#include "math.h"
#include "print/print.h"

#define W 480
#define H 360
#define GROUND 24          // height of the forest floor
#define FLY_X 120          // where the firefly stays on screen
#define FLY_R 6

#define GRAVITY 0.32
#define FLUTTER -5.2
#define MAX_FALL 7.0

#define PLANT_W 34
#define PLANT_GAP 170      // distance between one plant pair and the next
#define MAX_PLANTS 6
#define MAX_SPORES 6
#define MAX_STARS 60

#define LIGHT_DRAIN 0.0011 // per frame: about 18 s from full to dark
#define SPORE_LIGHT 0.30

#define HISCORE_FILE "/glowfly.hi"

typedef struct {
    double x;
    double center;         // middle of the gap, before swaying
    double gap;
    double sway, phase;    // how far and where in its sway the gap is
    int passed;
} plant_t;

typedef struct {
    double x, y, phase;
    int alive;
} spore_t;

typedef struct { int x, y; uint32_t color; } star_t;

enum { TITLE, PLAYING, OVER };

static canvas_t c;
static plant_t plants[MAX_PLANTS];
static spore_t spores[MAX_SPORES];
static star_t stars[MAX_STARS];

static int state = TITLE;
static double fly_y, fly_v, light, speed, scroll;
static long frame, score, best;
static long over_at;       // the frame the last flight ended on
static const char* why_over = "";
static uint64_t seed = 0x9e3779b97f4a7c15ull;

// ---- small helpers ------------------------------------------------------------

static uint32_t rand32(void) {
    seed ^= seed << 13;
    seed ^= seed >> 7;
    seed ^= seed << 17;
    return (uint32_t)(seed >> 16);
}

// A random number in lo..hi.
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

static void write_best(long v) {
    char buf[24];
    int n = 0;
    char digits[24];
    do { digits[n++] = '0' + v % 10; v /= 10; } while (v > 0);
    for (int i = 0; i < n; i++) buf[i] = digits[n - 1 - i];
    fs_write(HISCORE_FILE, buf, n);
}

static void number_text(char* out, const char* label, long v) {
    char digits[24];
    int n = 0;
    while (*label) *out++ = *label++;
    do { digits[n++] = '0' + v % 10; v /= 10; } while (v > 0);
    while (n > 0) *out++ = digits[--n];
    *out = '\0';
}

static void text_centered(int y, const char* s, uint32_t color, int scale) {
    canvas_text(&c, (W - canvas_text_width(s, scale)) / 2, y, s, color, scale);
}

// ---- the world ----------------------------------------------------------------

// Where the gap of plant p is right now.
static double gap_center(const plant_t* p) {
    return p->center + p->sway * sin(p->phase + frame * 0.035);
}

static void place_plant(plant_t* p, double x) {
    // It gets harder as the score climbs: narrower gaps that sway more.
    double hard = clamp(score / 30.0, 0, 1);
    p->x = x;
    p->gap = 128 - 30 * hard;
    p->sway = rand_range(0, 12 + 34 * hard);
    p->phase = rand_range(0, 6.28);
    double margin = p->gap / 2 + p->sway + 20;
    p->center = rand_range(margin, H - GROUND - margin);
    p->passed = 0;

    // Most gaps get a spore to aim for.
    if (rand32() % 10 < 7) {
        for (int i = 0; i < MAX_SPORES; i++) {
            if (!spores[i].alive) {
                spores[i] = (spore_t){ x + PLANT_W / 2.0, p->center, rand_range(0, 6.28), 1 };
                break;
            }
        }
    }
}

static void reset(void) {
    fly_y = H / 2.0;
    fly_v = 0;
    light = 1.0;
    speed = 2.2;
    scroll = 0;
    score = 0;
    for (int i = 0; i < MAX_SPORES; i++) spores[i].alive = 0;
    for (int i = 0; i < MAX_PLANTS; i++) place_plant(&plants[i], W + 60 + i * PLANT_GAP);
}

static void make_stars(void) {
    for (int i = 0; i < MAX_STARS; i++) {
        uint8_t b = 120 + rand32() % 120;
        stars[i] = (star_t){ rand32() % W, rand32() % (H / 2), rgb(b, b, b + 15 > 255 ? 255 : b + 15) };
    }
}

static void game_over(const char* why) {
    snd(440);
    sleep_ms(250);
    snd(330);
    sleep_ms(250);
    snd(220);
    sleep_ms(500);
    snd(0);
    state = OVER;
    over_at = frame;
    why_over = why;
    if (score > best) {
        best = score;
        write_best(best);
    }
}

static int hits_rect(double cx, double cy, double r, double x, double y, double w, double h) {
    double nx = clamp(cx, x, x + w), ny = clamp(cy, y, y + h);
    double dx = cx - nx, dy = cy - ny;
    return dx * dx + dy * dy < r * r;
}

static void update(void) {
    fly_v = clamp(fly_v + GRAVITY, FLUTTER, MAX_FALL);
    fly_y += fly_v;
    scroll += speed;
    speed = 2.2 + clamp(score / 40.0, 0, 1) * 1.2;
    light -= LIGHT_DRAIN;

    if (fly_y < FLY_R) { fly_y = FLY_R; fly_v = 0; }
    if (fly_y > H - GROUND - FLY_R) { game_over("You landed in the leaves"); return; }
    if (light <= 0) { light = 0; game_over("Your light went out"); return; }

    for (int i = 0; i < MAX_PLANTS; i++) {
        plant_t* p = &plants[i];
        p->x -= speed;
        if (p->x + PLANT_W < 0) {
            // Recycle it behind the furthest plant.
            double far = 0;
            for (int j = 0; j < MAX_PLANTS; j++) if (plants[j].x > far) far = plants[j].x;
            place_plant(p, far + PLANT_GAP);
            continue;
        }
        double g = gap_center(p);
        double top = g - p->gap / 2, bottom = g + p->gap / 2;
        if (hits_rect(FLY_X, fly_y, FLY_R - 1, p->x, 0, PLANT_W, top)
            || hits_rect(FLY_X, fly_y, FLY_R - 1, p->x, bottom, PLANT_W, H - GROUND - bottom)) {
            game_over("Caught in the plants");
            return;
        }
        if (!p->passed && p->x + PLANT_W < FLY_X) {
            p->passed = 1;
            score++;
        }
    }

    for (int i = 0; i < MAX_SPORES; i++) {
        spore_t* s = &spores[i];
        if (!s->alive) continue;
        s->x -= speed;
        // Spores ride the gap they were born in.
        for (int j = 0; j < MAX_PLANTS; j++) {
            plant_t* p = &plants[j];
            if (s->x >= p->x - 1 && s->x <= p->x + PLANT_W + 1) s->y = gap_center(p);
        }
        double dx = s->x - FLY_X, dy = s->y - fly_y;
        if (dx * dx + dy * dy < (FLY_R + 8) * (FLY_R + 8)) {
            s->alive = 0;
            light = clamp(light + SPORE_LIGHT, 0, 1);
            snd(880);
            sleep_ms(25);
            snd(0);
            score++;
        }
        if (s->x < -10) s->alive = 0;
    }
}

// ---- drawing ------------------------------------------------------------------

static void draw_sky(void) {
    for (int y = 0; y < H - GROUND; y++) {
        int t = y * 255 / (H - GROUND);
        canvas_rect(&c, 0, y, W, 1, rgb(8 + t / 12, 10 + t / 10, 30 + t / 6));
    }
    for (int i = 0; i < MAX_STARS; i++) {
        // Stars drift slowly, far away.
        int x = ((stars[i].x - (int)(scroll * 0.05)) % W + W) % W;
        if ((frame + i * 7) % 90 < 84) canvas_pixel(&c, x, stars[i].y, stars[i].color);
    }
    canvas_circle(&c, W - 70, 56, 22, rgb(230, 228, 200));
    canvas_circle(&c, W - 61, 50, 20, rgb(14, 18, 44));  // a crescent

    // Two layers of tree tops, the nearer one moving faster.
    for (int layer = 0; layer < 2; layer++) {
        double rate = layer ? 0.45 : 0.2;
        uint32_t color = layer ? rgb(14, 30, 26) : rgb(18, 26, 40);
        int base = H - GROUND - (layer ? 40 : 80);
        int offset = (int)(scroll * rate);
        for (int x = 0; x < W; x += 2) {
            int wx = x + offset;
            double h = 18 * sin(wx * 0.021 + layer) + 10 * sin(wx * 0.057 + 2 * layer);
            canvas_rect(&c, x, base - (int)h, 2, H - GROUND - base + (int)h, color);
        }
    }
}

static void draw_plant(const plant_t* p) {
    int x = (int)p->x;
    double g = gap_center(p);
    int top = (int)(g - p->gap / 2), bottom = (int)(g + p->gap / 2);
    uint32_t vine = rgb(40, 120, 60), leaf = rgb(70, 170, 80);
    uint32_t stem = rgb(70, 50, 40), thorn = rgb(170, 120, 90);

    // A hanging vine: twisted strands with leaves.
    for (int i = 0; i < 3; i++) {
        int sx = x + 6 + i * 11;
        canvas_rect(&c, sx, 0, 6, top, vine);
        for (int y = (i * 13) % 20; y < top - 6; y += 20) {
            canvas_circle(&c, sx + (y / 20 % 2 ? 9 : -3), y, 4, leaf);
        }
    }
    canvas_circle(&c, x + PLANT_W / 2, top, PLANT_W / 2 - 2, vine);

    // A thorny stalk: a thick stem with spikes.
    canvas_rect(&c, x + 4, bottom, PLANT_W - 8, H - GROUND - bottom, stem);
    for (int y = bottom + 8; y < H - GROUND; y += 14) {
        canvas_line(&c, x + 4, y, x - 4, y - 5, thorn);
        canvas_line(&c, x + PLANT_W - 5, y + 7, x + PLANT_W + 3, y + 2, thorn);
    }
    for (int i = 0; i < 4; i++) {
        canvas_line(&c, x + 4 + i * 8, bottom, x + 8 + i * 8, bottom - 9, thorn);
    }
}

static void draw_ground(void) {
    canvas_rect(&c, 0, H - GROUND, W, GROUND, rgb(20, 40, 24));
    int offset = (int)scroll % 12;
    for (int x = -offset; x < W; x += 12) {
        canvas_line(&c, x, H - GROUND, x + 4, H - GROUND - 6, rgb(40, 80, 44));
    }
}

// Everything beyond the firefly's glow fades into the night.
static void darken(double radius) {
    int r1 = (int)(radius * radius);
    int r2 = (int)(radius * radius * 2.6);
    for (int y = 0; y < H; y++) {
        int dy = y - (int)fly_y;
        uint32_t* row = c.pixels + y * W;
        for (int x = 0; x < W; x++) {
            int dx = x - FLY_X;
            int d = dx * dx + dy * dy;
            if (d < r1) continue;
            row[x] = d < r2 ? (row[x] >> 1) & 0x7f7f7f : (row[x] >> 2) & 0x3f3f3f;
        }
    }
}

static void draw_spores(void) {
    for (int i = 0; i < MAX_SPORES; i++) {
        spore_t* s = &spores[i];
        if (!s->alive) continue;
        int pulse = (int)(2 * sin(frame * 0.15 + s->phase));
        canvas_circle(&c, (int)s->x, (int)s->y, 7 + pulse, rgb(40, 90, 110));
        canvas_circle(&c, (int)s->x, (int)s->y, 4, rgb(150, 240, 255));
    }
}

static void draw_firefly(void) {
    int x = FLY_X, y = (int)fly_y;
    // Wings beat faster while climbing.
    int beat = (frame / (fly_v < 0 ? 2 : 5)) % 2;
    uint32_t wing = rgb(170, 190, 220);
    canvas_circle(&c, x - 1, y - 6 - beat * 2, 4, wing);
    canvas_circle(&c, x + 4, y - 5 - beat * 2, 3, wing);
    int glow = (int)(150 + 105 * light);
    canvas_circle(&c, x - 4, y + 1, 6, rgb(glow, glow, 60));
    canvas_circle(&c, x - 4, y + 1, 3, rgb(255, 255, 200));
    canvas_circle(&c, x + 4, y, 4, rgb(60, 50, 40));
    canvas_pixel(&c, x + 6, y - 1, rgb(255, 255, 255));
}

static void draw_hud(void) {
    char buf[40];
    number_text(buf, "", score);
    canvas_text(&c, 12, 10, buf, rgb(255, 255, 255), 3);
    // The light meter.
    int mw = 120;
    canvas_rect_outline(&c, W - mw - 14, 12, mw + 4, 14, rgb(200, 200, 160));
    uint32_t fill = light > 0.25 ? rgb(240, 230, 90) : (frame / 8 % 2 ? rgb(240, 90, 60) : rgb(120, 40, 30));
    canvas_rect(&c, W - mw - 12, 14, (int)(mw * light), 10, fill);
}

static void draw(void) {
    draw_sky();
    for (int i = 0; i < MAX_PLANTS; i++) draw_plant(&plants[i]);
    draw_ground();
    if (state != TITLE) darken(50 + 120 * light);
    draw_spores();
    draw_firefly();

    char buf[40];
    if (state == TITLE) {
        text_centered(56, "GLOWFLY", rgb(250, 240, 120), 5);
        text_centered(112, "Space, Up or click to flutter", rgb(220, 220, 230), 2);
        text_centered(138, "Catch spores to stay lit", rgb(150, 230, 250), 2);
        if (best > 0) {
            number_text(buf, "Best: ", best);
            text_centered(170, buf, rgb(220, 220, 230), 2);
        }
        if (frame / 25 % 2) text_centered(270, "Press space to fly", rgb(255, 255, 255), 2);
    } else {
        draw_hud();
    }
    if (state == OVER) {
        canvas_rect(&c, 60, 100, W - 120, 150, rgb(10, 12, 28));
        canvas_rect_outline(&c, 60, 100, W - 120, 150, rgb(250, 240, 120));
        text_centered(116, why_over, rgb(250, 240, 120), 2);
        number_text(buf, "Score: ", score);
        text_centered(150, buf, rgb(255, 255, 255), 3);
        number_text(buf, "Best: ", best);
        text_centered(186, buf, rgb(200, 200, 220), 2);
        text_centered(220, "Space to fly again", rgb(200, 200, 220), 2);
    }
    canvas_present(&c);
}

// ---- input --------------------------------------------------------------------

static void quit(void) {
    canvas_close(&c);
    writeln("glowfly: goodnight");
    exit();
}

static void press(void) {
    if (state == PLAYING) {
        snd(660);
        sleep_ms(20);
        snd(0);
        fly_v = FLUTTER;
    } else if (state == TITLE || frame - over_at > 20) {
        // The frame check stops a flutter held through the crash from
        // skipping the score screen.
        reset();
        state = PLAYING;
        fly_v = FLUTTER;
    }
}

void _start(void) {
    if (canvas_open(&c, "Glowfly", W, H) != 0) {
        writeln("glowfly: couldn't open a window");
        exit();
    }
    writeln("glowfly: space, up or click to flutter; escape quits");
    best = read_best();
    make_stars();
    reset();

    for (;;) {
        canvas_event_t e;
        while (canvas_poll(&c, &e) > 0) {
            if (e.kind == CANVAS_CLOSED) quit();
            if (e.kind == CANVAS_KEY) {
                if (e.key == 27) quit();
                if (e.key == ' ' || e.key == CANVAS_KEY_UP || e.key == 'w') press();
            }
            if (e.kind == CANVAS_MOUSE_DOWN && e.buttons == CANVAS_LEFT) press();
        }
        if (state == PLAYING) {
            update();
        } else if (state == TITLE) {
            // Waiting on the player is the only randomness there is.
            rand32();
            fly_y = 225 + 10 * sin(frame * 0.08);
            scroll += 1;
        }
        frame++;
        draw();
        sleep_ms(20);
    }
}
