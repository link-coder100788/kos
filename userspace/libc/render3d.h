#ifndef C_RENDER3D_H
#define C_RENDER3D_H

// A software 3D renderer for canvases, for games: flat-shaded polygons
// with a depth buffer, a camera, a sun, fog and a sky, drawn at a low
// resolution and scaled up into the canvas for chunky retro pixels. You
// place shapes in a world; it works out perspective, clipping, lighting
// and what hides what.
//
//     canvas_t c;
//     r3d_t r;
//     canvas_open(&c, "Cube", 640, 480);
//     r3d_init(&r, &c, 2);                       // renders at 320x240
//     for (double t = 0;; t += 0.02) {
//         ... canvas_poll(&c, &e) ...
//         r3d_look_at(&r, r3d_v(0, 3, -6), r3d_v(0, 0, 0));
//         r3d_begin(&r);                         // sky, empty depth buffer
//         r3d_push(&r);
//         r3d_rotate_y(&r, t);
//         r3d_box(&r, 2, 2, 2, rgb(220, 60, 50));
//         r3d_pop(&r);
//         r3d_end(&r);                           // scaled up into c
//         canvas_present(&c);
//         sleep_ms(20);
//     }
//
// The world: x is right, y up and z forward (where a camera with yaw 0
// looks), in whatever unit you like (metres work well). Angles are in
// radians. Colors are rgb() from canvas.h.
//
// Shapes are drawn through the model transform, which r3d_begin resets to
// nothing: r3d_translate, r3d_rotate_* and r3d_scale move, turn and size
// what's drawn after them, and r3d_push / r3d_pop save and restore it, so
// a model's parts are placed relative to the model (an arm relative to a
// body, say). Positions passed to r3d_project and r3d_visible are always
// in the world, whatever the transform.
//
// Between r3d_begin and r3d_end, r.screen is an ordinary canvas holding
// the low-resolution picture, so canvas.h's drawing works on it too: draw
// mountains on the sky before the 3D, or a HUD in fat pixels after it.
// After r3d_end, draw on the full-size canvas as usual.
//
// See userspace/prog/demo3d.c for a scene with every kind of shape, and
// games/powder/ for a game built on it.

#include <stdint.h>
#include "canvas.h"

typedef struct { float x, y, z; } r3d_vec;

static inline r3d_vec r3d_v(float x, float y, float z) { return (r3d_vec){ x, y, z }; }
static inline r3d_vec r3d_add(r3d_vec a, r3d_vec b) { return r3d_v(a.x + b.x, a.y + b.y, a.z + b.z); }
static inline r3d_vec r3d_sub(r3d_vec a, r3d_vec b) { return r3d_v(a.x - b.x, a.y - b.y, a.z - b.z); }
static inline r3d_vec r3d_mul(r3d_vec a, float s) { return r3d_v(a.x * s, a.y * s, a.z * s); }
static inline float r3d_dot(r3d_vec a, r3d_vec b) { return a.x * b.x + a.y * b.y + a.z * b.z; }
static inline r3d_vec r3d_cross(r3d_vec a, r3d_vec b) {
    return r3d_v(a.y * b.z - a.z * b.y, a.z * b.x - a.x * b.z, a.x * b.y - a.y * b.x);
}
float r3d_length(r3d_vec a);
// a scaled to length 1 (or a, if it's all zeros).
r3d_vec r3d_normalize(r3d_vec a);

// A transform: a 3x3 rotation and scale, then the translation in column 3.
typedef struct { float m[3][4]; } r3d_mat;

#define R3D_STACK 16

typedef struct {
    // The picture being drawn: see the top of this file.
    canvas_t screen;

    // ---- settings: change them whenever you like ----
    float fov;             // how wide the camera sees, in radians (default 70 degrees)
    float near;            // things nearer the camera than this aren't drawn (0.1)
    r3d_vec sun;           // the direction the sunlight comes from (any length)
    r3d_vec sun_light;     // how bright the sun is, red/green/blue, 0-1 each
    r3d_vec ambient;       // light that reaches everywhere, even out of the sun
    int unlit;             // nonzero: draw colors exactly as given, unshaded
    uint32_t fog_color;    // things fade into it from fog_near to fog_far away;
    float fog_near, fog_far;  // fog_far 0 means no fog
    uint32_t sky_top, sky_horizon, ground;  // what r3d_begin paints behind everything
    int scanlines;         // with scale > 1, dim every other line like an old monitor

    int polygons;          // how many polygons the last frame drew, for tuning

    // ---- the renderer's own ----
    canvas_t* canvas;
    int scale;
    float* depth;          // 1 / distance at each pixel; 0 is nothing yet
    r3d_vec eye;
    float yaw, pitch, focal;
    r3d_mat view, model, model_view;
    int model_view_stale;
    r3d_mat stack[R3D_STACK];
    int stack_size;
    r3d_vec sun_view;      // the sun's direction, turned to face the way the camera does
} r3d_t;

// ---- setup -----------------------------------------------------------------------

