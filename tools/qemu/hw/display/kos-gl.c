/*
 * kos-gl: a virtio device that draws for KOS programs with the host's
 * OpenGL (OpenGL.framework, 4.1 core), macOS only. KOS-specific; not part
 * of upstream QEMU.
 *
 * KOS's gl.h (userspace/libc/gl.c) keeps OpenGL 1.x-style state in the
 * program and sends this device a stream of commands; KOS's driver
 * (kernel/src/drivers/virtio/kgl.rs) passes them through. Each KOS GL
 * context is a CGL context here that renders into a framebuffer object
 * the size of the program's canvas, and the finished frame is read back
 * into the canvas's pixels. HLSL shaders are compiled by kos-hlslc
 * (tools/kos-hlslc), which runs next to QEMU, into GLSL for separable
 * programs joined in a program pipeline.
 *
 * Requests (one queue): a device-readable struct KosGLRequest and payload,
 * then a device-writable struct KosGLResponse and room for its data.
 *
 *   CREATE  ctx, a = width, b = height
 *   DESTROY ctx
 *   SUBMIT  ctx, payload = commands; a = 1 to read the frame back into
 *           the response data (width * height 0x00RRGGBB, top row first)
 *   HLSL    ctx, a = program id, payload = kos-hlslc's request; the
 *           response data is the compiler's messages if it fails
 *
 * A failed request has status 1 and an error message as its data.
 *
 * Commands are 32-bit words: an opcode, the command's length in words
 * (both included), then its arguments; see the CMD_ list below.
 */

#define GL_SILENCE_DEPRECATION

#include "qemu/osdep.h"
#include "qapi/error.h"
#include "qemu/error-report.h"
#include "qemu/iov.h"
#include "qemu/log.h"
#include "qemu/module.h"
#include "hw/virtio/virtio.h"
#include "hw/core/qdev-properties.h"

#include <gio/gio.h>
#include <mach-o/dyld.h>
#include <OpenGL/OpenGL.h>
#include <OpenGL/gl3.h>

#define TYPE_KOS_GL "kos-gl-device"
OBJECT_DECLARE_SIMPLE_TYPE(KosGL, KOS_GL)

/* Not a registered virtio id: KOS looks for this one. */
#define VIRTIO_ID_KOS_GL 63
#define QUEUE_SIZE 64
#define MAX_SIZE 4096
#define MAX_LIGHTS 4
/* The texture unit uploads use, out of programs' way. */
#define UPLOAD_UNIT 15

enum {
    REQ_CREATE = 1,
    REQ_DESTROY = 2,
    REQ_SUBMIT = 3,
    REQ_HLSL = 4,
};

enum {
    STATUS_OK = 0,
    STATUS_ERROR = 1,
};

typedef struct KosGLRequest {
    uint32_t op, ctx, a, b;
} KosGLRequest;

typedef struct KosGLResponse {
    uint32_t status, len;
} KosGLResponse;

enum {
    CMD_VIEWPORT = 1,       /* x y w h */
    CMD_CLEAR_COLOR = 2,    /* r g b a (floats) */
    CMD_CLEAR_DEPTH = 3,    /* depth (float) */
    CMD_CLEAR = 4,          /* GL_*_BUFFER_BIT mask */
    CMD_ENABLE = 5,         /* cap */
    CMD_DISABLE = 6,        /* cap */
    CMD_BLEND_FUNC = 7,     /* src dst */
    CMD_DEPTH_FUNC = 8,     /* func */
    CMD_DEPTH_MASK = 9,     /* flag */
    CMD_CULL_FACE = 10,     /* mode */
    CMD_FRONT_FACE = 11,    /* mode */
    CMD_TEX_IMAGE = 12,     /* id w h y0 rows, rows * w 0xAARRGGBB */
    CMD_TEX_DELETE = 13,    /* id */
    CMD_TEX_PARAM = 14,     /* id pname value */
    CMD_BIND_TEXTURE = 15,  /* unit id (0 unbinds) */
    CMD_USE_PROGRAM = 16,   /* id (0: fixed function) */
    CMD_PROGRAM_DELETE = 17, /* id */
    CMD_UNIFORM = 18,       /* program type components count name_words, name, values */
    CMD_FIXED_STATE = 19,   /* struct FixedState */
    CMD_DRAW = 20,          /* mode count, count vertices of VERTEX_FLOATS floats */
    CMD_GENERATE_MIPMAP = 21, /* id */
    CMD_SCISSOR = 22,       /* x y w h */
    CMD_COLOR_MASK = 23,    /* r g b a */
    CMD_POLYGON_OFFSET = 24, /* factor units (floats) */
};

/* CMD_UNIFORM types. Matrices come in OpenGL's column-major order. */
enum {
    UNIFORM_FLOAT = 1,
    UNIFORM_INT = 2,
    UNIFORM_MAT3 = 3,
    UNIFORM_MAT4 = 4,
};

/* A vertex: position xyzw, color rgba, normal xyz, texture coordinate st. */
#define VERTEX_FLOATS 13

/* FixedState flags. */
#define FIXED_LIGHTING        (1u << 0)
#define FIXED_COLOR_MATERIAL  (1u << 1)
#define FIXED_TEXTURE         (1u << 2)
#define FIXED_LIGHT0          (1u << 3)   /* .. FIXED_LIGHT0 << 3 */

typedef struct FixedLight {
    float position[4];      /* in eye space */
    float ambient[4], diffuse[4], specular[4];
} FixedLight;

/* What the fixed-function pipeline needs; gl.c keeps it up to date. */
typedef struct FixedState {
    float modelview[16], projection[16];
    uint32_t flags;
    float scene_ambient[4];
    float mat_ambient[4], mat_diffuse[4], mat_specular[4], mat_emission[4];
    float shininess;
    FixedLight lights[MAX_LIGHTS];
    uint32_t fog_mode;      /* 0 off, 1 linear, 2 exp, 3 exp2 */
    float fog_start, fog_end, fog_density, fog_color[4];
    uint32_t alpha_func;    /* GL_NEVER..GL_ALWAYS minus GL_NEVER */
    float alpha_ref;
} FixedState;

