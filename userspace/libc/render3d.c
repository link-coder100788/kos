// The 3D renderer: see render3d.h.
//
// Every polygon takes the same path. The model and view transforms carry
// it into the camera's frame (x right, y up, z straight ahead), where it's
// lit by its face normal and fogged by its distance. Whatever is behind
// the near plane is cut off, and the rest is projected onto the screen and
// filled a row at a time. The depth buffer holds 1/z rather than z: 1/z
// changes evenly across a polygon on the screen, so each pixel's is one
// addition from its neighbour's.

#include "render3d.h"
#include "math.h"

#define MAX_POINTS 40      // a polygon's points, once clipping adds some
#define MAX_SIDES 32
#define MAX_COLS 256       // the widest heightfield

float r3d_length(r3d_vec a) {
    return (float)sqrt(r3d_dot(a, a));
}

r3d_vec r3d_normalize(r3d_vec a) {
    float l = r3d_length(a);
    return l > 0 ? r3d_mul(a, 1 / l) : a;
}

static float clampf(float v, float lo, float hi) {
    return v < lo ? lo : v > hi ? hi : v;
}

// a + (b - a) * t, for each channel of two colors.
static uint32_t mix(uint32_t a, uint32_t b, float t) {
    uint32_t out = 0;
    for (int shift = 0; shift <= 16; shift += 8) {
        float x = (a >> shift & 255), y = (b >> shift & 255);
        out |= (uint32_t)(x + (y - x) * t + 0.5f) << shift;
    }
    return out;
}

// ---- transforms ----------------------------------------------------------------

static const r3d_mat IDENTITY = { { { 1, 0, 0, 0 }, { 0, 1, 0, 0 }, { 0, 0, 1, 0 } } };

// a then b, as one transform: b happens first.
static r3d_mat mat_mul(const r3d_mat* a, const r3d_mat* b) {
    r3d_mat o;
    for (int i = 0; i < 3; i++) {
        for (int j = 0; j < 4; j++) {
            float s = j == 3 ? a->m[i][3] : 0;
            for (int k = 0; k < 3; k++) s += a->m[i][k] * b->m[k][j];
            o.m[i][j] = s;
        }
    }
    return o;
}

static r3d_vec apply(const r3d_mat* m, r3d_vec p) {
    return r3d_v(m->m[0][0] * p.x + m->m[0][1] * p.y + m->m[0][2] * p.z + m->m[0][3],
                 m->m[1][0] * p.x + m->m[1][1] * p.y + m->m[1][2] * p.z + m->m[1][3],
                 m->m[2][0] * p.x + m->m[2][1] * p.y + m->m[2][2] * p.z + m->m[2][3]);
}

// The model transform followed by the view: from a shape's own
// coordinates straight to the camera's frame.
static const r3d_mat* model_view(r3d_t* r) {
    if (r->model_view_stale) {
        r->model_view = mat_mul(&r->view, &r->model);
        r->model_view_stale = 0;
    }
    return &r->model_view;
}

// Applies t to what's drawn next, inside the current model transform.
static void model_mul(r3d_t* r, const r3d_mat* t) {
    r->model = mat_mul(&r->model, t);
    r->model_view_stale = 1;
}

void r3d_push(r3d_t* r) {
    if (r->stack_size < R3D_STACK) r->stack[r->stack_size++] = r->model;
}

void r3d_pop(r3d_t* r) {
    if (r->stack_size > 0) r->model = r->stack[--r->stack_size];
    r->model_view_stale = 1;
}

void r3d_identity(r3d_t* r) {
    r->model = IDENTITY;
    r->model_view_stale = 1;
}

void r3d_translate(r3d_t* r, float x, float y, float z) {
    r3d_mat t = { { { 1, 0, 0, x }, { 0, 1, 0, y }, { 0, 0, 1, z } } };
    model_mul(r, &t);
}

