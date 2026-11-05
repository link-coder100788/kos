// Sandbox: a falling-sand toy for KOS. Paint sand, water, oil, lava, fire
// and more with the mouse, and watch them pour, pile up, float, burn and
// boil off.
//
// The world is a grid of 4x4-pixel cells, each one thing. Every step, each
// cell moves or reacts by a few rules: sand falls and piles; water and oil
// fall and spread, oil floating on water; fire burns wood, plants and oil
// and goes out in water; lava sets things alight and turns water to steam
// (and itself to stone); plants grow into water; smoke and steam rise and
// fade, and steam rains back down as water.
//
// Left-drag paints, right-drag erases; the toolbar or 1-9, 0 and - pick
// what to paint, e the eraser. The wheel or [ and ] size the brush. Space
// pauses, c clears, s saves the scene to /games/sandbox.sav and l loads it.
// Escape quits.
#include "sys.h"
#include "canvas.h"

#define W 640
#define H 480
#define CELL 4
#define GW (W / CELL)          // the grid, in cells
#define GH 108
#define BAR_Y (GH * CELL)      // the toolbar, below the grid
#define SAVE_FILE "/games/sandbox.sav"

enum { EMPTY, SAND, WATER, STONE, WOOD, PLANT, FIRE, OIL, LAVA, SMOKE, STEAM, ICE, N_KINDS };

typedef struct {
    uint8_t kind;
    uint8_t life;   // fire, smoke and steam: steps left
    uint8_t shade;  // a little random variation in its color
    uint8_t flags;
} cell_t;

#define F_STEP  1   // the parity of the last step this cell acted in
#define F_FIXED 2   // fire burning on something that doesn't move (wood, plant)

static canvas_t c;
static cell_t grid[GH][GW];
static unsigned step_parity;
static uint64_t seed = 0x2545f4914f6cdd1dull;

static int tool = SAND;     // what the brush paints (EMPTY is the eraser)
static int brush = 3;       // the brush's radius, in cells
static int paused;
static int mouse_x = -100, mouse_y = -100, painting, last_px, last_py;
static const char* msg = "";
static uint64_t msg_until;

// ---- small helpers ------------------------------------------------------------

static uint32_t rand32(void) {
    seed ^= seed << 13;
    seed ^= seed >> 7;
    seed ^= seed << 17;
    return (uint32_t)(seed >> 16);
}

// 1 in n.
static int chance(unsigned n) {
    return rand32() % n == 0;
}

static void show(const char* text) {
    msg = text;
    msg_until = uptime_ms() + 1500;
}

static int in_grid(int x, int y) {
    return x >= 0 && x < GW && y >= 0 && y < GH;
}

static int is_liquid(int k) {
    return k == WATER || k == OIL || k == LAVA;
}

// How heavy each thing is: a falling thing sinks through lighter liquids
// and gases (and nothing, the lightest).
static int density(int k) {
    switch (k) {
    case EMPTY: case SMOKE: case STEAM: return 0;
    case OIL: return 2;
    case WATER: return 3;
    case LAVA: return 4;
    case SAND: return 5;
    default: return 100;  // solids, and fire: nothing pushes them aside
    }
}

// How readily fire catches it, 1 in this many each step (0: it doesn't).
static unsigned flammable(int k) {
    switch (k) {
    case OIL: return 2;
    case PLANT: return 6;
    case WOOD: return 30;
    default: return 0;
    }
}

static cell_t make(int kind) {
    cell_t n = { (uint8_t)kind, 0, (uint8_t)(rand32() % 32), (uint8_t)(step_parity & F_STEP) };
    if (kind == FIRE) n.life = (uint8_t)(8 + rand32() % 14);
    if (kind == SMOKE) n.life = (uint8_t)(60 + rand32() % 80);
    if (kind == STEAM) n.life = (uint8_t)(90 + rand32() % 90);
    return n;
}

static void set(int x, int y, int kind) {
    grid[y][x] = make(kind);
}