static const char *fixed_vs =
    "#version 410\n"
    "layout(location = 0) in vec4 a_pos;\n"
    "layout(location = 1) in vec4 a_color;\n"
    "layout(location = 2) in vec3 a_normal;\n"
    "layout(location = 3) in vec2 a_uv;\n"
    "uniform mat4 u_modelview, u_projection;\n"
    "uniform mat3 u_normal_matrix;\n"
    "uniform bool u_lighting, u_color_material;\n"
    "uniform vec4 u_scene_ambient, u_mat_ambient, u_mat_diffuse, u_mat_specular, u_mat_emission;\n"
    "uniform float u_shininess;\n"
    "uniform bool u_light_on[4];\n"
    "uniform vec4 u_light_pos[4], u_light_ambient[4], u_light_diffuse[4], u_light_specular[4];\n"
    "out vec4 v_color;\n"
    "out vec2 v_uv;\n"
    "out float v_distance;\n"
    "void main() {\n"
    "    vec4 eye = u_modelview * a_pos;\n"
    "    gl_Position = u_projection * eye;\n"
    "    v_uv = a_uv;\n"
    "    v_distance = length(eye.xyz);\n"
    "    if (!u_lighting) { v_color = a_color; return; }\n"
    "    vec4 ambient = u_color_material ? a_color : u_mat_ambient;\n"
    "    vec4 diffuse = u_color_material ? a_color : u_mat_diffuse;\n"
    "    vec3 n = normalize(u_normal_matrix * a_normal);\n"
    "    vec3 c = u_mat_emission.rgb + u_scene_ambient.rgb * ambient.rgb;\n"
    "    for (int i = 0; i < 4; i++) {\n"
    "        if (!u_light_on[i]) continue;\n"
    "        vec3 l = u_light_pos[i].w == 0.0 ? normalize(u_light_pos[i].xyz)\n"
    "                                         : normalize(u_light_pos[i].xyz - eye.xyz);\n"
    "        float d = max(dot(n, l), 0.0);\n"
    "        c += u_light_ambient[i].rgb * ambient.rgb + d * u_light_diffuse[i].rgb * diffuse.rgb;\n"
    "        if (d > 0.0) {\n"
    "            float s = max(dot(n, normalize(l + vec3(0.0, 0.0, 1.0))), 0.0);\n"
    "            c += pow(s, u_shininess) * u_light_specular[i].rgb * u_mat_specular.rgb;\n"
    "        }\n"
    "    }\n"
    "    v_color = vec4(clamp(c, 0.0, 1.0), diffuse.a);\n"
    "}\n";

static const char *fixed_fs =
    "#version 410\n"
    "in vec4 v_color;\n"
    "in vec2 v_uv;\n"
    "in float v_distance;\n"
    "uniform bool u_texture;\n"
    "uniform sampler2D u_tex;\n"
    "uniform int u_fog_mode, u_alpha_func;\n"
    "uniform float u_fog_start, u_fog_end, u_fog_density, u_alpha_ref;\n"
    "uniform vec4 u_fog_color;\n"
    "out vec4 o_color;\n"
    "void main() {\n"
    "    vec4 c = v_color;\n"
    "    if (u_texture) c *= texture(u_tex, v_uv);\n"
    "    float a = c.a, r = u_alpha_ref;\n"
    "    bool pass = u_alpha_func == 1 ? a < r : u_alpha_func == 2 ? a == r : u_alpha_func == 3 ? a <= r\n"
    "              : u_alpha_func == 4 ? a > r : u_alpha_func == 5 ? a != r : u_alpha_func == 6 ? a >= r\n"
    "              : u_alpha_func == 7;\n"
    "    if (!pass) discard;\n"
    "    if (u_fog_mode != 0) {\n"
    "        float z = v_distance, f;\n"
    "        if (u_fog_mode == 1) f = (u_fog_end - z) / (u_fog_end - u_fog_start);\n"
    "        else if (u_fog_mode == 2) f = exp(-u_fog_density * z);\n"
    "        else f = exp(-pow(u_fog_density * z, 2.0));\n"
    "        c.rgb = mix(u_fog_color.rgb, c.rgb, clamp(f, 0.0, 1.0));\n"
    "    }\n"
    "    o_color = c;\n"
    "}\n";

/* The fixed-function program's uniforms, looked up once. */
typedef struct FixedLocations {
    GLint modelview, projection, normal_matrix, lighting, color_material;
    GLint scene_ambient, mat_ambient, mat_diffuse, mat_specular, mat_emission, shininess;
    GLint light_on, light_pos, light_ambient, light_diffuse, light_specular;
    GLint texture, tex, fog_mode, fog_start, fog_end, fog_density, fog_color, alpha_func, alpha_ref;
} FixedLocations;

/* A uniform's location in each stage's program (-1 if it has none). */
typedef struct KosGLUniform {
    GLint vs, ps;
} KosGLUniform;

typedef struct KosGLProgram {
    GLuint pipeline, vs, ps;
    GHashTable *uniforms;   /* name -> KosGLUniform* */
} KosGLProgram;

typedef struct KosGLContext {
    uint32_t id;
    CGLContextObj cgl;
    uint32_t width, height;
    GLuint fbo, color, depth, vao, vbo;
    GLuint fixed;
    /* Texture 0: one white texel, on every unit nothing else is bound to. */
    GLuint white;
    FixedLocations loc;
    FixedState state;
    GHashTable *textures;   /* guest id -> GLuint */
    GHashTable *programs;   /* guest id -> KosGLProgram* */
    KosGLProgram *current;  /* NULL: fixed function */
} KosGLContext;

struct KosGL {
    VirtIODevice parent_obj;
    VirtQueue *vq;
    CGLPixelFormatObj format;
    GHashTable *contexts;   /* id -> KosGLContext* */
    char *hlslc;            /* the kos-hlslc to run */
};

/* ---- contexts --------------------------------------------------------------- */

static GLuint compile_shader(GLenum stage, const char *source, GString *log)
{
    GLuint shader = glCreateShader(stage);
    GLint ok = 0;
    glShaderSource(shader, 1, &source, NULL);
    glCompileShader(shader);
    glGetShaderiv(shader, GL_COMPILE_STATUS, &ok);
    if (!ok) {
        char buf[4096];
        glGetShaderInfoLog(shader, sizeof buf, NULL, buf);
        g_string_append(log, buf);
        glDeleteShader(shader);
        return 0;
    }
    return shader;
}