void r3d_rotate_y(r3d_t* r, float angle) {
    float c = (float)cos(angle), s = (float)sin(angle);
    r3d_mat t = { { { c, 0, s, 0 }, { 0, 1, 0, 0 }, { -s, 0, c, 0 } } };
    model_mul(r, &t);
}

void r3d_rotate_x(r3d_t* r, float angle) {
    float c = (float)cos(angle), s = (float)sin(angle);
    r3d_mat t = { { { 1, 0, 0, 0 }, { 0, c, s, 0 }, { 0, -s, c, 0 } } };
    model_mul(r, &t);
}

void r3d_rotate_z(r3d_t* r, float angle) {
    float c = (float)cos(angle), s = (float)sin(angle);
    r3d_mat t = { { { c, -s, 0, 0 }, { s, c, 0, 0 }, { 0, 0, 1, 0 } } };
    model_mul(r, &t);
}

void r3d_scale(r3d_t* r, float x, float y, float z) {
    r3d_mat t = { { { x, 0, 0, 0 }, { 0, y, 0, 0 }, { 0, 0, z, 0 } } };
    model_mul(r, &t);
}

// ---- setup and the camera ----------------------------------------------------------

int r3d_init(r3d_t* r, canvas_t* c, int scale) {
    if (scale < 1 || scale > 8) return -1;
    int w = c->width / scale, h = c->height / scale;
    if (w < 1 || h < 1) return -1;
    *r = (r3d_t){ 0 };
    r->screen = *c;
    r->screen.width = w;
    r->screen.height = h;
    r->screen.pixels = alloc_z((size_t)w * h * sizeof(uint32_t));
    r->depth = alloc_z((size_t)w * h * sizeof(float));
    if (!r->screen.pixels || !r->depth) {
        r3d_free(r);
        return -1;
    }
    r->canvas = c;
    r->scale = scale;

    r->fov = 70 * (float)M_PI / 180;
    r->near = 0.1f;
    r->sun = r3d_v(-0.4f, 0.8f, -0.45f);
    r->sun_light = r3d_v(0.55f, 0.55f, 0.5f);
    r->ambient = r3d_v(0.45f, 0.47f, 0.55f);
    r->sky_top = rgb(60, 110, 200);
    r->sky_horizon = rgb(190, 215, 240);
    r->ground = rgb(90, 140, 80);
    r->fog_color = r->sky_horizon;
    r3d_camera(r, r3d_v(0, 0, 0), 0, 0);
    r3d_identity(r);
    return 0;
}

void r3d_free(r3d_t* r) {
    free(r->screen.pixels);
    free(r->depth);
    r->screen.pixels = 0;
    r->depth = 0;
}

void r3d_camera(r3d_t* r, r3d_vec eye, float yaw, float pitch) {
    r->eye = eye;
    r->yaw = yaw;
    r->pitch = pitch;
    // Turn the world by -yaw about the vertical, then by -pitch about the
    // sideways axis, so the way the camera looks ends up along +z.
    float cy = (float)cos(yaw), sy = (float)sin(yaw), cp = (float)cos(pitch), sp = (float)sin(pitch);
    r3d_mat v = { { { cy, 0, -sy, 0 }, { -sy * sp, cp, -cy * sp, 0 }, { sy * cp, sp, cy * cp, 0 } } };
    for (int i = 0; i < 3; i++) v.m[i][3] = -(v.m[i][0] * eye.x + v.m[i][1] * eye.y + v.m[i][2] * eye.z);
    r->view = v;
    r->model_view_stale = 1;
    r->focal = r->screen.width / 2.0f / (float)tan(r->fov / 2);
}

void r3d_look_at(r3d_t* r, r3d_vec eye, r3d_vec target) {
    r3d_vec d = r3d_sub(target, eye);
    float yaw = (float)atan2(d.x, d.z);
    float pitch = (float)atan2(d.y, sqrt(d.x * d.x + d.z * d.z));
    r3d_camera(r, eye, yaw, pitch);
}

