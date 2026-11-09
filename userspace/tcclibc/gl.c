// gl.h: the OpenGL state lives here, in the program; drawing goes to the
// kos-gl device as a stream of commands (the format is described in
// tools/qemu/hw/display/kos-gl.c), sent with syscall 18 when a frame is
// done or the stream gets big.
//
// Vertices go with each draw, 13 floats apiece (position, color, normal,
// texture coordinate); quads and polygons become triangles here, since
// the device only draws what OpenGL 4.1 can. The fixed-function state
// (matrices, lights, fog...) is one command, sent before a draw when it
// changed.

#include "gl.h"
#include "math.h"
#include "stdio.h"
#include "stdlib.h"
#include "string.h"
#include "sys.h"
#include "syscall.h"

// Syscall 18 is the GPU syscall; the sub-command goes in x16.
static uint64_t gpu_svc(uint64_t sub, uint64_t a0, uint64_t a1, uint64_t a2, uint64_t a3, uint64_t a4) {
    return KOS_SVC(18, sub, a0, a1, a2, a3, a4);
}

enum { GPU_AVAILABLE = 0, GPU_OPEN = 1, GPU_SUBMIT = 2, GPU_HLSL = 3, GPU_CLOSE = 4, GPU_GLSL = 5 };

// The device's commands.
enum {
    CMD_VIEWPORT = 1, CMD_CLEAR_COLOR = 2, CMD_CLEAR_DEPTH = 3, CMD_CLEAR = 4, CMD_ENABLE = 5,
    CMD_DISABLE = 6, CMD_BLEND_FUNC = 7, CMD_DEPTH_FUNC = 8, CMD_DEPTH_MASK = 9, CMD_CULL_FACE = 10,
    CMD_FRONT_FACE = 11, CMD_TEX_IMAGE = 12, CMD_TEX_DELETE = 13, CMD_TEX_PARAM = 14,
    CMD_BIND_TEXTURE = 15, CMD_USE_PROGRAM = 16, CMD_PROGRAM_DELETE = 17, CMD_UNIFORM = 18,
    CMD_FIXED_STATE = 19, CMD_DRAW = 20, CMD_GENERATE_MIPMAP = 21, CMD_SCISSOR = 22,
    CMD_COLOR_MASK = 23, CMD_POLYGON_OFFSET = 24, CMD_DRAW_ATTRIBS = 25, CMD_POLYGON_MODE = 26,
};
enum { UNIFORM_FLOAT = 1, UNIFORM_INT = 2, UNIFORM_MAT3 = 3, UNIFORM_MAT4 = 4 };

#define VERTEX_FLOATS 13
#define MAX_LIGHTS 4
#define FIXED_LIGHTING       (1u << 0)
#define FIXED_COLOR_MATERIAL (1u << 1)
#define FIXED_TEXTURE        (1u << 2)
#define FIXED_LIGHT0         (1u << 3)

// Exactly the device's FixedState.
typedef struct {
    float position[4], ambient[4], diffuse[4], specular[4];
} fixed_light;

typedef struct {
    float modelview[16], projection[16];
    uint32_t flags;
    float scene_ambient[4];
    float mat_ambient[4], mat_diffuse[4], mat_specular[4], mat_emission[4];
    float shininess;
    fixed_light lights[MAX_LIGHTS];
    uint32_t fog_mode;
    float fog_start, fog_end, fog_density, fog_color[4];
    uint32_t alpha_func;
    float alpha_ref;
} fixed_state;

// The stream is sent once it's this big (in 32-bit words), and no one
// command is bigger than MAX_COMMAND (the kernel takes 8 MiB at once).
#define FLUSH_WORDS (1u << 20)
#define MAX_COMMAND (1u << 18)
#define MAX_DRAW ((MAX_COMMAND - 4) / VERTEX_FLOATS)
#define MODELVIEW_DEPTH 32
#define PROJECTION_DEPTH 4
#define MAX_UNIFORMS 256
#define MAX_UNIFORM_NAME 63
#define TEXTURE_UNITS 8
#define MAX_SHADERS 32
#define MAX_ATTRIBS 8              // glVertexAttribPointer's locations
#define MAX_LINKABLE 32

typedef struct {
    int enabled;
    GLint size;
    GLenum type;
    GLsizei stride;
    const void* pointer;
} vertex_array;

// A buffer object: its data is kept here, and sent with each draw.
typedef struct {
    int used;
    uint8_t* data;
    size_t size;
} buffer_object;

// One glVertexAttribPointer location.
typedef struct {
    int enabled;
    GLint size;
    GLenum type;
    GLboolean normalized;
    GLsizei stride;
    GLuint buffer;             // 0: pointer is the program's memory
    const uint8_t* pointer;    // else an offset into the buffer
} attrib;

typedef struct {
    int used;
    attrib attribs[MAX_ATTRIBS];
    GLuint elements;           // the GL_ELEMENT_ARRAY_BUFFER
} vertex_array_object;

static struct {
    canvas_t* canvas;
    uint64_t ctx;              // 0: not open
    uint32_t* cmds;            // the stream, in words
    size_t len, cap;
    GLenum error;
    char message[2048];        // what kgl_error returns

    GLenum matrix_mode;
    float modelview[MODELVIEW_DEPTH][16];
    float projection[PROJECTION_DEPTH][16];
    float texture_matrix[16];  // accepted, and ignored
    int modelview_top, projection_top;

    fixed_state fixed;
    int fixed_dirty;
    int lighting, color_material, texture_2d, fog, alpha_test;
    uint32_t lights_on;
    GLenum fog_mode, alpha_func;
    int depth_test, blend, cull_face, scissor_test, polygon_offset_fill;

    int in_begin;
    GLenum begin_mode;
    float* verts;              // glBegin's vertices
    size_t nverts, verts_cap;
    float color[4], normal[3], uv[2];

    vertex_array vertex, color_array, normal_array, uv_array;

    GLuint next_texture, next_program, program;
    int unit;
    GLuint bound[TEXTURE_UNITS];
    struct {
        GLuint program;
        char name[MAX_UNIFORM_NAME + 1];
    } uniforms[MAX_UNIFORMS];
    int n_uniforms;

    // glCreateShader's shaders (shader n is shaders[n - 1]; type 0 is a
    // free slot), and glCreateProgram's programs (program 0 is free).
    struct {
        GLenum type;
        char* source;
        int compiled;
        char* log;
    } shaders[MAX_SHADERS];
    struct {
        GLuint program;
        char* vs;                 // the attached shaders' sources
        char* fs;
        int linked;
        char* log;
    } linkable[MAX_LINKABLE];

    // Buffer and vertex array object n is buffers[n - 1] / vaos[n - 1];
    // vertex array object 0 is default_vao.
    buffer_object* buffers;
    GLuint n_buffers;
    vertex_array_object* vaos;
    GLuint n_vaos;
    vertex_array_object default_vao;
    GLuint array_buffer, vao;
} gl;

static void set_error(GLenum e) {
    if (gl.error == GL_NO_ERROR) gl.error = e;
}

static void say(const char* why) {
    snprintf(gl.message, sizeof gl.message, "%s", why);
}

static uint32_t f2u(float f) {
    uint32_t u;
    memcpy(&u, &f, sizeof u);
    return u;
}

static void set4(float* dst, float a, float b, float c, float d) {
    dst[0] = a; dst[1] = b; dst[2] = c; dst[3] = d;
}

// ---- the stream --------------------------------------------------------------------

// Sends the stream; with `read`, then reads the frame into the canvas.
static int flush(int read) {
    if (!gl.ctx) return -1;
    if (gl.len == 0 && !read) return 0;
    uint64_t r = gpu_svc(GPU_SUBMIT, gl.ctx, (uint64_t)gl.cmds, gl.len * 4,
                         read ? (uint64_t)gl.canvas->pixels : 0, 0);
    gl.len = 0;
    if (r == (uint64_t)-1) {
        say(last_error());
        set_error(GL_INVALID_OPERATION);
        return -1;
    }
    return 0;
}

// Room for a command of `words` words (header included), its header
// written. NULL if there's no context or no memory.
static uint32_t* begin_command(uint32_t op, size_t words) {
    if (!gl.ctx) return NULL;
    if (gl.len + words > FLUSH_WORDS) flush(0);
    if (gl.len + words > gl.cap) {
        size_t cap = gl.cap ? gl.cap : 4096;
        while (cap < gl.len + words) cap *= 2;
        uint32_t* cmds = realloc(gl.cmds, cap * 4);
        if (!cmds) {
            set_error(GL_OUT_OF_MEMORY);
            return NULL;
        }
        gl.cmds = cmds;
        gl.cap = cap;
    }
    uint32_t* w = gl.cmds + gl.len;
    gl.len += words;
    w[0] = op;
    w[1] = (uint32_t)words;
    return w + 2;
}

static void command(uint32_t op, int n, uint32_t a, uint32_t b, uint32_t c, uint32_t d) {
    uint32_t* w = begin_command(op, 2 + n);
    if (!w) return;
    uint32_t args[4] = { a, b, c, d };
    for (int i = 0; i < n; i++) w[i] = args[i];
}

// ---- matrices ----------------------------------------------------------------------

static void identity(float* m) {
    memset(m, 0, 16 * sizeof(float));
    m[0] = m[5] = m[10] = m[15] = 1;
}

// out = a * b (column-major); out may be a or b.
static void mat_mul(float* out, const float* a, const float* b) {
    float r[16];
    for (int col = 0; col < 4; col++)
        for (int row = 0; row < 4; row++) {
            float sum = 0;
            for (int k = 0; k < 4; k++) sum += a[k * 4 + row] * b[col * 4 + k];
            r[col * 4 + row] = sum;
        }
    memcpy(out, r, sizeof r);
}