// Sets the thing at (x, y) alight: flames that stay where it is, burning as
// long as it would.
static void ignite(int x, int y) {
    int k = grid[y][x].kind;
    set(x, y, FIRE);
    grid[y][x].flags |= F_FIXED;
    if (k == WOOD) grid[y][x].life = (uint8_t)(90 + rand32() % 90);
    else if (k == PLANT) grid[y][x].life = (uint8_t)(20 + rand32() % 25);
    else grid[y][x].life = (uint8_t)(12 + rand32() % 18);
}

static void swap(int x0, int y0, int x1, int y1) {
    cell_t t = grid[y0][x0];
    grid[y0][x0] = grid[y1][x1];
    grid[y1][x1] = t;
}

// Moves the thing at (x, y) to (nx, ny) if what's there is lighter.
static int fall_to(int x, int y, int nx, int ny) {
    if (!in_grid(nx, ny)) return 0;
    if (density(grid[ny][nx].kind) >= density(grid[y][x].kind)) return 0;
    swap(x, y, nx, ny);
    return 1;
}

// ---- the rules ------------------------------------------------------------------

static void powder(int x, int y) {
    if (fall_to(x, y, x, y + 1)) return;
    int d = rand32() & 1 ? 1 : -1;
    if (fall_to(x, y, x + d, y + 1)) return;
    fall_to(x, y, x - d, y + 1);
}

// Falls, or else flows up to `spread` cells sideways.
static void liquid(int x, int y, int spread) {
    if (fall_to(x, y, x, y + 1)) return;
    int d = rand32() & 1 ? 1 : -1;
    if (fall_to(x, y, x + d, y + 1)) return;
    if (fall_to(x, y, x - d, y + 1)) return;
    for (int side = 0; side < 2; side++, d = -d) {
        int to = 0;
        for (int i = 1; i <= spread; i++) {
            if (!in_grid(x + d * i, y) || density(grid[y][x + d * i].kind) >= density(grid[y][x].kind)) break;
            to = i;
        }
        if (to) { swap(x, y, x + d * to, y); return; }
    }
}

// Rises and drifts, through anything lighter than nothing or a liquid.
static void gas(int x, int y) {
    int d = (int)(rand32() % 3) - 1;
    int nx = x + d, ny = y - 1;
    if (in_grid(nx, ny)) {
        int k = grid[ny][nx].kind;
        if (k == EMPTY || (is_liquid(k) && k != LAVA && chance(3))) { swap(x, y, nx, ny); return; }
    }
    nx = x + (rand32() & 1 ? 1 : -1);
    if (in_grid(nx, y) && grid[y][nx].kind == EMPTY) swap(x, y, nx, y);
}

static void burn(int x, int y) {
    cell_t* f = &grid[y][x];
    // Spread to whatever burns around it.
    for (int dy = -1; dy <= 1; dy++) {
        for (int dx = -1; dx <= 1; dx++) {
            int nx = x + dx, ny = y + dy;
            if (!in_grid(nx, ny)) continue;
            int k = grid[ny][nx].kind;
            if (k == WATER) {
                // Water puts it out, boiling off.
                set(nx, ny, STEAM);
                set(x, y, chance(2) ? SMOKE : EMPTY);
                return;
            }
            if (k == ICE && chance(4)) set(nx, ny, WATER);
            unsigned n = flammable(k);
            if (n && chance(n)) ignite(nx, ny);
        }
    }
    if (f->life == 0 || --f->life == 0) {
        int fixed = f->flags & F_FIXED;
        set(x, y, chance(fixed ? 2 : 5) ? SMOKE : EMPTY);
        return;
    }
    if (f->flags & F_FIXED) {
        // Flames lick up off it.
        if (y > 0 && grid[y - 1][x].kind == EMPTY && chance(3)) set(x, y - 1, FIRE);
        return;
    }
    // A loose flame flickers upwards.
    int nx = x + (int)(rand32() % 3) - 1, ny = y - (chance(3) ? 0 : 1);
    if (in_grid(nx, ny) && grid[ny][nx].kind == EMPTY) swap(x, y, nx, ny);
}