int r3d_horizon(const r3d_t* r) {
    double y = r->screen.height / 2.0 + r->focal * tan(r->pitch);
    return (int)(y < -1e6 ? -1e6 : y > 1e6 ? 1e6 : y);
}

float r3d_project(r3d_t* r, r3d_vec p, int* x, int* y) {
    r3d_vec v = apply(&r->view, p);
    if (v.z < r->near) return 0;
    *x = (int)((r->screen.width / 2.0f + r->focal * v.x / v.z) * r->scale);
    *y = (int)((r->screen.height / 2.0f - r->focal * v.y / v.z) * r->scale);
    return v.z;
}

int r3d_visible(r3d_t* r, r3d_vec p, float radius) {
    r3d_vec v = apply(&r->view, p);
    if (v.z + radius < r->near) return 0;
    if (r->fog_far > 0 && v.z - radius > r->fog_far) return 0;
    // How far outside each side of the view the centre is, against the radius.
    float tx = r->screen.width / 2.0f / r->focal, ty = r->screen.height / 2.0f / r->focal;
    if (fabs(v.x) - v.z * tx > radius * (float)sqrt(1 + tx * tx)) return 0;
    if (fabs(v.y) - v.z * ty > radius * (float)sqrt(1 + ty * ty)) return 0;
    return 1;
}

// ---- frames ----------------------------------------------------------------------

void r3d_begin(r3d_t* r) {
    int w = r->screen.width, h = r->screen.height;
    r->focal = w / 2.0f / (float)tan(r->fov / 2);
    r3d_vec sun = r3d_normalize(r->sun);
    r3d_mat turn = r->view;
    turn.m[0][3] = turn.m[1][3] = turn.m[2][3] = 0;
    r->sun_view = apply(&turn, sun);
    r3d_identity(r);
    r->stack_size = 0;
    r->polygons = 0;

    int horizon = r3d_horizon(r);
    for (int y = 0; y < h; y++) {
        uint32_t color = y < horizon ? mix(r->sky_top, r->sky_horizon, (y + 0.5f) / horizon) : r->ground;
        uint32_t* row = r->screen.pixels + y * w;
        float* depth = r->depth + y * w;
        for (int x = 0; x < w; x++) {
            row[x] = color;
            depth[x] = 0;
        }
    }
}

void r3d_end(r3d_t* r) {
    int w = r->screen.width, h = r->screen.height, s = r->scale;
    int stride = r->canvas->width;
    for (int y = 0; y < h; y++) {
        const uint32_t* src = r->screen.pixels + y * w;
        for (int line = 0; line < s; line++) {
            int dim = r->scanlines && s > 1 && line == s - 1;
            uint32_t* dst = r->canvas->pixels + (y * s + line) * stride;
            for (int x = 0; x < w; x++) {
                uint32_t p = src[x];
                if (dim) p = ((p >> 1) & 0x7f7f7f) + ((p >> 2) & 0x3f3f3f);  // 3/4 as bright
                for (int k = 0; k < s; k++) *dst++ = p;
            }
        }
    }
}

// ---- filling polygons -------------------------------------------------------------

static int iceil(float v) {
    int i = (int)v;
    return i < v ? i + 1 : i;
}