static float* current_matrix(void) {
    switch (gl.matrix_mode) {
    case GL_PROJECTION: return gl.projection[gl.projection_top];
    case GL_TEXTURE: return gl.texture_matrix;
    default: return gl.modelview[gl.modelview_top];
    }
}

static void matrix_changed(void) {
    gl.fixed_dirty = 1;
}

void glMatrixMode(GLenum mode) {
    if (mode != GL_MODELVIEW && mode != GL_PROJECTION && mode != GL_TEXTURE) {
        set_error(GL_INVALID_ENUM);
        return;
    }
    gl.matrix_mode = mode;
}

void glLoadIdentity(void) {
    identity(current_matrix());
    matrix_changed();
}

void glLoadMatrixf(const GLfloat* m) {
    memcpy(current_matrix(), m, 16 * sizeof(float));
    matrix_changed();
}

void glMultMatrixf(const GLfloat* m) {
    float* cur = current_matrix();
    mat_mul(cur, cur, m);
    matrix_changed();
}

void glPushMatrix(void) {
    if (gl.matrix_mode == GL_PROJECTION) {
        if (gl.projection_top + 1 >= PROJECTION_DEPTH) { set_error(GL_STACK_OVERFLOW); return; }
        memcpy(gl.projection[gl.projection_top + 1], gl.projection[gl.projection_top], 16 * sizeof(float));
        gl.projection_top++;
    } else if (gl.matrix_mode == GL_MODELVIEW) {
        if (gl.modelview_top + 1 >= MODELVIEW_DEPTH) { set_error(GL_STACK_OVERFLOW); return; }
        memcpy(gl.modelview[gl.modelview_top + 1], gl.modelview[gl.modelview_top], 16 * sizeof(float));
        gl.modelview_top++;
    }
}

void glPopMatrix(void) {
    int* top = gl.matrix_mode == GL_PROJECTION ? &gl.projection_top
             : gl.matrix_mode == GL_MODELVIEW ? &gl.modelview_top : NULL;
    if (!top) return;
    if (*top == 0) { set_error(GL_STACK_UNDERFLOW); return; }
    (*top)--;
    matrix_changed();
}

void glTranslatef(GLfloat x, GLfloat y, GLfloat z) {
    float m[16];
    identity(m);
    m[12] = x; m[13] = y; m[14] = z;
    glMultMatrixf(m);
}

void glScalef(GLfloat x, GLfloat y, GLfloat z) {
    float m[16];
    identity(m);
    m[0] = x; m[5] = y; m[10] = z;
    glMultMatrixf(m);
}

void glRotatef(GLfloat degrees, GLfloat x, GLfloat y, GLfloat z) {
    float len = (float)sqrt(x * x + y * y + z * z);
    if (len == 0) return;
    x /= len; y /= len; z /= len;
    float a = degrees * 3.14159265358979f / 180;
    float c = (float)cos(a), s = (float)sin(a), t = 1 - c;
    float m[16] = {
        x * x * t + c,     y * x * t + z * s, x * z * t - y * s, 0,
        x * y * t - z * s, y * y * t + c,     y * z * t + x * s, 0,
        x * z * t + y * s, y * z * t - x * s, z * z * t + c,     0,
        0, 0, 0, 1,
    };
    glMultMatrixf(m);
}

void glOrtho(GLdouble l, GLdouble r, GLdouble b, GLdouble t, GLdouble n, GLdouble f) {
    if (l == r || b == t || n == f) { set_error(GL_INVALID_VALUE); return; }
    float m[16];
    identity(m);
    m[0] = (float)(2 / (r - l));
    m[5] = (float)(2 / (t - b));
    m[10] = (float)(-2 / (f - n));
    m[12] = (float)(-(r + l) / (r - l));
    m[13] = (float)(-(t + b) / (t - b));
    m[14] = (float)(-(f + n) / (f - n));
    glMultMatrixf(m);
}

void gluOrtho2D(GLdouble left, GLdouble right, GLdouble bottom, GLdouble top) {
    glOrtho(left, right, bottom, top, -1, 1);
}

void glFrustum(GLdouble l, GLdouble r, GLdouble b, GLdouble t, GLdouble n, GLdouble f) {
    if (n <= 0 || f <= 0 || l == r || b == t || n == f) { set_error(GL_INVALID_VALUE); return; }
    float m[16] = { 0 };
    m[0] = (float)(2 * n / (r - l));
    m[5] = (float)(2 * n / (t - b));
    m[8] = (float)((r + l) / (r - l));
    m[9] = (float)((t + b) / (t - b));
    m[10] = (float)(-(f + n) / (f - n));
    m[11] = -1;
    m[14] = (float)(-2 * f * n / (f - n));
    glMultMatrixf(m);
}

void gluPerspective(GLdouble fovy, GLdouble aspect, GLdouble n, GLdouble f) {
    double top = n * tan(fovy * 3.14159265358979 / 360);
    glFrustum(-top * aspect, top * aspect, -top, top, n, f);
}

void gluLookAt(GLdouble ex, GLdouble ey, GLdouble ez, GLdouble ax, GLdouble ay, GLdouble az,
               GLdouble ux, GLdouble uy, GLdouble uz) {
    double fx = ax - ex, fy = ay - ey, fz = az - ez;
    double fl = sqrt(fx * fx + fy * fy + fz * fz);
    if (fl == 0) return;
    fx /= fl; fy /= fl; fz /= fl;
    // s = f x up, u = s x f
    double sx = fy * uz - fz * uy, sy = fz * ux - fx * uz, sz = fx * uy - fy * ux;
    double sl = sqrt(sx * sx + sy * sy + sz * sz);
    if (sl == 0) return;
    sx /= sl; sy /= sl; sz /= sl;
    double vx = sy * fz - sz * fy, vy = sz * fx - sx * fz, vz = sx * fy - sy * fx;
    float m[16] = {
        (float)sx, (float)vx, (float)-fx, 0,
        (float)sy, (float)vy, (float)-fy, 0,
        (float)sz, (float)vz, (float)-fz, 0,
        0, 0, 0, 1,
    };
    glMultMatrixf(m);
    glTranslatef((float)-ex, (float)-ey, (float)-ez);
}

// ---- state ---------------------------------------------------------------------------

static void reset_state(void) {
    gl.error = GL_NO_ERROR;
    gl.matrix_mode = GL_MODELVIEW;
    gl.modelview_top = gl.projection_top = 0;
    identity(gl.modelview[0]);
    identity(gl.projection[0]);
    identity(gl.texture_matrix);
    memset(&gl.fixed, 0, sizeof gl.fixed);
    set4(gl.fixed.scene_ambient, 0.2f, 0.2f, 0.2f, 1);
    set4(gl.fixed.mat_ambient, 0.2f, 0.2f, 0.2f, 1);
    set4(gl.fixed.mat_diffuse, 0.8f, 0.8f, 0.8f, 1);
    set4(gl.fixed.mat_specular, 0, 0, 0, 1);
    set4(gl.fixed.mat_emission, 0, 0, 0, 1);
    for (int i = 0; i < MAX_LIGHTS; i++) {
        fixed_light* l = &gl.fixed.lights[i];
        set4(l->position, 0, 0, 1, 0);
        set4(l->ambient, 0, 0, 0, 1);
        float on = i == 0 ? 1.0f : 0.0f;
        set4(l->diffuse, on, on, on, 1);
        set4(l->specular, on, on, on, 1);
    }
    gl.fixed.fog_end = 1;
    gl.fixed.fog_density = 1;
    gl.fog_mode = GL_EXP;
    gl.alpha_func = GL_ALWAYS;
    gl.lighting = gl.color_material = gl.texture_2d = gl.fog = gl.alpha_test = 0;
    gl.lights_on = 0;
    gl.depth_test = gl.blend = gl.cull_face = gl.scissor_test = gl.polygon_offset_fill = 0;
    gl.fixed_dirty = 1;
    gl.in_begin = 0;
    gl.nverts = 0;
    set4(gl.color, 1, 1, 1, 1);
    gl.normal[0] = 0; gl.normal[1] = 0; gl.normal[2] = 1;
    gl.uv[0] = gl.uv[1] = 0;
    memset(&gl.vertex, 0, sizeof gl.vertex);
    memset(&gl.color_array, 0, sizeof gl.color_array);
    memset(&gl.normal_array, 0, sizeof gl.normal_array);
    memset(&gl.uv_array, 0, sizeof gl.uv_array);
    gl.next_texture = gl.next_program = gl.program = 0;
    gl.unit = 0;
    memset(gl.bound, 0, sizeof gl.bound);
    gl.n_uniforms = 0;
}

GLenum glGetError(void) {
    GLenum e = gl.error;
    gl.error = GL_NO_ERROR;
    return e;
}

const GLubyte* glGetString(GLenum name) {
    switch (name) {
    case GL_VENDOR: return (const GLubyte*)"KOS";
    case GL_RENDERER: return (const GLubyte*)"kos-gl (the host's OpenGL)";
    case GL_VERSION: return (const GLubyte*)"1.5 KOS, with GLSL and HLSL";
    case GL_SHADING_LANGUAGE_VERSION: return (const GLubyte*)"4.10";
    }
    set_error(GL_INVALID_ENUM);
    return NULL;
}

// The flag behind a cap the device handles, or NULL.
static int* device_cap(GLenum cap) {
    switch (cap) {
    case GL_DEPTH_TEST: return &gl.depth_test;
    case GL_BLEND: return &gl.blend;
    case GL_CULL_FACE: return &gl.cull_face;
    case GL_SCISSOR_TEST: return &gl.scissor_test;
    case GL_POLYGON_OFFSET_FILL: return &gl.polygon_offset_fill;
    }
    return NULL;
}