static void lava(int x, int y) {
    for (int dy = -1; dy <= 1; dy++) {
        for (int dx = -1; dx <= 1; dx++) {
            int nx = x + dx, ny = y + dy;
            if (!in_grid(nx, ny)) continue;
            int k = grid[ny][nx].kind;
            if (k == WATER || k == ICE) {
                set(nx, ny, STEAM);
                set(x, y, STONE);
                return;
            }
            unsigned n = flammable(k);
            if (n && chance(n)) ignite(nx, ny);
            if (k == EMPTY && dy == -1 && chance(400)) set(nx, ny, SMOKE);
        }
    }
    // Thick: it only creeps sideways now and then.
    liquid(x, y, chance(4) ? 1 : 0);
}

static void plant(int x, int y) {
    if (!chance(12)) return;
    int nx = x + (int)(rand32() % 3) - 1, ny = y + (int)(rand32() % 3) - 1;
    if (in_grid(nx, ny) && grid[ny][nx].kind == WATER) set(nx, ny, PLANT);
}

// Ice freezes the water it touches, slowly, and melts by fire and lava.
static void ice(int x, int y) {
    if (!chance(40)) return;
    int nx = x + (int)(rand32() % 3) - 1, ny = y + (int)(rand32() % 3) - 1;
    if (in_grid(nx, ny) && grid[ny][nx].kind == WATER) set(nx, ny, ICE);
}

static void fade(int x, int y) {
    cell_t* g = &grid[y][x];
    if (g->life == 0 || --g->life == 0) {
        // Steam condenses back into rain.
        set(x, y, g->kind == STEAM && chance(3) ? WATER : EMPTY);
        return;
    }
    gas(x, y);
}

static void step(void) {
    step_parity ^= 1;
    // Bottom to top, so falling things don't move twice. Each cell carries
    // the parity of the step it last acted in, so one that moved up (or
    // sideways, ahead of the scan) waits for the next step.
    for (int y = GH - 1; y >= 0; y--) {
        int left_first = rand32() & 1;
        for (int i = 0; i < GW; i++) {
            int x = left_first ? i : GW - 1 - i;
            cell_t* g = &grid[y][x];
            if (g->kind == EMPTY || g->kind == STONE || g->kind == WOOD) continue;
            if ((g->flags & F_STEP) == step_parity) continue;
            g->flags = (uint8_t)((g->flags & ~F_STEP) | step_parity);
            switch (g->kind) {
            case SAND: powder(x, y); break;
            case WATER: liquid(x, y, 4); break;
            case OIL: liquid(x, y, 3); break;
            case LAVA: lava(x, y); break;
            case FIRE: burn(x, y); break;
            case PLANT: plant(x, y); break;
            case ICE: ice(x, y); break;
            case SMOKE: case STEAM: fade(x, y); break;
            }
        }
    }
}

// ---- painting -------------------------------------------------------------------

static void paint_at(int cx, int cy) {
    for (int dy = -brush + 1; dy < brush; dy++) {
        for (int dx = -brush + 1; dx < brush; dx++) {
            if (dx * dx + dy * dy > (brush - 1) * (brush - 1) + brush / 2) continue;
            int x = cx + dx, y = cy + dy;
            if (!in_grid(x, y)) continue;
            int k = grid[y][x].kind;
            if (tool == EMPTY) {
                grid[y][x].kind = EMPTY;
            } else if (tool == FIRE) {
                if (flammable(k)) ignite(x, y);
                else if (k == EMPTY && chance(3)) set(x, y, FIRE);
            } else if (tool == STONE || tool == WOOD || tool == ICE) {
                // Solids are laid over anything.
                if (k != tool) set(x, y, tool);
            } else if (k == EMPTY && chance(tool == PLANT ? 1 : 2)) {
                // Loose things are sprinkled, which makes a natural pile.
                set(x, y, tool);
            }
        }
    }
}