// Fills a convex polygon on the screen, where each point's w is its 1/z,
// keeping only the pixels nearer than what's already there.
static void fill(r3d_t* r, const float* px, const float* py, const float* pw, int n, uint32_t color) {
    // 1/z across the polygon is a plane, w = a*x + b*y + c: find it from the
    // widest triangle of its points, which the rounding upsets least.
    float best = 0, a = 0, b = 0;
    for (int i = 1; i + 1 < n; i++) {
        float dx1 = px[i] - px[0], dy1 = py[i] - py[0], dw1 = pw[i] - pw[0];
        float dx2 = px[i + 1] - px[0], dy2 = py[i + 1] - py[0], dw2 = pw[i + 1] - pw[0];
        float det = dx1 * dy2 - dx2 * dy1;
        if (fabs(det) > fabs(best)) {
            best = det;
            a = (dw1 * dy2 - dw2 * dy1) / det;
            b = (dx1 * dw2 - dx2 * dw1) / det;
        }
    }
    if (fabs(best) < 1e-4) return;  // edge-on: nothing to see
    float c = pw[0] - a * px[0] - b * py[0];

    int w = r->screen.width, h = r->screen.height;
    float top = py[0], bottom = py[0];
    for (int i = 1; i < n; i++) {
        if (py[i] < top) top = py[i];
        if (py[i] > bottom) bottom = py[i];
    }
    int y0 = iceil(clampf(top, -1, h + 1) - 0.5f), y1 = iceil(clampf(bottom, -1, h + 1) - 0.5f);
    if (y0 < 0) y0 = 0;
    if (y1 > h) y1 = h;
    for (int y = y0; y < y1; y++) {
        // Where this row's pixel centres cross the polygon's edges.
        float yc = y + 0.5f, lo = 1e9f, hi = -1e9f;
        for (int i = 0, j = n - 1; i < n; j = i++) {
            if ((py[j] <= yc) == (py[i] <= yc)) continue;
            float x = px[j] + (yc - py[j]) * (px[i] - px[j]) / (py[i] - py[j]);
            if (x < lo) lo = x;
            if (x > hi) hi = x;
        }
        if (lo > hi) continue;
        int x0 = iceil(clampf(lo, -1, w + 1) - 0.5f), x1 = iceil(clampf(hi, -1, w + 1) - 0.5f);
        if (x0 < 0) x0 = 0;
        if (x1 > w) x1 = w;
        uint32_t* row = r->screen.pixels + y * w;
        float* depth = r->depth + y * w;
        float z = a * (x0 + 0.5f) + b * yc + c;
        for (int x = x0; x < x1; x++, z += a) {
            if (z > depth[x]) {
                depth[x] = z;
                row[x] = color;
            }
        }
    }
    r->polygons++;
}

static uint32_t fog(const r3d_t* r, uint32_t color, float depth) {
    if (r->fog_far <= 0) return color;
    float t = clampf((depth - r->fog_near) / (r->fog_far - r->fog_near), 0, 1);
    return t > 0 ? mix(color, r->fog_color, t) : color;
}