// The flag behind a fixed-function cap, or NULL.
static int* fixed_cap(GLenum cap) {
    switch (cap) {
    case GL_LIGHTING: return &gl.lighting;
    case GL_COLOR_MATERIAL: return &gl.color_material;
    case GL_TEXTURE_2D: return &gl.texture_2d;
    case GL_FOG: return &gl.fog;
    case GL_ALPHA_TEST: return &gl.alpha_test;
    }
    return NULL;
}

static void set_cap(GLenum cap, int on) {
    int* flag;
    if ((flag = device_cap(cap))) {
        *flag = on;
        command(on ? CMD_ENABLE : CMD_DISABLE, 1, cap, 0, 0, 0);
    } else if ((flag = fixed_cap(cap))) {
        *flag = on;
        gl.fixed_dirty = 1;
    } else if (cap >= GL_LIGHT0 && cap < GL_LIGHT0 + MAX_LIGHTS) {
        uint32_t bit = 1u << (cap - GL_LIGHT0);
        gl.lights_on = on ? gl.lights_on | bit : gl.lights_on & ~bit;
        gl.fixed_dirty = 1;
    } else if (cap != GL_NORMALIZE && cap != GL_RESCALE_NORMAL) {
        set_error(GL_INVALID_ENUM);
    }
}

void glEnable(GLenum cap) { set_cap(cap, 1); }
void glDisable(GLenum cap) { set_cap(cap, 0); }

GLboolean glIsEnabled(GLenum cap) {
    int* flag = device_cap(cap);
    if (!flag) flag = fixed_cap(cap);
    if (flag) return *flag ? GL_TRUE : GL_FALSE;
    if (cap >= GL_LIGHT0 && cap < GL_LIGHT0 + MAX_LIGHTS) return (gl.lights_on >> (cap - GL_LIGHT0)) & 1;
    return GL_FALSE;
}

void glClearColor(GLclampf r, GLclampf g, GLclampf b, GLclampf a) {
    command(CMD_CLEAR_COLOR, 4, f2u(r), f2u(g), f2u(b), f2u(a));
}

void glClearDepthf(GLclampf depth) { command(CMD_CLEAR_DEPTH, 1, f2u(depth), 0, 0, 0); }
void glClearDepth(GLclampd depth) { glClearDepthf((float)depth); }
void glClear(GLbitfield mask) { command(CMD_CLEAR, 1, mask, 0, 0, 0); }

void glViewport(GLint x, GLint y, GLsizei w, GLsizei h) {
    if (w < 0 || h < 0) { set_error(GL_INVALID_VALUE); return; }
    command(CMD_VIEWPORT, 4, (uint32_t)x, (uint32_t)y, (uint32_t)w, (uint32_t)h);
}

void glScissor(GLint x, GLint y, GLsizei w, GLsizei h) {
    if (w < 0 || h < 0) { set_error(GL_INVALID_VALUE); return; }
    command(CMD_SCISSOR, 4, (uint32_t)x, (uint32_t)y, (uint32_t)w, (uint32_t)h);
}

void glBlendFunc(GLenum src, GLenum dst) { command(CMD_BLEND_FUNC, 2, src, dst, 0, 0); }
void glDepthFunc(GLenum func) { command(CMD_DEPTH_FUNC, 1, func, 0, 0, 0); }
void glDepthMask(GLboolean flag) { command(CMD_DEPTH_MASK, 1, flag != 0, 0, 0, 0); }
void glCullFace(GLenum mode) { command(CMD_CULL_FACE, 1, mode, 0, 0, 0); }
void glFrontFace(GLenum mode) { command(CMD_FRONT_FACE, 1, mode, 0, 0, 0); }

void glColorMask(GLboolean r, GLboolean g, GLboolean b, GLboolean a) {
    command(CMD_COLOR_MASK, 4, r != 0, g != 0, b != 0, a != 0);
}

void glPolygonOffset(GLfloat factor, GLfloat units) {
    command(CMD_POLYGON_OFFSET, 2, f2u(factor), f2u(units), 0, 0);
}

void glAlphaFunc(GLenum func, GLclampf ref) {
    if (func < GL_NEVER || func > GL_ALWAYS) { set_error(GL_INVALID_ENUM); return; }
    gl.alpha_func = func;
    gl.fixed.alpha_ref = ref;
    gl.fixed_dirty = 1;
}

void glShadeModel(GLenum mode) { (void)mode; }
void glHint(GLenum target, GLenum mode) { (void)target; (void)mode; }
void glFlush(void) { flush(0); }
void glFinish(void) { flush(0); }

// ---- lights, materials and fog ---------------------------------------------------------

void glLightfv(GLenum light, GLenum pname, const GLfloat* p) {
    if (light < GL_LIGHT0 || light >= GL_LIGHT0 + MAX_LIGHTS) { set_error(GL_INVALID_ENUM); return; }
    fixed_light* l = &gl.fixed.lights[light - GL_LIGHT0];
    switch (pname) {
    case GL_AMBIENT: memcpy(l->ambient, p, 4 * sizeof(float)); break;
    case GL_DIFFUSE: memcpy(l->diffuse, p, 4 * sizeof(float)); break;
    case GL_SPECULAR: memcpy(l->specular, p, 4 * sizeof(float)); break;
    case GL_POSITION: {
        // Kept in eye space, through the modelview matrix as it is now.
        const float* m = gl.modelview[gl.modelview_top];
        for (int row = 0; row < 4; row++)
            l->position[row] = m[row] * p[0] + m[4 + row] * p[1] + m[8 + row] * p[2] + m[12 + row] * p[3];
        break;
    }
    default: set_error(GL_INVALID_ENUM); return;
    }
    gl.fixed_dirty = 1;
}

void glLightModelfv(GLenum pname, const GLfloat* p) {
    if (pname != GL_LIGHT_MODEL_AMBIENT) { set_error(GL_INVALID_ENUM); return; }
    memcpy(gl.fixed.scene_ambient, p, 4 * sizeof(float));
    gl.fixed_dirty = 1;
}

void glMaterialfv(GLenum face, GLenum pname, const GLfloat* p) {
    (void)face;
    switch (pname) {
    case GL_AMBIENT: memcpy(gl.fixed.mat_ambient, p, 4 * sizeof(float)); break;
    case GL_DIFFUSE: memcpy(gl.fixed.mat_diffuse, p, 4 * sizeof(float)); break;
    case GL_AMBIENT_AND_DIFFUSE:
        memcpy(gl.fixed.mat_ambient, p, 4 * sizeof(float));
        memcpy(gl.fixed.mat_diffuse, p, 4 * sizeof(float));
        break;
    case GL_SPECULAR: memcpy(gl.fixed.mat_specular, p, 4 * sizeof(float)); break;
    case GL_EMISSION: memcpy(gl.fixed.mat_emission, p, 4 * sizeof(float)); break;
    case GL_SHININESS: gl.fixed.shininess = p[0]; break;
    default: set_error(GL_INVALID_ENUM); return;
    }
    gl.fixed_dirty = 1;
}

void glMaterialf(GLenum face, GLenum pname, GLfloat param) {
    if (pname != GL_SHININESS) { set_error(GL_INVALID_ENUM); return; }
    glMaterialfv(face, pname, &param);
}

void glColorMaterial(GLenum face, GLenum mode) { (void)face; (void)mode; }

void glFogf(GLenum pname, GLfloat param) {
    switch (pname) {
    case GL_FOG_MODE:
        if (param != GL_LINEAR && param != GL_EXP && param != GL_EXP2) { set_error(GL_INVALID_ENUM); return; }
        gl.fog_mode = (GLenum)param;
        break;
    case GL_FOG_START: gl.fixed.fog_start = param; break;
    case GL_FOG_END: gl.fixed.fog_end = param; break;
    case GL_FOG_DENSITY: gl.fixed.fog_density = param; break;
    default: set_error(GL_INVALID_ENUM); return;
    }
    gl.fixed_dirty = 1;
}

void glFogi(GLenum pname, GLint param) { glFogf(pname, (float)param); }

void glFogfv(GLenum pname, const GLfloat* p) {
    if (pname == GL_FOG_COLOR) {
        memcpy(gl.fixed.fog_color, p, 4 * sizeof(float));
        gl.fixed_dirty = 1;
    } else {
        glFogf(pname, p[0]);
    }
}

// Sends the fixed-function state (and the matrices, which programs get
// too) if it changed since the last draw.
static void send_fixed_state(void) {
    if (!gl.fixed_dirty) return;
    fixed_state* s = &gl.fixed;
    memcpy(s->modelview, gl.modelview[gl.modelview_top], sizeof s->modelview);
    memcpy(s->projection, gl.projection[gl.projection_top], sizeof s->projection);
    s->flags = (gl.lighting ? FIXED_LIGHTING : 0) | (gl.color_material ? FIXED_COLOR_MATERIAL : 0) |
               (gl.texture_2d ? FIXED_TEXTURE : 0) | (gl.lights_on * FIXED_LIGHT0);
    s->fog_mode = !gl.fog ? 0 : gl.fog_mode == GL_LINEAR ? 1 : gl.fog_mode == GL_EXP ? 2 : 3;
    s->alpha_func = gl.alpha_test ? gl.alpha_func - GL_NEVER : GL_ALWAYS - GL_NEVER;
    uint32_t* w = begin_command(CMD_FIXED_STATE, 2 + sizeof *s / 4);
    if (!w) return;
    memcpy(w, s, sizeof *s);
    gl.fixed_dirty = 0;
}

// ---- drawing ----------------------------------------------------------------------------

static int is_list(GLenum mode) {
    return mode == GL_POINTS || mode == GL_LINES || mode == GL_TRIANGLES;
}