// Paints a stroke from the last point to this one, so a fast drag leaves
// no gaps.
static void paint_line(int x0, int y0, int x1, int y1) {
    int dx = x1 - x0, dy = y1 - y0;
    int n = (dx < 0 ? -dx : dx) > (dy < 0 ? -dy : dy) ? (dx < 0 ? -dx : dx) : (dy < 0 ? -dy : dy);
    for (int i = 0; i <= n; i++) {
        int x = n ? x0 + dx * i / n : x0, y = n ? y0 + dy * i / n : y0;
        paint_at(x, y);
    }
}

static void clear(void) {
    for (int y = 0; y < GH; y++)
        for (int x = 0; x < GW; x++) grid[y][x] = (cell_t){ 0 };
}

// A first scene: two stone ledges, a wooden shelf with a plant on it, and
// a pond to pour things into.
static void demo_scene(void) {
    clear();
    for (int x = 14; x < 64; x++) {
        set(x, 60 + (x - 14) / 6, STONE);
        set(x, 61 + (x - 14) / 6, STONE);
    }
    for (int x = 96; x < 146; x++) {
        set(x, 44, WOOD);
        set(x, 45, WOOD);
    }
    for (int x = 104; x < 140; x++)
        for (int y = 40; y < 44; y++)
            if (!chance(6)) set(x, y, PLANT);
    for (int y = 88; y < GH; y++) {
        for (int x = 70; x < 136; x++) {
            int edge = x < 74 || x >= 132 || y >= GH - 3;
            set(x, y, edge ? STONE : WATER);
        }
    }
    for (int y = 92; y < GH; y++)
        for (int x = 4; x < 40; x++)
            if (y > GH - 1 - (x - 4) / 2 - 2) set(x, y, SAND);
}

static void save(void) {
    if (fs_write(SAVE_FILE, grid, sizeof(grid)) == 0) show("SAVED");
    else show("SAVE ERR");
}

static void load(void) {
    if (fs_read(SAVE_FILE, grid, sizeof(grid)) == (long)sizeof(grid)) {
        show("LOADED");
        for (int y = 0; y < GH; y++)
            for (int x = 0; x < GW; x++)
                if (grid[y][x].kind >= N_KINDS) grid[y][x] = (cell_t){ 0 };
    } else {
        show("NO SAVE");
    }
}

// ---- drawing ------------------------------------------------------------------

#define BG 0x0e0e16

static uint32_t mix(uint32_t a, uint32_t b, int t) {  // t/255 of the way from a to b
    int ar = a >> 16 & 255, ag = a >> 8 & 255, ab = a & 255;
    int br = b >> 16 & 255, bg = b >> 8 & 255, bb = b & 255;
    return rgb((uint8_t)(ar + (br - ar) * t / 255), (uint8_t)(ag + (bg - ag) * t / 255), (uint8_t)(ab + (bb - ab) * t / 255));
}

// Lightens (or darkens) a color by s.
static uint32_t tint(uint32_t col, int s) {
    int r = (col >> 16 & 255) + s, g = (col >> 8 & 255) + s, b = (col & 255) + s;
    r = r < 0 ? 0 : r > 255 ? 255 : r;
    g = g < 0 ? 0 : g > 255 ? 255 : g;
    b = b < 0 ? 0 : b > 255 ? 255 : b;
    return rgb((uint8_t)r, (uint8_t)g, (uint8_t)b);
}

static const uint32_t base_color[N_KINDS] = {
    [EMPTY] = BG,         [SAND] = 0xdcbc78,  [WATER] = 0x2c64dc, [STONE] = 0x7c7c88,
    [WOOD] = 0x7a4e2c,    [PLANT] = 0x34aa40, [FIRE] = 0xff9a28,  [OIL] = 0x5a3c22,
    [LAVA] = 0xf05a14,    [SMOKE] = 0x56565e, [STEAM] = 0xc0cce0, [ICE] = 0xa8dcf0,
};

static const char* const names[N_KINDS] = {
    [EMPTY] = "ERASE", [SAND] = "SAND", [WATER] = "WATER", [STONE] = "STONE", [WOOD] = "WOOD",
    [PLANT] = "PLANT", [FIRE] = "FIRE", [OIL] = "OIL", [LAVA] = "LAVA", [SMOKE] = "SMOKE",
    [STEAM] = "STEAM", [ICE] = "ICE",
};