// A polygon in the camera's frame: lit, fogged, clipped and filled.
// `inside` is a point inside the solid it's a side of, so it's skipped if
// it faces away; or NULL for a flat polygon, seen from both sides.
static void face(r3d_t* r, const r3d_vec* v, int n, uint32_t color, const r3d_vec* inside) {
    // The normal, by Newell's method, which any number of points can share.
    r3d_vec normal = { 0, 0, 0 };
    float depth = 0;
    int ahead = 0;
    for (int i = 0; i < n; i++) {
        r3d_vec p = v[i], q = v[(i + 1) % n];
        normal.x += (p.y - q.y) * (p.z + q.z);
        normal.y += (p.z - q.z) * (p.x + q.x);
        normal.z += (p.x - q.x) * (p.y + q.y);
        depth += p.z;
        ahead |= p.z >= r->near;
    }
    if (!ahead) return;
    normal = r3d_normalize(normal);
    if (inside) {
        if (r3d_dot(normal, r3d_sub(v[0], *inside)) < 0) normal = r3d_mul(normal, -1);
        if (r3d_dot(normal, v[0]) >= 0) return;  // its back is to the camera
    } else if (r3d_dot(normal, v[0]) > 0) {
        normal = r3d_mul(normal, -1);  // light the side we see
    }

    if (!r->unlit) {
        float d = r3d_dot(normal, r->sun_view);
        if (d < 0) d = 0;
        r3d_vec light = r3d_add(r->ambient, r3d_mul(r->sun_light, d));
        uint32_t lit = 0;
        float scales[3] = { light.z, light.y, light.x };  // blue, green, red
        for (int k = 0; k < 3; k++) {
            float c = (color >> (8 * k) & 255) * scales[k];
            lit |= (uint32_t)clampf(c, 0, 255) << (8 * k);
        }
        color = lit;
    }
    color = fog(r, color, depth / n);

    // Cut off what's behind the near plane, and project what's left.
    float px[MAX_POINTS], py[MAX_POINTS], pw[MAX_POINTS];
    float cx = r->screen.width / 2.0f, cy = r->screen.height / 2.0f, f = r->focal, near = r->near;
    int m = 0;
    for (int i = 0; i < n && m + 2 <= MAX_POINTS; i++) {
        r3d_vec p = v[i], q = v[(i + 1) % n];
        int p_in = p.z >= near, q_in = q.z >= near;
        if (p_in) {
            px[m] = cx + f * p.x / p.z;
            py[m] = cy - f * p.y / p.z;
            pw[m++] = 1 / p.z;
        }
        if (p_in != q_in) {
            float t = (near - p.z) / (q.z - p.z);
            px[m] = cx + f * (p.x + t * (q.x - p.x)) / near;
            py[m] = cy - f * (p.y + t * (q.y - p.y)) / near;
            pw[m++] = 1 / near;
        }
    }
    if (m >= 3) fill(r, px, py, pw, m, color);
}

// ---- flat things ----------------------------------------------------------------

void r3d_polygon(r3d_t* r, const r3d_vec* points, int n, uint32_t color) {
    if (n < 3 || n > MAX_SIDES) return;
    const r3d_mat* m = model_view(r);
    r3d_vec v[MAX_SIDES];
    for (int i = 0; i < n; i++) v[i] = apply(m, points[i]);
    face(r, v, n, color, 0);
}

void r3d_triangle(r3d_t* r, r3d_vec a, r3d_vec b, r3d_vec c, uint32_t color) {
    r3d_vec p[3] = { a, b, c };
    r3d_polygon(r, p, 3, color);
}

void r3d_quad(r3d_t* r, r3d_vec a, r3d_vec b, r3d_vec c, r3d_vec d, uint32_t color) {
    r3d_vec p[4] = { a, b, c, d };
    r3d_polygon(r, p, 4, color);
}

void r3d_line(r3d_t* r, r3d_vec a, r3d_vec b, uint32_t color) {
    const r3d_mat* m = model_view(r);
    r3d_vec p = apply(m, a), q = apply(m, b);
    float near = r->near;
    if (p.z < near && q.z < near) return;
    if (p.z < near) p = r3d_add(p, r3d_mul(r3d_sub(q, p), (near - p.z) / (q.z - p.z)));
    if (q.z < near) q = r3d_add(q, r3d_mul(r3d_sub(p, q), (near - q.z) / (p.z - q.z)));
    color = fog(r, color, (p.z + q.z) / 2);

    int w = r->screen.width, h = r->screen.height;
    float cx = w / 2.0f, cy = h / 2.0f, f = r->focal;
    float x0 = cx + f * p.x / p.z, y0 = cy - f * p.y / p.z, w0 = 1 / p.z;
    float x1 = cx + f * q.x / q.z, y1 = cy - f * q.y / q.z, w1 = 1 / q.z;
    float steps = fmax(fabs(x1 - x0), fabs(y1 - y0));
    if (steps > 4 * (w + h)) steps = 4 * (w + h);  // one that runs far off the screen
    int n = (int)steps + 1;
    for (int i = 0; i <= n; i++) {
        float t = (float)i / n;
        int x = (int)floor(x0 + (x1 - x0) * t), y = (int)floor(y0 + (y1 - y0) * t);
        if (x < 0 || y < 0 || x >= w || y >= h) continue;
        float z = w0 + (w1 - w0) * t;
        // A little leeway, so a line along a surface isn't hidden by it.
        float* depth = r->depth + y * w + x;
        if (z * 1.002f >= *depth) {
            if (z > *depth) *depth = z;
            r->screen.pixels[y * w + x] = color;
        }
    }
}