static GLuint build_fixed_program(void)
{
    g_autoptr(GString) log = g_string_new(NULL);
    GLuint vs = compile_shader(GL_VERTEX_SHADER, fixed_vs, log);
    GLuint fs = compile_shader(GL_FRAGMENT_SHADER, fixed_fs, log);
    GLuint program = 0;
    GLint ok = 0;

    if (vs && fs) {
        program = glCreateProgram();
        glAttachShader(program, vs);
        glAttachShader(program, fs);
        glLinkProgram(program);
        glGetProgramiv(program, GL_LINK_STATUS, &ok);
        if (!ok) {
            char buf[4096];
            glGetProgramInfoLog(program, sizeof buf, NULL, buf);
            g_string_append(log, buf);
            glDeleteProgram(program);
            program = 0;
        }
    }
    glDeleteShader(vs);
    glDeleteShader(fs);
    if (!program) {
        error_report("kos-gl: fixed-function shaders failed: %s", log->str);
    }
    return program;
}

static void find_fixed_locations(KosGLContext *c)
{
    GLuint p = c->fixed;
    FixedLocations *l = &c->loc;
#define LOC(field, name) l->field = glGetUniformLocation(p, name)
    LOC(modelview, "u_modelview");
    LOC(projection, "u_projection");
    LOC(normal_matrix, "u_normal_matrix");
    LOC(lighting, "u_lighting");
    LOC(color_material, "u_color_material");
    LOC(scene_ambient, "u_scene_ambient");
    LOC(mat_ambient, "u_mat_ambient");
    LOC(mat_diffuse, "u_mat_diffuse");
    LOC(mat_specular, "u_mat_specular");
    LOC(mat_emission, "u_mat_emission");
    LOC(shininess, "u_shininess");
    LOC(light_on, "u_light_on");
    LOC(light_pos, "u_light_pos");
    LOC(light_ambient, "u_light_ambient");
    LOC(light_diffuse, "u_light_diffuse");
    LOC(light_specular, "u_light_specular");
    LOC(texture, "u_texture");
    LOC(tex, "u_tex");
    LOC(fog_mode, "u_fog_mode");
    LOC(fog_start, "u_fog_start");
    LOC(fog_end, "u_fog_end");
    LOC(fog_density, "u_fog_density");
    LOC(fog_color, "u_fog_color");
    LOC(alpha_func, "u_alpha_func");
    LOC(alpha_ref, "u_alpha_ref");
#undef LOC
}

static void free_program(gpointer data)
{
    KosGLProgram *p = data;
    glDeleteProgramPipelines(1, &p->pipeline);
    glDeleteProgram(p->vs);
    glDeleteProgram(p->ps);
    g_hash_table_destroy(p->uniforms);
    g_free(p);
}

static void free_texture(gpointer data)
{
    GLuint tex = GPOINTER_TO_UINT(data);
    glDeleteTextures(1, &tex);
}

static void identity(float *m)
{
    memset(m, 0, 16 * sizeof(float));
    m[0] = m[5] = m[10] = m[15] = 1;
}

static KosGLContext *context_new(KosGL *s, uint32_t id, uint32_t width, uint32_t height, Error **errp)
{
    KosGLContext *c = g_new0(KosGLContext, 1);
    CGLError err = CGLCreateContext(s->format, NULL, &c->cgl);

    if (err != kCGLNoError) {
        error_setg(errp, "CGLCreateContext: %s", CGLErrorString(err));
        g_free(c);
        return NULL;
    }
    CGLSetCurrentContext(c->cgl);
    c->id = id;
    c->width = width;
    c->height = height;

    glGenRenderbuffers(1, &c->color);
    glBindRenderbuffer(GL_RENDERBUFFER, c->color);
    glRenderbufferStorage(GL_RENDERBUFFER, GL_RGBA8, width, height);
    glGenRenderbuffers(1, &c->depth);
    glBindRenderbuffer(GL_RENDERBUFFER, c->depth);
    glRenderbufferStorage(GL_RENDERBUFFER, GL_DEPTH24_STENCIL8, width, height);
    glGenFramebuffers(1, &c->fbo);
    glBindFramebuffer(GL_FRAMEBUFFER, c->fbo);
    glFramebufferRenderbuffer(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_RENDERBUFFER, c->color);
    glFramebufferRenderbuffer(GL_FRAMEBUFFER, GL_DEPTH_STENCIL_ATTACHMENT, GL_RENDERBUFFER, c->depth);
    if (glCheckFramebufferStatus(GL_FRAMEBUFFER) != GL_FRAMEBUFFER_COMPLETE) {
        error_setg(errp, "framebuffer incomplete");
    }
    glViewport(0, 0, width, height);

    /* One buffer holds each draw's vertices, laid out as in VERTEX_FLOATS. */
    glGenVertexArrays(1, &c->vao);
    glBindVertexArray(c->vao);
    glGenBuffers(1, &c->vbo);
    glBindBuffer(GL_ARRAY_BUFFER, c->vbo);
    static const int sizes[] = { 4, 4, 3, 2 };
    size_t offset = 0;
    for (int i = 0; i < 4; i++) {
        glEnableVertexAttribArray(i);
        glVertexAttribPointer(i, sizes[i], GL_FLOAT, GL_FALSE, VERTEX_FLOATS * sizeof(float),
                              (const void *)offset);
        offset += sizes[i] * sizeof(float);
    }

    c->fixed = build_fixed_program();
    if (!c->fixed) {
        error_setg(errp, "no fixed-function program");
    } else {
        find_fixed_locations(c);
        glProgramUniform1i(c->fixed, c->loc.tex, 0);
    }
    static const uint32_t white = 0xffffffff;
    glGenTextures(1, &c->white);
    for (int unit = 0; unit < UPLOAD_UNIT; unit++) {
        glActiveTexture(GL_TEXTURE0 + unit);
        glBindTexture(GL_TEXTURE_2D, c->white);
    }
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST);
    glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA8, 1, 1, 0, GL_BGRA, GL_UNSIGNED_INT_8_8_8_8_REV, &white);
    identity(c->state.modelview);
    identity(c->state.projection);
    c->state.alpha_func = 7;
    c->textures = g_hash_table_new_full(NULL, NULL, NULL, free_texture);
    c->programs = g_hash_table_new_full(NULL, NULL, NULL, free_program);
    CGLSetCurrentContext(NULL);
    return c;
}

static void context_free(gpointer data)
{
    KosGLContext *c = data;
    CGLSetCurrentContext(c->cgl);
    g_hash_table_destroy(c->textures);
    g_hash_table_destroy(c->programs);
    glDeleteProgram(c->fixed);
    glDeleteTextures(1, &c->white);
    glDeleteBuffers(1, &c->vbo);
    glDeleteVertexArrays(1, &c->vao);
    glDeleteFramebuffers(1, &c->fbo);
    glDeleteRenderbuffers(1, &c->color);
    glDeleteRenderbuffers(1, &c->depth);
    CGLSetCurrentContext(NULL);
    CGLDestroyContext(c->cgl);
    g_free(c);
}