// Vertex `i` of `src`, appended to `dst`.
static float* put(float* dst, const float* src, size_t i) {
    memcpy(dst, src + i * VERTEX_FLOATS, VERTEX_FLOATS * sizeof(float));
    return dst + VERTEX_FLOATS;
}

// The same shapes as points, lines or triangles (malloc'd), in *mode and
// *count. Quads, quad strips and polygons always need this; the device
// draws the rest as they are, unless they're too big for one command.
static float* to_list(GLenum* mode, const float* v, size_t* count) {
    size_t n = *count, out_n = 0;
    GLenum out_mode = GL_TRIANGLES;
    switch (*mode) {
    case GL_TRIANGLE_STRIP: case GL_TRIANGLE_FAN: case GL_POLYGON:
        out_n = n >= 3 ? (n - 2) * 3 : 0; break;
    case GL_QUADS: out_n = n / 4 * 6; break;
    case GL_QUAD_STRIP: out_n = n >= 4 ? (n - 2) / 2 * 6 : 0; break;
    case GL_LINE_STRIP: out_mode = GL_LINES; out_n = n >= 2 ? (n - 1) * 2 : 0; break;
    case GL_LINE_LOOP: out_mode = GL_LINES; out_n = n >= 2 ? n * 2 : 0; break;
    default: out_mode = *mode; out_n = n; break;
    }
    float* out = malloc((out_n ? out_n : 1) * VERTEX_FLOATS * sizeof(float));
    if (!out) return NULL;
    float* p = out;
    switch (*mode) {
    case GL_TRIANGLE_STRIP:
        for (size_t i = 0; i + 2 < n; i++) {
            // Every other triangle is turned around, to keep the winding.
            p = put(p, v, i % 2 ? i + 1 : i);
            p = put(p, v, i % 2 ? i : i + 1);
            p = put(p, v, i + 2);
        }
        break;
    case GL_TRIANGLE_FAN: case GL_POLYGON:
        for (size_t i = 1; i + 1 < n; i++) {
            p = put(p, v, 0); p = put(p, v, i); p = put(p, v, i + 1);
        }
        break;
    case GL_QUADS:
        for (size_t i = 0; i + 3 < n; i += 4) {
            p = put(p, v, i); p = put(p, v, i + 1); p = put(p, v, i + 2);
            p = put(p, v, i); p = put(p, v, i + 2); p = put(p, v, i + 3);
        }
        break;
    case GL_QUAD_STRIP:
        for (size_t i = 0; i + 3 < n; i += 2) {
            p = put(p, v, i); p = put(p, v, i + 1); p = put(p, v, i + 3);
            p = put(p, v, i); p = put(p, v, i + 3); p = put(p, v, i + 2);
        }
        break;
    case GL_LINE_STRIP: case GL_LINE_LOOP:
        for (size_t i = 0; i + 1 < n; i++) { p = put(p, v, i); p = put(p, v, i + 1); }
        if (*mode == GL_LINE_LOOP && n >= 2) { p = put(p, v, n - 1); p = put(p, v, 0); }
        break;
    default:
        memcpy(out, v, n * VERTEX_FLOATS * sizeof(float));
        break;
    }
    *mode = out_mode;
    *count = out_n;
    return out;
}

static void draw_command(GLenum mode, const float* v, size_t count) {
    uint32_t* w = begin_command(CMD_DRAW, 4 + count * VERTEX_FLOATS);
    if (!w) return;
    w[0] = mode;
    w[1] = (uint32_t)count;
    memcpy(w + 2, v, count * VERTEX_FLOATS * sizeof(float));
}

// Draws `count` vertices laid out VERTEX_FLOATS apiece.
static void draw(GLenum mode, const float* v, size_t count) {
    if (mode > GL_POLYGON) { set_error(GL_INVALID_ENUM); return; }
    if (!gl.ctx || count == 0) return;
    send_fixed_state();
    float* list = NULL;
    if (mode > GL_TRIANGLE_FAN || (count > MAX_DRAW && !is_list(mode))) {
        list = to_list(&mode, v, &count);
        if (!list) { set_error(GL_OUT_OF_MEMORY); return; }
        v = list;
    }
    // A list splits anywhere on a whole shape.
    size_t per = mode == GL_TRIANGLES ? 3 : mode == GL_LINES ? 2 : 1;
    size_t chunk = is_list(mode) ? MAX_DRAW / per * per : count;
    for (size_t start = 0; start < count; start += chunk) {
        size_t n = count - start < chunk ? count - start : chunk;
        draw_command(mode, v + start * VERTEX_FLOATS, n);
    }
    free(list);
}

void glBegin(GLenum mode) {
    if (gl.in_begin) { set_error(GL_INVALID_OPERATION); return; }
    if (mode > GL_POLYGON) { set_error(GL_INVALID_ENUM); return; }
    gl.in_begin = 1;
    gl.begin_mode = mode;
    gl.nverts = 0;
}

void glEnd(void) {
    if (!gl.in_begin) { set_error(GL_INVALID_OPERATION); return; }
    gl.in_begin = 0;
    draw(gl.begin_mode, gl.verts, gl.nverts);
}

void glVertex4f(GLfloat x, GLfloat y, GLfloat z, GLfloat w) {
    if (!gl.in_begin) return;
    if (gl.nverts == gl.verts_cap) {
        size_t cap = gl.verts_cap ? gl.verts_cap * 2 : 256;
        float* verts = realloc(gl.verts, cap * VERTEX_FLOATS * sizeof(float));
        if (!verts) { set_error(GL_OUT_OF_MEMORY); return; }
        gl.verts = verts;
        gl.verts_cap = cap;
    }
    float* v = gl.verts + gl.nverts++ * VERTEX_FLOATS;
    set4(v, x, y, z, w);
    memcpy(v + 4, gl.color, 4 * sizeof(float));
    memcpy(v + 8, gl.normal, 3 * sizeof(float));
    memcpy(v + 11, gl.uv, 2 * sizeof(float));
}

void glVertex2f(GLfloat x, GLfloat y) { glVertex4f(x, y, 0, 1); }
void glVertex3f(GLfloat x, GLfloat y, GLfloat z) { glVertex4f(x, y, z, 1); }
void glVertex2fv(const GLfloat* v) { glVertex4f(v[0], v[1], 0, 1); }
void glVertex3fv(const GLfloat* v) { glVertex4f(v[0], v[1], v[2], 1); }

void glColor4f(GLfloat r, GLfloat g, GLfloat b, GLfloat a) { set4(gl.color, r, g, b, a); }
void glColor3f(GLfloat r, GLfloat g, GLfloat b) { set4(gl.color, r, g, b, 1); }
void glColor3fv(const GLfloat* v) { set4(gl.color, v[0], v[1], v[2], 1); }
void glColor4fv(const GLfloat* v) { set4(gl.color, v[0], v[1], v[2], v[3]); }
void glColor4ub(GLubyte r, GLubyte g, GLubyte b, GLubyte a) {
    set4(gl.color, r / 255.0f, g / 255.0f, b / 255.0f, a / 255.0f);
}
void glColor3ub(GLubyte r, GLubyte g, GLubyte b) { glColor4ub(r, g, b, 255); }

void glNormal3f(GLfloat x, GLfloat y, GLfloat z) {
    gl.normal[0] = x; gl.normal[1] = y; gl.normal[2] = z;
}
void glNormal3fv(const GLfloat* v) { glNormal3f(v[0], v[1], v[2]); }

void glTexCoord2f(GLfloat s, GLfloat t) { gl.uv[0] = s; gl.uv[1] = t; }
void glTexCoord2fv(const GLfloat* v) { glTexCoord2f(v[0], v[1]); }

// ---- vertex arrays --------------------------------------------------------------------------

static vertex_array* client_array(GLenum array) {
    switch (array) {
    case GL_VERTEX_ARRAY: return &gl.vertex;
    case GL_COLOR_ARRAY: return &gl.color_array;
    case GL_NORMAL_ARRAY: return &gl.normal_array;
    case GL_TEXTURE_COORD_ARRAY: return &gl.uv_array;
    }
    return NULL;
}

void glEnableClientState(GLenum array) {
    vertex_array* a = client_array(array);
    if (a) a->enabled = 1; else set_error(GL_INVALID_ENUM);
}

void glDisableClientState(GLenum array) {
    vertex_array* a = client_array(array);
    if (a) a->enabled = 0; else set_error(GL_INVALID_ENUM);
}

static void set_array(vertex_array* a, GLint size, GLenum type, GLsizei stride, const GLvoid* p) {
    a->size = size;
    a->type = type;
    a->stride = stride;
    a->pointer = p;
}

void glVertexPointer(GLint size, GLenum type, GLsizei stride, const GLvoid* p) {
    if (size < 2 || size > 4 || type != GL_FLOAT) { set_error(size < 2 || size > 4 ? GL_INVALID_VALUE : GL_INVALID_ENUM); return; }
    set_array(&gl.vertex, size, type, stride, p);
}

void glColorPointer(GLint size, GLenum type, GLsizei stride, const GLvoid* p) {
    if (size < 3 || size > 4 || (type != GL_FLOAT && type != GL_UNSIGNED_BYTE)) { set_error(GL_INVALID_ENUM); return; }
    set_array(&gl.color_array, size, type, stride, p);
}

void glNormalPointer(GLenum type, GLsizei stride, const GLvoid* p) {
    if (type != GL_FLOAT) { set_error(GL_INVALID_ENUM); return; }
    set_array(&gl.normal_array, 3, type, stride, p);
}