// ---- solid shapes ---------------------------------------------------------------

// The corners of a regular polygon, worked out once for each number of sides.
static float ring_cos[MAX_SIDES + 1][MAX_SIDES], ring_sin[MAX_SIDES + 1][MAX_SIDES];
static uint8_t ring_ready[MAX_SIDES + 1];

static int ring(int sides) {
    sides = sides < 3 ? 3 : sides > MAX_SIDES ? MAX_SIDES : sides;
    if (!ring_ready[sides]) {
        for (int k = 0; k < sides; k++) {
            double a = 2 * M_PI * k / sides;
            ring_cos[sides][k] = (float)cos(a);
            ring_sin[sides][k] = (float)sin(a);
        }
        ring_ready[sides] = 1;
    }
    return sides;
}

void r3d_box(r3d_t* r, float w, float h, float d, uint32_t color) {
    static const int sides[6][4] = {
        { 0, 2, 6, 4 }, { 1, 3, 7, 5 }, { 0, 1, 5, 4 }, { 2, 3, 7, 6 }, { 0, 1, 3, 2 }, { 4, 5, 7, 6 },
    };
    const r3d_mat* m = model_view(r);
    r3d_vec p[8];
    for (int k = 0; k < 8; k++) {
        p[k] = apply(m, r3d_v(k & 1 ? w / 2 : -w / 2, k & 2 ? h / 2 : -h / 2, k & 4 ? d / 2 : -d / 2));
    }
    r3d_vec inside = apply(m, r3d_v(0, 0, 0));
    for (int s = 0; s < 6; s++) {
        r3d_vec quad[4] = { p[sides[s][0]], p[sides[s][1]], p[sides[s][2]], p[sides[s][3]] };
        face(r, quad, 4, color, &inside);
    }
}

void r3d_cylinder(r3d_t* r, float radius, float top_radius, float height, int sides, uint32_t color) {
    sides = ring(sides);
    const r3d_mat* m = model_view(r);
    r3d_vec lo[MAX_SIDES], hi[MAX_SIDES];
    for (int k = 0; k < sides; k++) {
        float c = ring_cos[sides][k], s = ring_sin[sides][k];
        lo[k] = apply(m, r3d_v(radius * c, 0, radius * s));
        hi[k] = apply(m, r3d_v(top_radius * c, height, top_radius * s));
    }
    r3d_vec inside = apply(m, r3d_v(0, height * (top_radius > 0 ? 0.5f : 0.25f), 0));
    for (int k = 0; k < sides; k++) {
        int n = (k + 1) % sides;
        if (top_radius > 0) {
            r3d_vec quad[4] = { lo[k], lo[n], hi[n], hi[k] };
            face(r, quad, 4, color, &inside);
        } else {
            r3d_vec tri[3] = { lo[k], lo[n], hi[0] };  // every hi[] is the point
            face(r, tri, 3, color, &inside);
        }
    }
    face(r, lo, sides, color, &inside);
    if (top_radius > 0) face(r, hi, sides, color, &inside);
}

void r3d_cone(r3d_t* r, float radius, float height, int sides, uint32_t color) {
    r3d_cylinder(r, radius, 0, height, sides, color);
}