/* ---- uniforms ------------------------------------------------------------------ */

/*
 * Where `name` is in one stage's program: as itself, or as a member of a
 * cbuffer, which kos-hlslc makes a struct uniform (a global `float time`
 * is kos_Global.time).
 */
static GLint find_uniform(GLuint program, const char *name)
{
    GLint loc = glGetUniformLocation(program, name);
    if (loc >= 0) {
        return loc;
    }
    g_autofree char *global = g_strconcat("kos_Global.", name, NULL);
    loc = glGetUniformLocation(program, global);
    if (loc >= 0) {
        return loc;
    }
    GLint count = 0;
    glGetProgramiv(program, GL_ACTIVE_UNIFORMS, &count);
    g_autofree char *suffix = g_strconcat(".", name, NULL);
    for (GLint i = 0; i < count; i++) {
        char active[256];
        GLint size;
        GLenum type;
        glGetActiveUniform(program, i, sizeof active, NULL, &size, &type, active);
        char *bracket = strstr(active, "[0]");
        if (bracket && bracket[3] == '\0') {
            *bracket = '\0';
        }
        if (g_str_has_prefix(active, "kos") && g_str_has_suffix(active, suffix)) {
            return glGetUniformLocation(program, active);
        }
    }
    return -1;
}

static KosGLUniform *program_uniform(KosGLProgram *p, const char *name)
{
    KosGLUniform *u = g_hash_table_lookup(p->uniforms, name);
    if (!u) {
        u = g_new(KosGLUniform, 1);
        u->vs = find_uniform(p->vs, name);
        u->ps = find_uniform(p->ps, name);
        g_hash_table_insert(p->uniforms, g_strdup(name), u);
    }
    return u;
}

/* Matrices are transposed on the way in: HLSL's mul(m, v) becomes v * m in GLSL. */
static void set_uniform(GLuint program, GLint loc, uint32_t type, uint32_t components, GLsizei count,
                        const void *values)
{
    if (loc < 0) {
        return;
    }
    switch (type) {
    case UNIFORM_FLOAT:
        switch (components) {
        case 1: glProgramUniform1fv(program, loc, count, values); break;
        case 2: glProgramUniform2fv(program, loc, count, values); break;
        case 3: glProgramUniform3fv(program, loc, count, values); break;
        case 4: glProgramUniform4fv(program, loc, count, values); break;
        }
        break;
    case UNIFORM_INT:
        switch (components) {
        case 1: glProgramUniform1iv(program, loc, count, values); break;
        case 2: glProgramUniform2iv(program, loc, count, values); break;
        case 3: glProgramUniform3iv(program, loc, count, values); break;
        case 4: glProgramUniform4iv(program, loc, count, values); break;
        }
        break;
    case UNIFORM_MAT3:
        glProgramUniformMatrix3fv(program, loc, count, GL_TRUE, values);
        break;
    case UNIFORM_MAT4:
        glProgramUniformMatrix4fv(program, loc, count, GL_TRUE, values);
        break;
    }
}

static void set_program_uniform(KosGLProgram *p, const char *name, uint32_t type, uint32_t components,
                                GLsizei count, const void *values)
{
    KosGLUniform *u = program_uniform(p, name);
    set_uniform(p->vs, u->vs, type, components, count, values);
    set_uniform(p->ps, u->ps, type, components, count, values);
}

/* ---- drawing ------------------------------------------------------------------- */

/* out = a * b, column-major. */
static void mat4_mul(float *out, const float *a, const float *b)
{
    for (int col = 0; col < 4; col++) {
        for (int row = 0; row < 4; row++) {
            float sum = 0;
            for (int k = 0; k < 4; k++) {
                sum += a[k * 4 + row] * b[col * 4 + k];
            }
            out[col * 4 + row] = sum;
        }
    }
}

/* The inverse transpose of m's upper 3x3, column-major, for normals. */
static void normal_matrix(float *out, const float *m)
{
    float a = m[0], b = m[4], c = m[8];
    float d = m[1], e = m[5], f = m[9];
    float g = m[2], h = m[6], i = m[10];
    float det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    float inv = det != 0 ? 1 / det : 0;
    /* The transpose of the inverse is the cofactor matrix over det. */
    float cof[9] = {
        (e * i - f * h), -(d * i - f * g), (d * h - e * g),
        -(b * i - c * h), (a * i - c * g), -(a * h - b * g),
        (b * f - c * e), -(a * f - c * d), (a * e - b * d),
    };
    /* cof is row-major (row r = cofactors of row r); column-major out. */
    for (int r = 0; r < 3; r++) {
        for (int col = 0; col < 3; col++) {
            out[col * 3 + r] = cof[r * 3 + col] * inv;
        }
    }
}

static void upload_fixed_state(KosGLContext *c)
{
    const FixedState *s = &c->state;
    const FixedLocations *l = &c->loc;
    GLuint p = c->fixed;
    float nm[9];
    GLint on[MAX_LIGHTS];
    float pos[MAX_LIGHTS][4], amb[MAX_LIGHTS][4], dif[MAX_LIGHTS][4], spec[MAX_LIGHTS][4];

    normal_matrix(nm, s->modelview);
    glProgramUniformMatrix4fv(p, l->modelview, 1, GL_FALSE, s->modelview);
    glProgramUniformMatrix4fv(p, l->projection, 1, GL_FALSE, s->projection);
    glProgramUniformMatrix3fv(p, l->normal_matrix, 1, GL_FALSE, nm);
    glProgramUniform1i(p, l->lighting, !!(s->flags & FIXED_LIGHTING));
    glProgramUniform1i(p, l->color_material, !!(s->flags & FIXED_COLOR_MATERIAL));
    glProgramUniform4fv(p, l->scene_ambient, 1, s->scene_ambient);
    glProgramUniform4fv(p, l->mat_ambient, 1, s->mat_ambient);
    glProgramUniform4fv(p, l->mat_diffuse, 1, s->mat_diffuse);
    glProgramUniform4fv(p, l->mat_specular, 1, s->mat_specular);
    glProgramUniform4fv(p, l->mat_emission, 1, s->mat_emission);
    glProgramUniform1f(p, l->shininess, s->shininess);
    for (int i = 0; i < MAX_LIGHTS; i++) {
        on[i] = !!(s->flags & (FIXED_LIGHT0 << i));
        memcpy(pos[i], s->lights[i].position, sizeof pos[i]);
        memcpy(amb[i], s->lights[i].ambient, sizeof amb[i]);
        memcpy(dif[i], s->lights[i].diffuse, sizeof dif[i]);
        memcpy(spec[i], s->lights[i].specular, sizeof spec[i]);
    }
    glProgramUniform1iv(p, l->light_on, MAX_LIGHTS, on);
    glProgramUniform4fv(p, l->light_pos, MAX_LIGHTS, &pos[0][0]);
    glProgramUniform4fv(p, l->light_ambient, MAX_LIGHTS, &amb[0][0]);
    glProgramUniform4fv(p, l->light_diffuse, MAX_LIGHTS, &dif[0][0]);
    glProgramUniform4fv(p, l->light_specular, MAX_LIGHTS, &spec[0][0]);
    glProgramUniform1i(p, l->texture, !!(s->flags & FIXED_TEXTURE));
    glProgramUniform1i(p, l->fog_mode, s->fog_mode);
    glProgramUniform1f(p, l->fog_start, s->fog_start);
    glProgramUniform1f(p, l->fog_end, s->fog_end);
    glProgramUniform1f(p, l->fog_density, s->fog_density);
    glProgramUniform4fv(p, l->fog_color, 1, s->fog_color);
    glProgramUniform1i(p, l->alpha_func, s->alpha_func);
    glProgramUniform1f(p, l->alpha_ref, s->alpha_ref);
}