void glTexCoordPointer(GLint size, GLenum type, GLsizei stride, const GLvoid* p) {
    if (size != 2 || type != GL_FLOAT) { set_error(GL_INVALID_ENUM); return; }
    set_array(&gl.uv_array, size, type, stride, p);
}

// Element `i` of an array, as bytes.
static const uint8_t* element(const vertex_array* a, size_t i) {
    size_t size = a->size * (a->type == GL_UNSIGNED_BYTE ? 1 : 4);
    return (const uint8_t*)a->pointer + i * (a->stride ? (size_t)a->stride : size);
}

// Vertex `i` of the enabled arrays, into v (VERTEX_FLOATS floats).
static void fetch(float* v, size_t i) {
    const float* p = (const float*)element(&gl.vertex, i);
    set4(v, p[0], p[1], gl.vertex.size > 2 ? p[2] : 0, gl.vertex.size > 3 ? p[3] : 1);
    if (gl.color_array.enabled) {
        const uint8_t* c = element(&gl.color_array, i);
        int n = gl.color_array.size;
        if (gl.color_array.type == GL_UNSIGNED_BYTE)
            set4(v + 4, c[0] / 255.0f, c[1] / 255.0f, c[2] / 255.0f, n > 3 ? c[3] / 255.0f : 1);
        else {
            const float* f = (const float*)c;
            set4(v + 4, f[0], f[1], f[2], n > 3 ? f[3] : 1);
        }
    } else {
        memcpy(v + 4, gl.color, 4 * sizeof(float));
    }
    memcpy(v + 8, gl.normal_array.enabled ? (const float*)element(&gl.normal_array, i) : gl.normal, 3 * sizeof(float));
    memcpy(v + 11, gl.uv_array.enabled ? (const float*)element(&gl.uv_array, i) : gl.uv, 2 * sizeof(float));
}

// Draws the arrays' vertices at `indices` (or first.. if indices is NULL).
static void draw_arrays(GLenum mode, size_t first, size_t count, GLenum index_type, const void* indices) {
    if (!gl.vertex.enabled || !gl.vertex.pointer || count == 0) return;
    float* v = malloc(count * VERTEX_FLOATS * sizeof(float));
    if (!v) { set_error(GL_OUT_OF_MEMORY); return; }
    for (size_t i = 0; i < count; i++) {
        size_t at = first + i;
        if (indices) {
            at = index_type == GL_UNSIGNED_BYTE ? ((const uint8_t*)indices)[i]
               : index_type == GL_UNSIGNED_SHORT ? ((const uint16_t*)indices)[i]
               : ((const uint32_t*)indices)[i];
        }
        fetch(v + i * VERTEX_FLOATS, at);
    }
    draw(mode, v, count);
    free(v);
}

// ---- buffer objects and vertex array objects -----------------------------------------------

static vertex_array_object* current_vao(void) {
    return gl.vao ? &gl.vaos[gl.vao - 1] : &gl.default_vao;
}

static buffer_object* buffer(GLuint id) {
    return id && id <= gl.n_buffers && gl.buffers[id - 1].used ? &gl.buffers[id - 1] : NULL;
}

// Hands out unused slots of a table of objects of `size` bytes, growing it.
static int gen_objects(void** table, GLuint* n, size_t size, GLsizei count, GLuint* ids) {
    if (count < 0) { set_error(GL_INVALID_VALUE); return -1; }
    for (GLsizei k = 0; k < count; k++) {
        GLuint id = 0;
        for (GLuint i = 0; i < *n && !id; i++)
            if (!*(int*)((uint8_t*)*table + i * size)) id = i + 1;
        if (!id) {
            void* grown = realloc(*table, (*n + 1) * size);
            if (!grown) { set_error(GL_OUT_OF_MEMORY); return -1; }
            *table = grown;
            id = ++*n;
        }
        uint8_t* slot = (uint8_t*)*table + (id - 1) * size;
        memset(slot, 0, size);
        *(int*)slot = 1;
        ids[k] = id;
    }
    return 0;
}

void glGenBuffers(GLsizei n, GLuint* buffers) {
    gen_objects((void**)&gl.buffers, &gl.n_buffers, sizeof(buffer_object), n, buffers);
}

void glDeleteBuffers(GLsizei n, const GLuint* buffers) {
    for (GLsizei k = 0; k < n; k++) {
        buffer_object* b = buffer(buffers[k]);
        if (!b) continue;
        free(b->data);
        memset(b, 0, sizeof *b);
        if (gl.array_buffer == buffers[k]) gl.array_buffer = 0;
        if (current_vao()->elements == buffers[k]) current_vao()->elements = 0;
    }
}

static GLuint* binding(GLenum target) {
    switch (target) {
    case GL_ARRAY_BUFFER: return &gl.array_buffer;
    case GL_ELEMENT_ARRAY_BUFFER: return &current_vao()->elements;
    }
    return NULL;
}

void glBindBuffer(GLenum target, GLuint id) {
    GLuint* bound = binding(target);
    if (!bound) { set_error(GL_INVALID_ENUM); return; }
    if (id && !buffer(id)) { set_error(GL_INVALID_OPERATION); return; }
    *bound = id;
}

void glBufferData(GLenum target, GLsizeiptr size, const GLvoid* data, GLenum usage) {
    (void)usage;
    GLuint* bound = binding(target);
    if (!bound) { set_error(GL_INVALID_ENUM); return; }
    buffer_object* b = buffer(*bound);
    if (!b || size < 0) { set_error(b ? GL_INVALID_VALUE : GL_INVALID_OPERATION); return; }
    uint8_t* copy = malloc(size ? (size_t)size : 1);
    if (!copy) { set_error(GL_OUT_OF_MEMORY); return; }
    if (data) memcpy(copy, data, (size_t)size); else memset(copy, 0, (size_t)size);
    free(b->data);
    b->data = copy;
    b->size = (size_t)size;
}

void glBufferSubData(GLenum target, GLintptr offset, GLsizeiptr size, const GLvoid* data) {
    GLuint* bound = binding(target);
    if (!bound) { set_error(GL_INVALID_ENUM); return; }
    buffer_object* b = buffer(*bound);
    if (!b) { set_error(GL_INVALID_OPERATION); return; }
    if (offset < 0 || size < 0 || (size_t)offset + (size_t)size > b->size) { set_error(GL_INVALID_VALUE); return; }
    memcpy(b->data + offset, data, (size_t)size);
}

void glGenVertexArrays(GLsizei n, GLuint* arrays) {
    gen_objects((void**)&gl.vaos, &gl.n_vaos, sizeof(vertex_array_object), n, arrays);
}

void glDeleteVertexArrays(GLsizei n, const GLuint* arrays) {
    for (GLsizei k = 0; k < n; k++) {
        GLuint id = arrays[k];
        if (!id || id > gl.n_vaos || !gl.vaos[id - 1].used) continue;
        gl.vaos[id - 1].used = 0;
        if (gl.vao == id) gl.vao = 0;
    }
}

void glBindVertexArray(GLuint array) {
    if (array && (array > gl.n_vaos || !gl.vaos[array - 1].used)) { set_error(GL_INVALID_OPERATION); return; }
    gl.vao = array;
}

static size_t type_size(GLenum type) {
    switch (type) {
    case GL_BYTE: case GL_UNSIGNED_BYTE: return 1;
    case GL_SHORT: case GL_UNSIGNED_SHORT: return 2;
    case GL_INT: case GL_UNSIGNED_INT: case GL_FLOAT: return 4;
    }
    return 0;
}

void glVertexAttribPointer(GLuint index, GLint size, GLenum type, GLboolean normalized, GLsizei stride,
                           const GLvoid* pointer) {
    if (index >= MAX_ATTRIBS || size < 1 || size > 4 || stride < 0) { set_error(GL_INVALID_VALUE); return; }
    if (!type_size(type)) { set_error(GL_INVALID_ENUM); return; }
    attrib* a = &current_vao()->attribs[index];
    a->size = size;
    a->type = type;
    a->normalized = normalized;
    a->stride = stride;
    a->buffer = gl.array_buffer;
    a->pointer = pointer;
}

void glEnableVertexAttribArray(GLuint index) {
    if (index >= MAX_ATTRIBS) { set_error(GL_INVALID_VALUE); return; }
    current_vao()->attribs[index].enabled = 1;
}

void glDisableVertexAttribArray(GLuint index) {
    if (index >= MAX_ATTRIBS) { set_error(GL_INVALID_VALUE); return; }
    current_vao()->attribs[index].enabled = 0;
}

void glPolygonMode(GLenum face, GLenum mode) {
    if (face != GL_FRONT_AND_BACK || (mode != GL_POINT && mode != GL_LINE && mode != GL_FILL)) {
        set_error(GL_INVALID_ENUM);
        return;
    }
    command(CMD_POLYGON_MODE, 1, mode, 0, 0, 0);
}

static int attribs_enabled(void) {
    for (int i = 0; i < MAX_ATTRIBS; i++)
        if (current_vao()->attribs[i].enabled) return 1;
    return 0;
}

// One component, as a float (normalized as OpenGL does).
static float component(const uint8_t* p, GLenum type, GLboolean normalized) {
    float f;
    int8_t b; int16_t s; uint16_t us; int32_t i; uint32_t u;
    switch (type) {
    case GL_FLOAT: memcpy(&f, p, 4); return f;
    case GL_UNSIGNED_BYTE: return normalized ? p[0] / 255.0f : p[0];
    case GL_BYTE: memcpy(&b, p, 1); return normalized ? (b < -127 ? -1 : b / 127.0f) : b;
    case GL_UNSIGNED_SHORT: memcpy(&us, p, 2); return normalized ? us / 65535.0f : us;
    case GL_SHORT: memcpy(&s, p, 2); return normalized ? (s < -32767 ? -1 : s / 32767.0f) : s;
    case GL_UNSIGNED_INT: memcpy(&u, p, 4); return normalized ? (float)(u / 4294967295.0) : (float)u;
    case GL_INT: memcpy(&i, p, 4); return normalized ? (float)(i < -2147483647 ? -1 : i / 2147483647.0) : (float)i;
    }
    return 0;
}