// The toolbar's buttons, in order: 1-9, 0 and - pick the first eleven.
static const int tools[] = { SAND, WATER, STONE, WOOD, PLANT, FIRE, OIL, LAVA, ICE, SMOKE, STEAM, EMPTY };
#define N_TOOLS ((int)(sizeof(tools) / sizeof(tools[0])))
#define BTN_W 44
#define BTN_GAP 3

static uint32_t cell_color(const cell_t* g, unsigned t) {
    int s = g->shade;
    switch (g->kind) {
    case EMPTY: return BG;
    case SAND: return tint(base_color[SAND], s - 16);
    case WATER: return tint(base_color[WATER], ((s + t / 4) % 32) / 2 - 8);  // a shimmer
    case STONE: return tint(base_color[STONE], s / 2 - 8);
    case WOOD: return tint(base_color[WOOD], (s % 8) * 3 - 10);
    case PLANT: return tint(base_color[PLANT], s / 2 - 8);
    case FIRE: {
        // Yellow-white when fresh, to deep red as it dies.
        int heat = g->life > 20 ? 255 : g->life * 12;
        return tint(mix(0xc81e0a, 0xffdc50, heat), (int)((s + t) % 5) * 6);
    }
    case OIL: return tint(base_color[OIL], s / 3 - 5);
    case LAVA: return tint(mix(0xc0280a, 0xff9628, (s * 8 + t * 3) % 256 < 128 ? 80 : 200), s / 3 - 5);
    case SMOKE: return mix(BG, tint(base_color[SMOKE], s / 2), g->life > 60 ? 255 : g->life * 4);
    case STEAM: return mix(BG, tint(base_color[STEAM], s / 2 - 8), g->life > 80 ? 230 : g->life * 3);
    case ICE: return tint(base_color[ICE], s / 2 - 8);
    }
    return BG;
}

static void draw(void) {
    static unsigned frame;
    frame++;
    uint32_t* px = c.pixels;
    for (int y = 0; y < GH; y++) {
        for (int x = 0; x < GW; x++) {
            uint32_t col = cell_color(&grid[y][x], frame);
            uint32_t* p = px + y * CELL * W + x * CELL;
            for (int j = 0; j < CELL; j++, p += W) p[0] = p[1] = p[2] = p[3] = col;
        }
    }

    // The brush, as an outline where the mouse is.
    if (mouse_y < BAR_Y && mouse_x >= 0) {
        int rad = (brush - 1) * CELL + CELL / 2;
        canvas_circle_outline(&c, mouse_x, mouse_y, rad < 2 ? 2 : rad, rgb(255, 255, 255));
    }

    // The toolbar.
    canvas_rect(&c, 0, BAR_Y, W, H - BAR_Y, rgb(28, 28, 40));
    canvas_rect(&c, 0, BAR_Y, W, 1, rgb(70, 70, 92));
    for (int i = 0; i < N_TOOLS; i++) {
        int k = tools[i];
        int bx = 4 + i * (BTN_W + BTN_GAP), by = BAR_Y + 5;
        uint32_t swatch = k == EMPTY ? rgb(40, 40, 52) : base_color[k];
        canvas_rect(&c, bx, by, BTN_W, 22, swatch);
        if (k == EMPTY) {
            canvas_line(&c, bx + 14, by + 4, bx + 32, by + 18, rgb(230, 80, 80));
            canvas_line(&c, bx + 32, by + 4, bx + 14, by + 18, rgb(230, 80, 80));
        }
        if (k == tool) {
            canvas_rect_outline(&c, bx - 2, by - 2, BTN_W + 4, 26, rgb(255, 255, 255));
            canvas_rect_outline(&c, bx - 1, by - 1, BTN_W + 2, 24, rgb(255, 255, 255));
        }
        canvas_text(&c, bx + (BTN_W - canvas_text_width(names[k], 1)) / 2, by + 28, names[k],
                    k == tool ? rgb(255, 255, 255) : rgb(170, 170, 190), 1);
    }
    int rx = 4 + N_TOOLS * (BTN_W + BTN_GAP) + 2;
    char size[] = "SIZE 0";
    size[5] = (char)('0' + brush);
    canvas_text(&c, rx, BAR_Y + 10, size, rgb(200, 200, 220), 1);
    const char* status = uptime_ms() < msg_until ? msg : paused ? "PAUSED" : "";
    canvas_text(&c, rx, BAR_Y + 26, status, rgb(255, 220, 120), 1);

    canvas_present(&c);
}