/* Gives an HLSL program the fixed-function matrices, if it declares them. */
static void upload_program_matrices(KosGLContext *c, KosGLProgram *p)
{
    float mvp[16];
    mat4_mul(mvp, c->state.projection, c->state.modelview);
    set_program_uniform(p, "kos_mvp", UNIFORM_MAT4, 16, 1, mvp);
    set_program_uniform(p, "kos_modelview", UNIFORM_MAT4, 16, 1, c->state.modelview);
    set_program_uniform(p, "kos_projection", UNIFORM_MAT4, 16, 1, c->state.projection);
}

static bool valid_mode(uint32_t mode)
{
    return mode <= GL_TRIANGLE_FAN;
}

static bool valid_cap(uint32_t cap)
{
    switch (cap) {
    case GL_DEPTH_TEST:
    case GL_BLEND:
    case GL_CULL_FACE:
    case GL_SCISSOR_TEST:
    case GL_POLYGON_OFFSET_FILL:
        return true;
    }
    return false;
}

static GLuint texture_for(KosGLContext *c, uint32_t id, bool create)
{
    if (id == 0) {
        return c->white;
    }
    GLuint tex = GPOINTER_TO_UINT(g_hash_table_lookup(c->textures, GUINT_TO_POINTER(id)));
    if (!tex && create) {
        glGenTextures(1, &tex);
        glActiveTexture(GL_TEXTURE0 + UPLOAD_UNIT);
        glBindTexture(GL_TEXTURE_2D, tex);
        /* No mipmaps unless asked for, so a plain texture is complete. */
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
        g_hash_table_insert(c->textures, GUINT_TO_POINTER(id), GUINT_TO_POINTER(tex));
    }
    return tex;
}

/* Binds texture `id` to the upload unit, out of programs' way. */
static GLuint bind_for_upload(KosGLContext *c, uint32_t id)
{
    GLuint tex = texture_for(c, id, true);
    glActiveTexture(GL_TEXTURE0 + UPLOAD_UNIT);
    glBindTexture(GL_TEXTURE_2D, tex);
    return tex;
}

static bool valid_tex_param(uint32_t pname, uint32_t value)
{
    switch (pname) {
    case GL_TEXTURE_MIN_FILTER:
        return value == GL_NEAREST || value == GL_LINEAR || (value >= GL_NEAREST_MIPMAP_NEAREST &&
                                                             value <= GL_LINEAR_MIPMAP_LINEAR);
    case GL_TEXTURE_MAG_FILTER:
        return value == GL_NEAREST || value == GL_LINEAR;
    case GL_TEXTURE_WRAP_S:
    case GL_TEXTURE_WRAP_T:
        return value == GL_REPEAT || value == GL_CLAMP_TO_EDGE || value == GL_MIRRORED_REPEAT;
    }
    return false;
}

static float as_float(uint32_t w)
{
    float f;
    memcpy(&f, &w, sizeof f);
    return f;
}

/*
 * Runs a stream of commands. Returns NULL, or what was wrong with the
 * command it stopped at.
 */