// Sets up r to draw into canvas c, at 1/scale of its size in each direction
// (scale 1-8). Fills in default settings: daylight, a blue sky, no fog.
// Returns 0, or -1 if the scale is out of range or memory runs out.
int r3d_init(r3d_t* r, canvas_t* c, int scale);

// Frees r's buffers (the canvas is yours to close).
void r3d_free(r3d_t* r);

// ---- the camera ------------------------------------------------------------------
// Set it before r3d_begin, which uses it to place the horizon.

// Puts the camera at `eye`, turned yaw to the right (0 looks along +z) and
// pitch up (negative looks down).
void r3d_camera(r3d_t* r, r3d_vec eye, float yaw, float pitch);

// Puts the camera at `eye`, looking at `target`.
void r3d_look_at(r3d_t* r, r3d_vec eye, r3d_vec target);

// ---- frames ----------------------------------------------------------------------

// Starts a frame: paints the sky and ground and forgets the last frame's
// depths, and resets the model transform.
void r3d_begin(r3d_t* r);

// Finishes a frame, scaling it up into the canvas (in its top-left corner).
// Call canvas_present afterwards, as usual.
void r3d_end(r3d_t* r);

// The row of r.screen the horizon is on (it can be off the screen).
int r3d_horizon(const r3d_t* r);

// Where world point p is on the canvas (full-size, so you can put a label
// on it after r3d_end). Returns its distance in front of the camera, or 0
// (leaving *x and *y alone) if it's behind.
float r3d_project(r3d_t* r, r3d_vec p, int* x, int* y);

// 1 if any of the ball of `radius` around world point p could be on
// screen (and nearer than fog_far): skip drawing things that aren't.
int r3d_visible(r3d_t* r, r3d_vec p, float radius);

// ---- the model transform -----------------------------------------------------------

void r3d_push(r3d_t* r);
void r3d_pop(r3d_t* r);
// Back to no transform.
void r3d_identity(r3d_t* r);
void r3d_translate(r3d_t* r, float x, float y, float z);
// Turns +z towards +x (like turning right).
void r3d_rotate_y(r3d_t* r, float angle);
// Tips +z up towards +y (like looking up).
void r3d_rotate_x(r3d_t* r, float angle);
// Rolls +x up towards +y (like leaning left).
void r3d_rotate_z(r3d_t* r, float angle);
void r3d_scale(r3d_t* r, float x, float y, float z);

// ---- drawing -----------------------------------------------------------------------
// Flat polygons are seen from both sides. The shapes are solid: only the
// sides facing the camera are drawn.

void r3d_triangle(r3d_t* r, r3d_vec a, r3d_vec b, r3d_vec c, uint32_t color);
void r3d_quad(r3d_t* r, r3d_vec a, r3d_vec b, r3d_vec c, r3d_vec d, uint32_t color);
// A flat, convex polygon of up to 32 points, in order around its edge.
void r3d_polygon(r3d_t* r, const r3d_vec* points, int n, uint32_t color);
// A line, one pixel wide, unshaded: for wireframes and vector-style games.
void r3d_line(r3d_t* r, r3d_vec a, r3d_vec b, uint32_t color);

// A box w wide (x), h high (y) and d deep (z), centred on the origin.
void r3d_box(r3d_t* r, float w, float h, float d, uint32_t color);
// A cone with its base (a `sides`-sided polygon, 3-32) on y = 0 and its
// point `height` above.
void r3d_cone(r3d_t* r, float radius, float height, int sides, uint32_t color);
// A cylinder from y = 0 up to y = height. top_radius narrows (or widens) the
// top: equal to radius for a plain cylinder, smaller for a stump or a lampshade.
void r3d_cylinder(r3d_t* r, float radius, float top_radius, float height, int sides, uint32_t color);
// A ball centred on the origin, with `detail` bands from pole to pole (2-16):
// 3 or 4 looks like a gem, 8 or more nearly round.
void r3d_sphere(r3d_t* r, float radius, int detail, uint32_t color);

// A model of your own: points, and triangles joining them by index.
typedef struct { int a, b, c; uint32_t color; } r3d_face;
typedef struct {
    const r3d_vec* points;
    int n_points;
    const r3d_face* faces;
    int n_faces;
} r3d_mesh;
void r3d_draw_mesh(r3d_t* r, const r3d_mesh* mesh);

// Ground: a grid of cols x rows square cells, `cell` apart, from (x0, z0)
// towards +x and +z, with height(x, z) at each corner and each cell painted
// color(x, z) (given the cell's middle). Each cell is two flat triangles,
// split from its (x0, z0) corner to the opposite one.
void r3d_heightfield(r3d_t* r, float x0, float z0, int cols, int rows, float cell,
                     float (*height)(float x, float z), uint32_t (*color)(float x, float z));

// The height of such a grid's surface at (x, z), exactly as drawn, for
// standing things on it. The grid's corners are at whole multiples of
// `cell` from (x0, z0), anywhere you like.
float r3d_heightfield_at(float x0, float z0, float cell, float (*height)(float x, float z), float x, float z);

#endif //C_RENDER3D_H