void r3d_sphere(r3d_t* r, float radius, int detail, uint32_t color) {
    int bands = detail < 2 ? 2 : detail > 16 ? 16 : detail;
    int sides = ring(2 * bands);
    const r3d_mat* m = model_view(r);
    // Rings of points from the top pole (band 0) to the bottom one.
    static r3d_vec p[17][MAX_SIDES];
    for (int i = 0; i <= bands; i++) {
        double a = M_PI * i / bands;
        float y = radius * (float)cos(a), rr = radius * (float)sin(a);
        for (int k = 0; k < sides; k++) p[i][k] = apply(m, r3d_v(rr * ring_cos[sides][k], y, rr * ring_sin[sides][k]));
    }
    r3d_vec inside = apply(m, r3d_v(0, 0, 0));
    for (int i = 0; i < bands; i++) {
        for (int k = 0; k < sides; k++) {
            int n = (k + 1) % sides;
            if (i == 0) {
                r3d_vec tri[3] = { p[0][0], p[1][k], p[1][n] };
                face(r, tri, 3, color, &inside);
            } else if (i == bands - 1) {
                r3d_vec tri[3] = { p[i][k], p[i][n], p[bands][0] };
                face(r, tri, 3, color, &inside);
            } else {
                r3d_vec quad[4] = { p[i][k], p[i][n], p[i + 1][n], p[i + 1][k] };
                face(r, quad, 4, color, &inside);
            }
        }
    }
}

void r3d_draw_mesh(r3d_t* r, const r3d_mesh* mesh) {
    const r3d_mat* m = model_view(r);
    for (int i = 0; i < mesh->n_faces; i++) {
        const r3d_face* f = &mesh->faces[i];
        if (f->a < 0 || f->b < 0 || f->c < 0 || f->a >= mesh->n_points || f->b >= mesh->n_points
            || f->c >= mesh->n_points) continue;
        r3d_vec tri[3] = { apply(m, mesh->points[f->a]), apply(m, mesh->points[f->b]), apply(m, mesh->points[f->c]) };
        face(r, tri, 3, f->color, 0);
    }
}

// ---- ground ---------------------------------------------------------------------

void r3d_heightfield(r3d_t* r, float x0, float z0, int cols, int rows, float cell,
                     float (*height)(float x, float z), uint32_t (*color)(float x, float z)) {
    if (cols < 1 || rows < 1) return;
    if (cols > MAX_COLS) cols = MAX_COLS;
    const r3d_mat* m = model_view(r);
    // Two rows of corners at a time, already in the camera's frame.
    static r3d_vec rows_buf[2][MAX_COLS + 1];
    r3d_vec* back = rows_buf[0];
    r3d_vec* front = rows_buf[1];
    for (int j = 0; j <= rows; j++) {
        float z = z0 + j * cell;
        for (int i = 0; i <= cols; i++) {
            float x = x0 + i * cell;
            front[i] = apply(m, r3d_v(x, height(x, z), z));
        }
        if (j > 0) {
            for (int i = 0; i < cols; i++) {
                uint32_t c = color(x0 + (i + 0.5f) * cell, z - 0.5f * cell);
                r3d_vec a[3] = { back[i], back[i + 1], front[i + 1] };
                r3d_vec b[3] = { back[i], front[i + 1], front[i] };
                face(r, a, 3, c, 0);
                face(r, b, 3, c, 0);
            }
        }
        r3d_vec* t = back;
        back = front;
        front = t;
    }
}

float r3d_heightfield_at(float x0, float z0, float cell, float (*height)(float x, float z), float x, float z) {
    float gx = (x - x0) / cell, gz = (z - z0) / cell;
    float i = (float)floor(gx), j = (float)floor(gz);
    float fx = gx - i, fz = gz - j;
    float ax = x0 + i * cell, az = z0 + j * cell;
    float h00 = height(ax, az), h11 = height(ax + cell, az + cell);
    if (fx >= fz) {
        float h10 = height(ax + cell, az);
        return h00 + fx * (h10 - h00) + fz * (h11 - h10);
    }
    float h01 = height(ax, az + cell);
    return h00 + fz * (h01 - h00) + fx * (h11 - h01);
}