static char *run_commands(KosGLContext *c, const uint32_t *w, size_t words)
{
    size_t i = 0;
    while (i < words) {
        if (words - i < 2 || w[i + 1] < 2 || w[i + 1] > words - i) {
            return g_strdup_printf("malformed command at word %zu", i);
        }
        uint32_t op = w[i];
        size_t len = w[i + 1];
        const uint32_t *a = &w[i + 2];
        size_t n = len - 2;
        i += len;

#define NEED(count) do { \
            if (n < (count)) { \
                return g_strdup_printf("command %u is too short", op); \
            } \
        } while (0)

        switch (op) {
        case CMD_VIEWPORT:
            NEED(4);
            glViewport(a[0], a[1], a[2], a[3]);
            break;
        case CMD_SCISSOR:
            NEED(4);
            glScissor(a[0], a[1], a[2], a[3]);
            break;
        case CMD_CLEAR_COLOR:
            NEED(4);
            glClearColor(as_float(a[0]), as_float(a[1]), as_float(a[2]), as_float(a[3]));
            break;
        case CMD_CLEAR_DEPTH:
            NEED(1);
            glClearDepth(as_float(a[0]));
            break;
        case CMD_CLEAR:
            NEED(1);
            glClear(a[0] & (GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT | GL_STENCIL_BUFFER_BIT));
            break;
        case CMD_ENABLE:
        case CMD_DISABLE:
            NEED(1);
            if (!valid_cap(a[0])) {
                return g_strdup_printf("can't enable or disable %#x", a[0]);
            }
            (op == CMD_ENABLE ? glEnable : glDisable)(a[0]);
            break;
        case CMD_BLEND_FUNC:
            NEED(2);
            glBlendFunc(a[0], a[1]);
            break;
        case CMD_DEPTH_FUNC:
            NEED(1);
            glDepthFunc(a[0]);
            break;
        case CMD_DEPTH_MASK:
            NEED(1);
            glDepthMask(a[0] ? GL_TRUE : GL_FALSE);
            break;
        case CMD_COLOR_MASK:
            NEED(4);
            glColorMask(!!a[0], !!a[1], !!a[2], !!a[3]);
            break;
        case CMD_CULL_FACE:
            NEED(1);
            glCullFace(a[0]);
            break;
        case CMD_FRONT_FACE:
            NEED(1);
            glFrontFace(a[0]);
            break;
        case CMD_POLYGON_OFFSET:
            NEED(2);
            glPolygonOffset(as_float(a[0]), as_float(a[1]));
            break;
        case CMD_TEX_IMAGE: {
            NEED(5);
            uint32_t id = a[0], width = a[1], height = a[2], y0 = a[3], rows = a[4];
            if (!id || !width || !height || width > MAX_SIZE || height > MAX_SIZE ||
                y0 >= height || rows > height - y0 || n - 5 < (size_t)width * rows) {
                return g_strdup_printf("bad texture image for texture %u", id);
            }
            bind_for_upload(c, id);
            if (y0 == 0) {
                glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA8, width, height, 0, GL_BGRA,
                             GL_UNSIGNED_INT_8_8_8_8_REV, NULL);
            }
            glPixelStorei(GL_UNPACK_ALIGNMENT, 4);
            glTexSubImage2D(GL_TEXTURE_2D, 0, 0, y0, width, rows, GL_BGRA, GL_UNSIGNED_INT_8_8_8_8_REV, &a[5]);
            break;
        }
        case CMD_TEX_DELETE:
            NEED(1);
            g_hash_table_remove(c->textures, GUINT_TO_POINTER(a[0]));
            break;
        case CMD_TEX_PARAM:
            NEED(3);
            if (!valid_tex_param(a[1], a[2])) {
                return g_strdup_printf("bad texture parameter %#x = %#x", a[1], a[2]);
            }
            bind_for_upload(c, a[0]);
            glTexParameteri(GL_TEXTURE_2D, a[1], a[2]);
            break;
        case CMD_GENERATE_MIPMAP:
            NEED(1);
            bind_for_upload(c, a[0]);
            glGenerateMipmap(GL_TEXTURE_2D);
            break;
        case CMD_BIND_TEXTURE:
            NEED(2);
            if (a[0] >= UPLOAD_UNIT) {
                return g_strdup_printf("no texture unit %u", a[0]);
            }
            glActiveTexture(GL_TEXTURE0 + a[0]);
            glBindTexture(GL_TEXTURE_2D, texture_for(c, a[1], true));
            break;
        case CMD_USE_PROGRAM:
            NEED(1);
            if (a[0] == 0) {
                c->current = NULL;
            } else {
                c->current = g_hash_table_lookup(c->programs, GUINT_TO_POINTER(a[0]));
                if (!c->current) {
                    return g_strdup_printf("no program %u", a[0]);
                }
            }
            break;
        case CMD_PROGRAM_DELETE:
            NEED(1);
            if (c->current == g_hash_table_lookup(c->programs, GUINT_TO_POINTER(a[0]))) {
                c->current = NULL;
            }
            g_hash_table_remove(c->programs, GUINT_TO_POINTER(a[0]));
            break;
        case CMD_UNIFORM: {
            NEED(5);
            uint32_t type = a[1], components = a[2], count = a[3], name_words = a[4];
            if (type < UNIFORM_FLOAT || type > UNIFORM_MAT4 || components < 1 ||
                components > (type == UNIFORM_MAT4 ? 16 : type == UNIFORM_MAT3 ? 9 : 4) ||
                count < 1 || count > 4096 || name_words < 1 || name_words > 64 ||
                n - 5 < name_words + (size_t)components * count) {
                return g_strdup("bad uniform");
            }
            KosGLProgram *p = g_hash_table_lookup(c->programs, GUINT_TO_POINTER(a[0]));
            if (!p) {
                return g_strdup_printf("no program %u", a[0]);
            }
            g_autofree char *name = g_strndup((const char *)&a[5], name_words * 4);
            set_program_uniform(p, name, type, components, count, &a[5 + name_words]);
            break;
        }
        case CMD_FIXED_STATE:
            if (n * 4 < sizeof(FixedState)) {
                return g_strdup("fixed-function state is too short");
            }
            memcpy(&c->state, a, sizeof(FixedState));
            if (c->state.alpha_func > 7) {
                c->state.alpha_func = 7;
            }
            break;
        case CMD_DRAW: {
            NEED(2);
            uint32_t mode = a[0], count = a[1];
            if (!valid_mode(mode) || n - 2 < (size_t)count * VERTEX_FLOATS) {
                return g_strdup_printf("bad draw (mode %u, %u vertices)", mode, count);
            }
            if (c->current) {
                glUseProgram(0);
                glBindProgramPipeline(c->current->pipeline);
                upload_program_matrices(c, c->current);
            } else {
                glBindProgramPipeline(0);
                glUseProgram(c->fixed);
                upload_fixed_state(c);
            }
            glBufferData(GL_ARRAY_BUFFER, (GLsizeiptr)count * VERTEX_FLOATS * sizeof(float), &a[2],
                         GL_STREAM_DRAW);
            glDrawArrays(mode, 0, count);
            break;
        }
        default:
            return g_strdup_printf("unknown command %u", op);
        }
#undef NEED
    }
    GLenum err = glGetError();
    if (err != GL_NO_ERROR) {
        qemu_log_mask(LOG_GUEST_ERROR, "kos-gl: context %u: OpenGL error %#x\n", c->id, err);
    }
    return NULL;
}

/* The frame, top row first, as 0x00RRGGBB. */
static void read_frame(KosGLContext *c, uint32_t *out)
{
    size_t row = c->width;
    glFinish();
    glPixelStorei(GL_PACK_ALIGNMENT, 4);
    glReadPixels(0, 0, c->width, c->height, GL_BGRA, GL_UNSIGNED_INT_8_8_8_8_REV, out);
    /* OpenGL's rows go bottom to top. */
    g_autofree uint32_t *tmp = g_new(uint32_t, row);
    for (uint32_t y = 0; y < c->height / 2; y++) {
        uint32_t *top = out + y * row, *bottom = out + (c->height - 1 - y) * row;
        memcpy(tmp, top, row * 4);
        memcpy(top, bottom, row * 4);
        memcpy(bottom, tmp, row * 4);
    }
    for (size_t i = 0; i < row * c->height; i++) {
        out[i] &= 0x00ffffff;
    }
}