// ---- input --------------------------------------------------------------------

static void quit(void) {
    canvas_close(&c);
    writeln("sandbox: bye");
    exit();
}

// The tool under a click on the toolbar, or -1.
static int tool_at(int x, int y) {
    if (y < BAR_Y) return -1;
    int i = (x - 4) / (BTN_W + BTN_GAP);
    if (x < 4 || i >= N_TOOLS || (x - 4) % (BTN_W + BTN_GAP) >= BTN_W) return -1;
    return tools[i];
}

static void set_brush(int b) {
    brush = b < 1 ? 1 : b > 9 ? 9 : b;
}

static void key(int k) {
    static const char tool_keys[] = "1234567890-";
    if (k == 27) quit();
    for (int i = 0; tool_keys[i]; i++)
        if (k == tool_keys[i]) tool = tools[i];
    if (k == 'e') tool = EMPTY;
    if (k == '[') set_brush(brush - 1);
    if (k == ']') set_brush(brush + 1);
    if (k == ' ') paused = !paused;
    if (k == 'c') clear();
    if (k == 's' || k == CANVAS_CTRL('s')) save();
    if (k == 'l') load();
}

static void mouse(const canvas_event_t* e) {
    mouse_x = e->x;
    mouse_y = e->y;
    int gx = e->x / CELL, gy = e->y / CELL;
    if (e->kind == CANVAS_MOUSE_DOWN) {
        int t = tool_at(e->x, e->y);
        if (t >= 0) { tool = t; return; }
        if (e->y >= BAR_Y) return;
        painting = e->buttons == CANVAS_RIGHT ? 2 : 1;
        last_px = gx;
        last_py = gy;
    }
    if (e->kind == CANVAS_MOUSE_UP) {
        painting = 0;
        return;
    }
    if (!painting) return;
    if (e->kind == CANVAS_MOUSE_MOVE && !(e->buttons & (CANVAS_LEFT | CANVAS_RIGHT))) {
        painting = 0;
        return;
    }
    int keep = tool;
    if (painting == 2) tool = EMPTY;
    paint_line(last_px, last_py, gx, gy);
    tool = keep;
    last_px = gx;
    last_py = gy;
}

void _start(void) {
    if (canvas_open(&c, "Sandbox", W, H) != 0) {
        writeln("sandbox: couldn't open a window");
        exit();
    }
    writeln("sandbox: left-drag paints, right-drag erases, 1-9 0 - pick; escape quits");
    seed ^= uptime_ms() * 0x9e3779b97f4a7c15ull;
    demo_scene();

    for (;;) {
        uint64_t now = uptime_ms();
        canvas_event_t e;
        while (canvas_poll(&c, &e) > 0) {
            if (e.kind == CANVAS_CLOSED) quit();
            if (e.kind == CANVAS_KEY) key(e.key);
            if (e.kind == CANVAS_SCROLL) set_brush(brush + (e.key > 0 ? 1 : -1));
            if (e.kind == CANVAS_MOUSE_DOWN || e.kind == CANVAS_MOUSE_UP || e.kind == CANVAS_MOUSE_MOVE) mouse(&e);
        }
        // Holding the button still keeps pouring.
        if (painting && mouse_y < BAR_Y) {
            int keep = tool;
            if (painting == 2) tool = EMPTY;
            paint_at(mouse_x / CELL, mouse_y / CELL);
            tool = keep;
        }
        if (!paused) step();
        draw();

        uint64_t spent = uptime_ms() - now;
        sleep_ms(spent < 30 ? 30 - spent : 5);
    }
}