// Where attribute a's data starts, and how many bytes of it there are
// (SIZE_MAX for the program's own memory). NULL if it has none.
static const uint8_t* attrib_data(const attrib* a, size_t* size) {
    if (!a->buffer) {
        *size = SIZE_MAX;
        return a->pointer;
    }
    buffer_object* b = buffer(a->buffer);
    if (!b || (size_t)a->pointer > b->size) return NULL;
    *size = b->size - (size_t)a->pointer;
    return b->data + (size_t)a->pointer;
}

// glDrawArrays and glDrawElements with glVertexAttribPointer's arrays:
// each draw sends the enabled locations' values, as floats, to the
// locations the shaders read.
static void draw_attribs(GLenum mode, size_t first, size_t count, GLenum index_type, const uint8_t* indices) {
    if (mode > GL_TRIANGLE_FAN) { set_error(GL_INVALID_ENUM); return; }
    if (!gl.ctx || count == 0) return;
    vertex_array_object* vao = current_vao();
    const uint8_t* data[MAX_ATTRIBS];
    size_t have[MAX_ATTRIBS], stride[MAX_ATTRIBS];
    uint32_t layout = 0, floats = 0;
    for (int i = 0; i < MAX_ATTRIBS; i++) {
        const attrib* a = &vao->attribs[i];
        if (!a->enabled) continue;
        data[i] = attrib_data(a, &have[i]);
        if (!data[i]) { set_error(GL_INVALID_OPERATION); return; }
        stride[i] = a->stride ? (size_t)a->stride : a->size * type_size(a->type);
        layout |= (uint32_t)a->size << (i * 4);
        floats += (uint32_t)a->size;
    }

    // Which vertex each one sent is; strips, loops and fans too big for
    // one command become lists, which split anywhere on a whole shape.
    size_t max = (MAX_COMMAND - 5) / floats;
    size_t n = count;
    int list = mode == GL_POINTS || mode == GL_LINES || mode == GL_TRIANGLES;
    if (!list && count > max) {
        n = mode == GL_LINE_STRIP ? (count - 1) * 2 : mode == GL_LINE_LOOP ? count * 2 : (count - 2) * 3;
    }
    size_t* at = malloc(n * sizeof(size_t));
    if (!at) { set_error(GL_OUT_OF_MEMORY); return; }
    size_t top = 0;
    for (size_t k = 0; k < count; k++) {
        size_t i = first + k;
        if (indices) {
            i = index_type == GL_UNSIGNED_BYTE ? indices[k]
              : index_type == GL_UNSIGNED_SHORT ? ((const uint16_t*)indices)[k]
              : ((const uint32_t*)indices)[k];
        }
        if (i > top) top = i;
        at[k] = i;
    }
    if (n != count) {
        size_t* seq = malloc(count * sizeof(size_t));
        if (!seq) { free(at); set_error(GL_OUT_OF_MEMORY); return; }
        memcpy(seq, at, count * sizeof(size_t));
        size_t m = 0;
        if (mode == GL_LINE_STRIP || mode == GL_LINE_LOOP) {
            for (size_t k = 0; k + 1 < count; k++) { at[m++] = seq[k]; at[m++] = seq[k + 1]; }
            if (mode == GL_LINE_LOOP) { at[m++] = seq[count - 1]; at[m++] = seq[0]; }
            mode = GL_LINES;
        } else {
            for (size_t k = 0; k + 2 < count; k++) {
                int odd = mode == GL_TRIANGLE_STRIP && (k & 1);
                at[m++] = mode == GL_TRIANGLE_FAN ? seq[0] : seq[odd ? k + 1 : k];
                at[m++] = mode == GL_TRIANGLE_FAN ? seq[k + 1] : seq[odd ? k : k + 1];
                at[m++] = seq[k + 2];
            }
            mode = GL_TRIANGLES;
        }
        n = m;
        free(seq);
        list = 1;
    }
    // Every vertex used must be inside its buffers.
    for (int i = 0; i < MAX_ATTRIBS; i++) {
        const attrib* a = &vao->attribs[i];
        if (a->enabled && have[i] != SIZE_MAX && top * stride[i] + a->size * type_size(a->type) > have[i]) {
            free(at);
            set_error(GL_INVALID_OPERATION);
            return;
        }
    }

    send_fixed_state();
    size_t per = mode == GL_TRIANGLES ? 3 : mode == GL_LINES ? 2 : 1;
    size_t chunk = list ? max / per * per : n;
    for (size_t start = 0; start < n; start += chunk) {
        size_t len = n - start < chunk ? n - start : chunk;
        uint32_t* w = begin_command(CMD_DRAW_ATTRIBS, 5 + len * floats);
        if (!w) break;
        w[0] = mode;
        w[1] = (uint32_t)len;
        w[2] = layout;
        float* out = (float*)(w + 3);
        for (size_t k = start; k < start + len; k++)
            for (int i = 0; i < MAX_ATTRIBS; i++) {
                const attrib* a = &vao->attribs[i];
                if (!a->enabled) continue;
                const uint8_t* p = data[i] + at[k] * stride[i];
                for (int c = 0; c < a->size; c++)
                    *out++ = component(p + c * type_size(a->type), a->type, a->normalized);
            }
    }
    free(at);
}

void glDrawArrays(GLenum mode, GLint first, GLsizei count) {
    if (first < 0 || count < 0) { set_error(GL_INVALID_VALUE); return; }
    if (attribs_enabled()) draw_attribs(mode, (size_t)first, (size_t)count, 0, NULL);
    else draw_arrays(mode, (size_t)first, (size_t)count, 0, NULL);
}

// With a GL_ELEMENT_ARRAY_BUFFER bound, indices is an offset into it.
void glDrawElements(GLenum mode, GLsizei count, GLenum type, const GLvoid* indices) {
    if (count < 0) { set_error(GL_INVALID_VALUE); return; }
    if (type != GL_UNSIGNED_BYTE && type != GL_UNSIGNED_SHORT && type != GL_UNSIGNED_INT) {
        set_error(GL_INVALID_ENUM);
        return;
    }
    GLuint elements = current_vao()->elements;
    if (elements) {
        buffer_object* b = buffer(elements);
        size_t offset = (size_t)indices;
        if (!b || offset > b->size || (size_t)count * type_size(type) > b->size - offset) {
            set_error(GL_INVALID_OPERATION);
            return;
        }
        indices = b->data + offset;
    }
    if (attribs_enabled()) draw_attribs(mode, 0, (size_t)count, type, indices);
    else draw_arrays(mode, 0, (size_t)count, type, indices);
}

// ---- textures -------------------------------------------------------------------------------

void glGenTextures(GLsizei n, GLuint* textures) {
    for (GLsizei i = 0; i < n; i++) textures[i] = ++gl.next_texture;
}

void glDeleteTextures(GLsizei n, const GLuint* textures) {
    for (GLsizei i = 0; i < n; i++) {
        if (!textures[i]) continue;
        for (int u = 0; u < TEXTURE_UNITS; u++)
            if (gl.bound[u] == textures[i]) gl.bound[u] = 0;
        command(CMD_TEX_DELETE, 1, textures[i], 0, 0, 0);
    }
}

void glActiveTexture(GLenum unit) {
    if (unit < GL_TEXTURE0 || unit >= GL_TEXTURE0 + TEXTURE_UNITS) { set_error(GL_INVALID_ENUM); return; }
    gl.unit = unit - GL_TEXTURE0;
}

void glBindTexture(GLenum target, GLuint texture) {
    if (target != GL_TEXTURE_2D) { set_error(GL_INVALID_ENUM); return; }
    gl.bound[gl.unit] = texture;
    command(CMD_BIND_TEXTURE, 2, (uint32_t)gl.unit, texture, 0, 0);
}

static uint32_t texel(GLenum format, GLenum type, const uint8_t* pixels, size_t i) {
    if (type == GL_UNSIGNED_INT_8_8_8_8_REV) return ((const uint32_t*)pixels)[i];
    if (format == GL_RGB) {
        const uint8_t* p = pixels + i * 3;
        return 0xff000000u | (uint32_t)p[0] << 16 | (uint32_t)p[1] << 8 | p[2];
    }
    const uint8_t* p = pixels + i * 4;
    return (uint32_t)p[3] << 24 | (uint32_t)p[0] << 16 | (uint32_t)p[1] << 8 | p[2];
}

void glTexImage2D(GLenum target, GLint level, GLint internal_format, GLsizei width, GLsizei height,
                  GLint border, GLenum format, GLenum type, const GLvoid* pixels) {
    (void)internal_format;
    if (target != GL_TEXTURE_2D) { set_error(GL_INVALID_ENUM); return; }
    // Only level 0: glGenerateMipmap makes the rest.
    if (level != 0) return;
    if (width <= 0 || height <= 0 || width > 4096 || height > 4096 || border != 0) { set_error(GL_INVALID_VALUE); return; }
    int ok = (type == GL_UNSIGNED_BYTE && (format == GL_RGBA || format == GL_RGB)) ||
             (type == GL_UNSIGNED_INT_8_8_8_8_REV && format == GL_BGRA);
    if (!ok) { set_error(GL_INVALID_ENUM); return; }
    GLuint id = gl.bound[gl.unit];
    if (!id) { set_error(GL_INVALID_OPERATION); return; }
    // In bands of rows, so no one command gets too big.
    size_t w = (size_t)width, h = (size_t)height;
    size_t band = (MAX_COMMAND - 7) / w;
    for (size_t y0 = 0; y0 < h; y0 += band) {
        size_t rows = h - y0 < band ? h - y0 : band;
        uint32_t* c = begin_command(CMD_TEX_IMAGE, 7 + w * rows);
        if (!c) return;
        c[0] = id; c[1] = (uint32_t)w; c[2] = (uint32_t)h; c[3] = (uint32_t)y0; c[4] = (uint32_t)rows;
        for (size_t i = 0; i < w * rows; i++)
            c[5 + i] = pixels ? texel(format, type, pixels, y0 * w + i) : 0;
    }
}