/* ---- HLSL ---------------------------------------------------------------------- */

static GLuint separable_program(GLenum stage, const char *source, GString *log)
{
    GLuint program = glCreateShaderProgramv(stage, 1, &source);
    GLint ok = 0;
    glGetProgramiv(program, GL_LINK_STATUS, &ok);
    if (!ok) {
        char buf[4096];
        glGetProgramInfoLog(program, sizeof buf, NULL, buf);
        g_string_append_printf(log, "%s shader (as GLSL): %s", stage == GL_VERTEX_SHADER ? "vertex" : "pixel",
                               buf);
        glDeleteProgram(program);
        return 0;
    }
    return program;
}

/* Points each kos_t<unit>_s<sampler> sampler at texture unit <unit>. */
static void bind_samplers(GLuint program)
{
    GLint count = 0;
    glGetProgramiv(program, GL_ACTIVE_UNIFORMS, &count);
    for (GLint i = 0; i < count; i++) {
        char name[256];
        GLint size;
        GLenum type;
        unsigned unit;
        glGetActiveUniform(program, i, sizeof name, NULL, &size, &type, name);
        if (sscanf(name, "kos_t%u_s", &unit) == 1 && unit < UPLOAD_UNIT) {
            glProgramUniform1i(program, glGetUniformLocation(program, name), unit);
        }
    }
}

/* Takes the next "<length>\n<bytes>" block of kos-hlslc's output. */
static char *take_block(const char **p, const char *end)
{
    char *nl = memchr(*p, '\n', end - *p);
    if (!nl) {
        return NULL;
    }
    size_t len = strtoul(*p, NULL, 10);
    if (len > (size_t)(end - nl - 1)) {
        return NULL;
    }
    char *block = g_strndup(nl + 1, len);
    *p = nl + 1 + len;
    return block;
}

/*
 * Compiles an HLSL program as program `id`. Returns NULL, or the
 * compiler's (or OpenGL's) messages.
 */
static char *compile_hlsl(KosGL *s, KosGLContext *c, uint32_t id, const void *request, size_t len)
{
    g_autoptr(GError) err = NULL;
    const char *argv[] = { s->hlslc, NULL };
    g_autoptr(GSubprocess) proc = g_subprocess_newv(argv, G_SUBPROCESS_FLAGS_STDIN_PIPE |
                                                    G_SUBPROCESS_FLAGS_STDOUT_PIPE, &err);
    if (!proc) {
        return g_strdup_printf("can't run the HLSL compiler (%s): %s", s->hlslc, err->message);
    }
    g_autoptr(GBytes) in = g_bytes_new(request, len);
    g_autoptr(GBytes) out = NULL;
    if (!g_subprocess_communicate(proc, in, NULL, &out, NULL, &err)) {
        return g_strdup_printf("the HLSL compiler failed: %s", err->message);
    }
    gsize out_len = 0;
    const char *text = g_bytes_get_data(out, &out_len);
    const char *end = text + out_len;
    if (out_len >= 6 && memcmp(text, "error\n", 6) == 0) {
        return g_strndup(text + 6, out_len - 6);
    }
    if (out_len < 3 || memcmp(text, "ok\n", 3) != 0) {
        return g_strdup("the HLSL compiler gave no answer (is the request well-formed?)");
    }
    const char *p = text + 3;
    g_autofree char *vs_glsl = take_block(&p, end);
    g_autofree char *ps_glsl = vs_glsl ? take_block(&p, end) : NULL;
    if (!ps_glsl) {
        return g_strdup("the HLSL compiler's answer was cut short");
    }

    g_autoptr(GString) log = g_string_new(NULL);
    GLuint vs = separable_program(GL_VERTEX_SHADER, vs_glsl, log);
    GLuint ps = separable_program(GL_FRAGMENT_SHADER, ps_glsl, log);
    if (!vs || !ps) {
        glDeleteProgram(vs);
        glDeleteProgram(ps);
        return g_string_free(g_steal_pointer(&log), false);
    }
    KosGLProgram *program = g_new0(KosGLProgram, 1);
    program->vs = vs;
    program->ps = ps;
    program->uniforms = g_hash_table_new_full(g_str_hash, g_str_equal, g_free, g_free);
    glGenProgramPipelines(1, &program->pipeline);
    glUseProgramStages(program->pipeline, GL_VERTEX_SHADER_BIT, vs);
    glUseProgramStages(program->pipeline, GL_FRAGMENT_SHADER_BIT, ps);
    bind_samplers(vs);
    bind_samplers(ps);
    if (c->current == g_hash_table_lookup(c->programs, GUINT_TO_POINTER(id))) {
        c->current = NULL;
    }
    g_hash_table_replace(c->programs, GUINT_TO_POINTER(id), program);
    return NULL;
}

/* ---- requests ------------------------------------------------------------------ */

typedef struct Reply {
    uint32_t status;
    void *data;
    size_t len;
} Reply;

static void reply_error(Reply *r, char *message)
{
    r->status = STATUS_ERROR;
    r->data = message;
    r->len = strlen(message);
}

static void handle_request(KosGL *s, const KosGLRequest *req, const void *payload, size_t len,
                           size_t room, Reply *r)
{
    KosGLContext *c = g_hash_table_lookup(s->contexts, GUINT_TO_POINTER(req->ctx));

    if (req->op != REQ_CREATE && !c) {
        reply_error(r, g_strdup_printf("no context %u", req->ctx));
        return;
    }
    switch (req->op) {
    case REQ_CREATE: {
        Error *err = NULL;
        if (c) {
            reply_error(r, g_strdup_printf("context %u exists", req->ctx));
        } else if (req->a == 0 || req->b == 0 || req->a > MAX_SIZE || req->b > MAX_SIZE) {
            reply_error(r, g_strdup_printf("bad size %ux%u", req->a, req->b));
        } else {
            c = context_new(s, req->ctx, req->a, req->b, &err);
            if (err) {
                reply_error(r, g_strdup(error_get_pretty(err)));
                error_free(err);
                if (c) {
                    context_free(c);
                }
            } else {
                g_hash_table_insert(s->contexts, GUINT_TO_POINTER(req->ctx), c);
            }
        }
        break;
    }
    case REQ_DESTROY:
        g_hash_table_remove(s->contexts, GUINT_TO_POINTER(req->ctx));
        break;
    case REQ_SUBMIT: {
        CGLSetCurrentContext(c->cgl);
        glBindFramebuffer(GL_FRAMEBUFFER, c->fbo);
        glBindVertexArray(c->vao);
        glBindBuffer(GL_ARRAY_BUFFER, c->vbo);
        char *error = len % 4 ? g_strdup("commands aren't whole words") :
                      run_commands(c, payload, len / 4);
        if (error) {
            reply_error(r, error);
        } else if (req->a) {
            size_t size = (size_t)c->width * c->height * 4;
            if (room < size) {
                reply_error(r, g_strdup("no room for the frame"));
            } else {
                r->data = g_malloc(size);
                r->len = size;
                read_frame(c, r->data);
            }
        }
        CGLSetCurrentContext(NULL);
        break;
    }
    case REQ_HLSL: {
        CGLSetCurrentContext(c->cgl);
        char *error = req->a ? compile_hlsl(s, c, req->a, payload, len) : g_strdup("program 0");
        if (error) {
            reply_error(r, error);
        }
        CGLSetCurrentContext(NULL);
        break;
    }
    default:
        reply_error(r, g_strdup_printf("unknown request %u", req->op));
    }
}

static void kos_gl_handle(VirtIODevice *vdev, VirtQueue *vq)
{
    KosGL *s = KOS_GL(vdev);
    VirtQueueElement *elem;

    while ((elem = virtqueue_pop(vq, sizeof(VirtQueueElement)))) {
        KosGLRequest req;
        KosGLResponse resp;
        size_t out_size = iov_size(elem->out_sg, elem->out_num);
        size_t in_size = iov_size(elem->in_sg, elem->in_num);
        size_t written = 0;

        if (out_size < sizeof req || in_size < sizeof resp) {
            virtio_error(vdev, "kos-gl: request too small");
            virtqueue_detach_element(vq, elem, 0);
            g_free(elem);
            break;
        }
        iov_to_buf(elem->out_sg, elem->out_num, 0, &req, sizeof req);
        size_t len = out_size - sizeof req;
        g_autofree void *payload = g_malloc(MAX(len, 1));
        iov_to_buf(elem->out_sg, elem->out_num, sizeof req, payload, len);

        Reply r = { STATUS_OK, NULL, 0 };
        handle_request(s, &req, payload, len, in_size - sizeof resp, &r);
        if (r.status != STATUS_OK) {
            qemu_log_mask(LOG_GUEST_ERROR, "kos-gl: request %u (context %u): %.*s\n", req.op, req.ctx,
                          (int)r.len, (const char *)r.data);
        }
        size_t data_len = MIN(r.len, in_size - sizeof resp);
        resp.status = r.status;
        resp.len = data_len;
        written += iov_from_buf(elem->in_sg, elem->in_num, 0, &resp, sizeof resp);
        written += iov_from_buf(elem->in_sg, elem->in_num, sizeof resp, r.data, data_len);
        g_free(r.data);

        virtqueue_push(vq, elem, written);
        g_free(elem);
    }
    virtio_notify(vdev, vq);
}

/* ---- the device ---------------------------------------------------------------- */

static uint64_t kos_gl_get_features(VirtIODevice *vdev, uint64_t features, Error **errp)
{
    return features;
}

/* kos-hlslc next to the QEMU executable. */
static char *default_hlslc(void)
{
    char path[PATH_MAX];
    uint32_t size = sizeof path;
    if (_NSGetExecutablePath(path, &size) != 0) {
        return g_strdup("kos-hlslc");
    }
    g_autofree char *dir = g_path_get_dirname(path);
    return g_build_filename(dir, "kos-hlslc", NULL);
}

static void kos_gl_realize(DeviceState *dev, Error **errp)
{
    VirtIODevice *vdev = VIRTIO_DEVICE(dev);
    KosGL *s = KOS_GL(dev);
    CGLPixelFormatAttribute attrs[] = {
        kCGLPFAOpenGLProfile, (CGLPixelFormatAttribute)kCGLOGLPVersion_GL4_Core,
        kCGLPFAAccelerated,
        kCGLPFAAllowOfflineRenderers,
        0,
    };
    GLint count = 0;
    CGLError err = CGLChoosePixelFormat(attrs, &s->format, &count);

    if (err != kCGLNoError || !s->format) {
        error_setg(errp, "kos-gl: no OpenGL 4.1 renderer (%s)", CGLErrorString(err));
        return;
    }
    if (!s->hlslc) {
        s->hlslc = default_hlslc();
    }
    s->contexts = g_hash_table_new_full(NULL, NULL, NULL, context_free);
    virtio_init(vdev, VIRTIO_ID_KOS_GL, 0);
    s->vq = virtio_add_queue(vdev, QUEUE_SIZE, kos_gl_handle);
}

static void kos_gl_unrealize(DeviceState *dev)
{
    VirtIODevice *vdev = VIRTIO_DEVICE(dev);
    KosGL *s = KOS_GL(dev);

    g_hash_table_destroy(s->contexts);
    CGLReleasePixelFormat(s->format);
    virtio_del_queue(vdev, 0);
    virtio_cleanup(vdev);
}

/* A rebooted guest starts with no contexts. */
static void kos_gl_reset(VirtIODevice *vdev)
{
    KosGL *s = KOS_GL(vdev);
    g_hash_table_remove_all(s->contexts);
}

static const VMStateDescription vmstate_kos_gl = {
    .name = "kos-gl",
    .unmigratable = 1,
};

static const Property kos_gl_properties[] = {
    DEFINE_PROP_STRING("hlslc", KosGL, hlslc),
};

static void kos_gl_class_init(ObjectClass *klass, const void *data)
{
    DeviceClass *dc = DEVICE_CLASS(klass);
    VirtioDeviceClass *vdc = VIRTIO_DEVICE_CLASS(klass);

    device_class_set_props(dc, kos_gl_properties);
    dc->vmsd = &vmstate_kos_gl;
    dc->desc = "KOS OpenGL (host OpenGL.framework)";
    set_bit(DEVICE_CATEGORY_DISPLAY, dc->categories);
    vdc->realize = kos_gl_realize;
    vdc->unrealize = kos_gl_unrealize;
    vdc->get_features = kos_gl_get_features;
    vdc->reset = kos_gl_reset;
}

static const TypeInfo kos_gl_info = {
    .name = TYPE_KOS_GL,
    .parent = TYPE_VIRTIO_DEVICE,
    .instance_size = sizeof(KosGL),
    .class_init = kos_gl_class_init,
};

static void kos_gl_register_types(void)
{
    type_register_static(&kos_gl_info);
}

type_init(kos_gl_register_types)