void glTexParameteri(GLenum target, GLenum pname, GLint value) {
    if (target != GL_TEXTURE_2D) { set_error(GL_INVALID_ENUM); return; }
    if (!gl.bound[gl.unit]) { set_error(GL_INVALID_OPERATION); return; }
    command(CMD_TEX_PARAM, 3, gl.bound[gl.unit], pname, (uint32_t)value, 0);
}

void glGenerateMipmap(GLenum target) {
    if (target != GL_TEXTURE_2D) { set_error(GL_INVALID_ENUM); return; }
    if (!gl.bound[gl.unit]) { set_error(GL_INVALID_OPERATION); return; }
    command(CMD_GENERATE_MIPMAP, 1, gl.bound[gl.unit], 0, 0, 0);
}

// ---- programs -------------------------------------------------------------------------------

GLuint kgl_hlsl_program(const char* vs_source, const char* vs_entry, const char* ps_source, const char* ps_entry) {
    if (!gl.ctx) { say("no context: call kgl_open first"); return 0; }
    // What's been drawn so far goes first.
    if (flush(0) < 0) return 0;
    // kos-hlslc's request: the entry points, then each source after its length.
    size_t vs_len = strlen(vs_source), ps_len = strlen(ps_source);
    size_t cap = strlen(vs_entry) + strlen(ps_entry) + vs_len + ps_len + 64;
    char* req = malloc(cap);
    if (!req) { say("out of memory"); return 0; }
    int head = snprintf(req, cap, "%s\n%s\n%lu\n", vs_entry, ps_entry, (unsigned long)vs_len);
    size_t len = (size_t)head;
    memcpy(req + len, vs_source, vs_len);
    len += vs_len;
    len += (size_t)snprintf(req + len, cap - len, "%lu\n", (unsigned long)ps_len);
    memcpy(req + len, ps_source, ps_len);
    len += ps_len;

    GLuint program = ++gl.next_program;
    gl.message[0] = '\0';
    uint64_t r = gpu_svc(GPU_HLSL, gl.ctx | (uint64_t)program << 32, (uint64_t)req, len,
                         (uint64_t)gl.message, sizeof gl.message);
    free(req);
    if (r == (uint64_t)-1) {
        say(last_error());
        return 0;
    }
    return r == 0 ? program : 0;
}

// Sends the device a GLSL request: program `program` from vs and fs, or
// with program 0 only a check of the ones that aren't empty. Returns 1 if
// they compiled, or 0 with the messages in gl.message.
static int glsl_request(GLuint program, const char* vs, const char* fs) {
    if (!gl.ctx) { say("no context: call kgl_open first"); return 0; }
    // What's been drawn so far goes first.
    if (flush(0) < 0) return 0;
    size_t vs_len = strlen(vs), fs_len = strlen(fs);
    size_t cap = vs_len + fs_len + 64;
    char* req = malloc(cap);
    if (!req) { say("out of memory"); return 0; }
    size_t len = (size_t)snprintf(req, cap, "%lu\n", (unsigned long)vs_len);
    memcpy(req + len, vs, vs_len);
    len += vs_len;
    len += (size_t)snprintf(req + len, cap - len, "%lu\n", (unsigned long)fs_len);
    memcpy(req + len, fs, fs_len);
    len += fs_len;

    gl.message[0] = '\0';
    uint64_t r = gpu_svc(GPU_GLSL, gl.ctx | (uint64_t)program << 32, (uint64_t)req, len,
                         (uint64_t)gl.message, sizeof gl.message);
    free(req);
    if (r == (uint64_t)-1) {
        say(last_error());
        return 0;
    }
    return r == 0;
}

GLuint kgl_glsl_program(const char* vs_source, const char* fs_source) {
    GLuint program = gl.next_program + 1;
    if (!glsl_request(program, vs_source, fs_source)) return 0;
    gl.next_program = program;
    return program;
}

// ---- OpenGL's shader and program objects ----------------------------------------------------

static char* copy_text(const char* s) {
    size_t n = strlen(s) + 1;
    char* copy = malloc(n);
    if (copy) memcpy(copy, s, n);
    return copy;
}

static void set_log(char** log, const char* text) {
    free(*log);
    *log = text && *text ? copy_text(text) : NULL;
}

static int shader_slot(GLuint shader) {
    if (shader == 0 || shader > MAX_SHADERS || !gl.shaders[shader - 1].type) return -1;
    return (int)shader - 1;
}

static int linkable_slot(GLuint program) {
    for (int i = 0; program && i < MAX_LINKABLE; i++)
        if (gl.linkable[i].program == program) return i;
    return -1;
}

GLuint glCreateShader(GLenum type) {
    if (type != GL_VERTEX_SHADER && type != GL_FRAGMENT_SHADER) { set_error(GL_INVALID_ENUM); return 0; }
    for (int i = 0; i < MAX_SHADERS; i++)
        if (!gl.shaders[i].type) {
            gl.shaders[i].type = type;
            gl.shaders[i].compiled = 0;
            return (GLuint)i + 1;
        }
    set_error(GL_OUT_OF_MEMORY);
    return 0;
}

void glDeleteShader(GLuint shader) {
    int i = shader_slot(shader);
    if (i < 0) return;
    free(gl.shaders[i].source);
    free(gl.shaders[i].log);
    gl.shaders[i].source = gl.shaders[i].log = NULL;
    gl.shaders[i].type = 0;
}

void glShaderSource(GLuint shader, GLsizei count, const GLchar* const* string, const GLint* length) {
    int i = shader_slot(shader);
    if (i < 0) { set_error(GL_INVALID_VALUE); return; }
    size_t total = 0;
    for (GLsizei k = 0; k < count; k++)
        total += length && length[k] >= 0 ? (size_t)length[k] : strlen(string[k]);
    char* source = malloc(total + 1);
    if (!source) { set_error(GL_OUT_OF_MEMORY); return; }
    size_t at = 0;
    for (GLsizei k = 0; k < count; k++) {
        size_t n = length && length[k] >= 0 ? (size_t)length[k] : strlen(string[k]);
        memcpy(source + at, string[k], n);
        at += n;
    }
    source[at] = '\0';
    free(gl.shaders[i].source);
    gl.shaders[i].source = source;
}

// Compiled on the Mac, on its own (the stages are joined by glLinkProgram).
void glCompileShader(GLuint shader) {
    int i = shader_slot(shader);
    if (i < 0) { set_error(GL_INVALID_VALUE); return; }
    const char* source = gl.shaders[i].source ? gl.shaders[i].source : "";
    int vertex = gl.shaders[i].type == GL_VERTEX_SHADER;
    gl.shaders[i].compiled = glsl_request(0, vertex ? source : "", vertex ? "" : source);
    set_log(&gl.shaders[i].log, gl.shaders[i].compiled ? NULL : gl.message);
}

void glGetShaderiv(GLuint shader, GLenum pname, GLint* params) {
    int i = shader_slot(shader);
    if (i < 0) { set_error(GL_INVALID_VALUE); return; }
    switch (pname) {
    case GL_SHADER_TYPE: *params = (GLint)gl.shaders[i].type; break;
    case GL_COMPILE_STATUS: *params = gl.shaders[i].compiled; break;
    case GL_DELETE_STATUS: *params = GL_FALSE; break;
    case GL_INFO_LOG_LENGTH: *params = gl.shaders[i].log ? (GLint)strlen(gl.shaders[i].log) + 1 : 0; break;
    case GL_SHADER_SOURCE_LENGTH:
        *params = gl.shaders[i].source ? (GLint)strlen(gl.shaders[i].source) + 1 : 0;
        break;
    default: set_error(GL_INVALID_ENUM);
    }
}

static void copy_log(const char* log, GLsizei max, GLsizei* length, GLchar* out) {
    size_t n = log ? strlen(log) : 0;
    if (max <= 0) { if (length) *length = 0; return; }
    if (n > (size_t)max - 1) n = (size_t)max - 1;
    memcpy(out, log ? log : "", n);
    out[n] = '\0';
    if (length) *length = (GLsizei)n;
}

void glGetShaderInfoLog(GLuint shader, GLsizei max, GLsizei* length, GLchar* log) {
    int i = shader_slot(shader);
    if (i < 0) { set_error(GL_INVALID_VALUE); return; }
    copy_log(gl.shaders[i].log, max, length, log);
}

GLuint glCreateProgram(void) {
    for (int i = 0; i < MAX_LINKABLE; i++)
        if (!gl.linkable[i].program) {
            gl.linkable[i].program = ++gl.next_program;
            gl.linkable[i].linked = 0;
            return gl.linkable[i].program;
        }
    set_error(GL_OUT_OF_MEMORY);
    return 0;
}

// Takes a copy of the shader's source as it is now.
void glAttachShader(GLuint program, GLuint shader) {
    int p = linkable_slot(program), i = shader_slot(shader);
    if (p < 0 || i < 0) { set_error(GL_INVALID_VALUE); return; }
    char** stage = gl.shaders[i].type == GL_VERTEX_SHADER ? &gl.linkable[p].vs : &gl.linkable[p].fs;
    if (*stage) { set_error(GL_INVALID_OPERATION); return; }
    *stage = copy_text(gl.shaders[i].source ? gl.shaders[i].source : "");
}

void glDetachShader(GLuint program, GLuint shader) {
    int p = linkable_slot(program), i = shader_slot(shader);
    if (p < 0 || i < 0) { set_error(GL_INVALID_VALUE); return; }
    char** stage = gl.shaders[i].type == GL_VERTEX_SHADER ? &gl.linkable[p].vs : &gl.linkable[p].fs;
    free(*stage);
    *stage = NULL;
}

void glLinkProgram(GLuint program) {
    int p = linkable_slot(program);
    if (p < 0) { set_error(GL_INVALID_VALUE); return; }
    if (!gl.linkable[p].vs || !gl.linkable[p].fs) {
        gl.linkable[p].linked = 0;
        set_log(&gl.linkable[p].log, "a program needs a vertex shader and a fragment shader");
        return;
    }
    gl.linkable[p].linked = glsl_request(program, gl.linkable[p].vs, gl.linkable[p].fs);
    set_log(&gl.linkable[p].log, gl.linkable[p].linked ? NULL : gl.message);
}

void glGetProgramiv(GLuint program, GLenum pname, GLint* params) {
    int p = linkable_slot(program);
    if (p < 0) { set_error(GL_INVALID_VALUE); return; }
    switch (pname) {
    case GL_LINK_STATUS: *params = gl.linkable[p].linked; break;
    case GL_DELETE_STATUS: *params = GL_FALSE; break;
    case GL_ATTACHED_SHADERS: *params = !!gl.linkable[p].vs + !!gl.linkable[p].fs; break;
    case GL_INFO_LOG_LENGTH: *params = gl.linkable[p].log ? (GLint)strlen(gl.linkable[p].log) + 1 : 0; break;
    default: set_error(GL_INVALID_ENUM);
    }
}

void glGetProgramInfoLog(GLuint program, GLsizei max, GLsizei* length, GLchar* log) {
    int p = linkable_slot(program);
    if (p < 0) { set_error(GL_INVALID_VALUE); return; }
    copy_log(gl.linkable[p].log, max, length, log);
}

static void free_linkable(int p) {
    free(gl.linkable[p].vs);
    free(gl.linkable[p].fs);
    free(gl.linkable[p].log);
    gl.linkable[p].vs = gl.linkable[p].fs = gl.linkable[p].log = NULL;
    gl.linkable[p].program = 0;
}

void glUseProgram(GLuint program) {
    gl.program = program;
    command(CMD_USE_PROGRAM, 1, program, 0, 0, 0);
}

void glDeleteProgram(GLuint program) {
    if (!program) return;
    int p = linkable_slot(program);
    if (p >= 0) free_linkable(p);
    if (gl.program == program) gl.program = 0;
    command(CMD_PROGRAM_DELETE, 1, program, 0, 0, 0);
}

// Locations are this program's own: an index into gl.uniforms, which
// holds each name for the device to look up.
GLint glGetUniformLocation(GLuint program, const char* name) {
    if (!program || strlen(name) > MAX_UNIFORM_NAME) return -1;
    for (int i = 0; i < gl.n_uniforms; i++)
        if (gl.uniforms[i].program == program && strcmp(gl.uniforms[i].name, name) == 0) return i;
    if (gl.n_uniforms == MAX_UNIFORMS) return -1;
    int i = gl.n_uniforms++;
    gl.uniforms[i].program = program;
    strlcpy(gl.uniforms[i].name, name, sizeof gl.uniforms[i].name);
    return i;
}

static void uniform(GLint location, uint32_t type, uint32_t components, GLsizei count, const void* values) {
    if (location < 0) return;
    if (location >= gl.n_uniforms || !gl.program || count < 1) { set_error(GL_INVALID_OPERATION); return; }
    const char* name = gl.uniforms[location].name;
    size_t name_words = strlen(name) / 4 + 1;  // with at least one NUL
    size_t value_words = (size_t)components * (size_t)count;
    uint32_t* w = begin_command(CMD_UNIFORM, 7 + name_words + value_words);
    if (!w) return;
    w[0] = gl.program; w[1] = type; w[2] = components; w[3] = (uint32_t)count; w[4] = (uint32_t)name_words;
    memset(w + 5, 0, name_words * 4);
    memcpy(w + 5, name, strlen(name));
    memcpy(w + 5 + name_words, values, value_words * 4);
}

void glUniform1f(GLint l, GLfloat x) { uniform(l, UNIFORM_FLOAT, 1, 1, &x); }
void glUniform2f(GLint l, GLfloat x, GLfloat y) { float v[2] = { x, y }; uniform(l, UNIFORM_FLOAT, 2, 1, v); }
void glUniform3f(GLint l, GLfloat x, GLfloat y, GLfloat z) { float v[3] = { x, y, z }; uniform(l, UNIFORM_FLOAT, 3, 1, v); }
void glUniform4f(GLint l, GLfloat x, GLfloat y, GLfloat z, GLfloat w) {
    float v[4] = { x, y, z, w };
    uniform(l, UNIFORM_FLOAT, 4, 1, v);
}
void glUniform1i(GLint l, GLint x) { uniform(l, UNIFORM_INT, 1, 1, &x); }
void glUniform2i(GLint l, GLint x, GLint y) { GLint v[2] = { x, y }; uniform(l, UNIFORM_INT, 2, 1, v); }
void glUniform3i(GLint l, GLint x, GLint y, GLint z) { GLint v[3] = { x, y, z }; uniform(l, UNIFORM_INT, 3, 1, v); }
void glUniform4i(GLint l, GLint x, GLint y, GLint z, GLint w) {
    GLint v[4] = { x, y, z, w };
    uniform(l, UNIFORM_INT, 4, 1, v);
}
void glUniform1fv(GLint l, GLsizei n, const GLfloat* v) { uniform(l, UNIFORM_FLOAT, 1, n, v); }
void glUniform2fv(GLint l, GLsizei n, const GLfloat* v) { uniform(l, UNIFORM_FLOAT, 2, n, v); }
void glUniform3fv(GLint l, GLsizei n, const GLfloat* v) { uniform(l, UNIFORM_FLOAT, 3, n, v); }
void glUniform4fv(GLint l, GLsizei n, const GLfloat* v) { uniform(l, UNIFORM_FLOAT, 4, n, v); }
void glUniform1iv(GLint l, GLsizei n, const GLint* v) { uniform(l, UNIFORM_INT, 1, n, v); }

// Matrices go to the device column-major; transpose = GL_TRUE means
// these are row-major.
static void uniform_matrix(GLint l, int size, GLsizei count, GLboolean transpose, const GLfloat* m) {
    if (!transpose) {
        uniform(l, size == 4 ? UNIFORM_MAT4 : UNIFORM_MAT3, size * size, count, m);
        return;
    }
    if (count < 1) return;
    float* t = malloc((size_t)count * size * size * sizeof(float));
    if (!t) { set_error(GL_OUT_OF_MEMORY); return; }
    for (GLsizei k = 0; k < count; k++)
        for (int r = 0; r < size; r++)
            for (int c = 0; c < size; c++)
                t[k * size * size + c * size + r] = m[k * size * size + r * size + c];
    uniform(l, size == 4 ? UNIFORM_MAT4 : UNIFORM_MAT3, size * size, count, t);
    free(t);
}

void glUniformMatrix3fv(GLint l, GLsizei n, GLboolean transpose, const GLfloat* m) { uniform_matrix(l, 3, n, transpose, m); }
void glUniformMatrix4fv(GLint l, GLsizei n, GLboolean transpose, const GLfloat* m) { uniform_matrix(l, 4, n, transpose, m); }

// ---- contexts -------------------------------------------------------------------------------

int kgl_available(void) {
    return gpu_svc(GPU_AVAILABLE, 0, 0, 0, 0, 0) == 1;
}

int kgl_open(canvas_t* c) {
    if (gl.ctx) { say("a context is already open"); return -1; }
    if (!kgl_available()) {
        say("no GPU: run KOS on its own QEMU (KOS.app, or make qemu, then make run-gui)");
        return -1;
    }
    uint64_t ctx = gpu_svc(GPU_OPEN, (uint64_t)c->width, (uint64_t)c->height, 0, 0, 0);
    if (ctx == (uint64_t)-1) {
        say(last_error());
        return -1;
    }
    gl.ctx = ctx;
    gl.canvas = c;
    gl.len = 0;
    reset_state();
    return 0;
}

int kgl_swap(void) {
    if (!gl.ctx) { say("no context: call kgl_open first"); return -1; }
    int r = flush(1);
    canvas_present(gl.canvas);
    return r;
}

void kgl_close(void) {
    if (!gl.ctx) return;
    gpu_svc(GPU_CLOSE, gl.ctx, 0, 0, 0, 0);
    gl.ctx = 0;
    free(gl.cmds);
    free(gl.verts);
    gl.cmds = NULL;
    gl.verts = NULL;
    gl.len = gl.cap = gl.nverts = gl.verts_cap = 0;
    for (GLuint i = 1; i <= MAX_SHADERS; i++) glDeleteShader(i);
    for (int p = 0; p < MAX_LINKABLE; p++) free_linkable(p);
    for (GLuint i = 0; i < gl.n_buffers; i++) free(gl.buffers[i].data);
    free(gl.buffers);
    free(gl.vaos);
    gl.buffers = NULL;
    gl.vaos = NULL;
    gl.n_buffers = gl.n_vaos = gl.array_buffer = gl.vao = 0;
    memset(&gl.default_vao, 0, sizeof gl.default_vao);
}

const char* kgl_error(void) {
    return gl.message;
}
