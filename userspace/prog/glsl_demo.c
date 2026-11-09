// GLSL on the Mac's GPU: a spinning, lit, textured cube drawn with GLSL
// shaders, made with OpenGL's own shader calls, above a floor drawn with
// plain OpenGL (lights and fog). The same scene as hlsl_demo.
// Needs KOS's own QEMU (see gl.h). Keys: space pauses, q or Escape quits.
#include "../libc/sys.h"
#include "../libc/stdio.h"
#include "../libc/canvas.h"
#include "../libc/gl.h"

#define W 640
#define H 480

static const char* vertex_shader =
    "#version 410\n"
    "uniform mat4 kos_mvp;\n"
    "uniform mat4 kos_modelview;\n"
    "in vec4 kos_position;\n"
    "in vec3 kos_normal;\n"
    "in vec2 kos_texcoord;\n"
    "out vec2 uv;\n"
    "out vec3 normal;\n"
    "\n"
    "void main() {\n"
    "    gl_Position = kos_mvp * kos_position;\n"
    "    uv = kos_texcoord;\n"
    "    normal = mat3(kos_modelview) * kos_normal;\n"
    "}\n";

static const char* fragment_shader =
    "#version 410\n"
    "uniform float time;\n"
    "uniform sampler2D checker;\n"
    "in vec2 uv;\n"
    "in vec3 normal;\n"
    "out vec4 color;\n"
    "\n"
    "void main() {\n"
    "    vec3 n = normalize(normal);\n"
    "    float light = 0.25 + 0.75 * clamp(dot(n, normalize(vec3(0.4, 0.8, 0.6))), 0.0, 1.0);\n"
    "    vec3 tex = texture(checker, uv).rgb;\n"
    "    vec3 glow = 0.5 + 0.5 * cos(time + uv.xyx * 3.0 + vec3(0, 2, 4));\n"
    "    color = vec4(tex * glow * light, 1);\n"
    "}\n";

// Compiles one shader, or prints why it didn't and returns 0.
static GLuint compile(GLenum type, const char* source) {
    GLuint shader = glCreateShader(type);
    glShaderSource(shader, 1, &source, NULL);
    glCompileShader(shader);
    GLint ok;
    glGetShaderiv(shader, GL_COMPILE_STATUS, &ok);
    if (!ok) {
        char log[1024];
        glGetShaderInfoLog(shader, sizeof log, NULL, log);
        printf("glsl_demo: the %s shader didn't compile:\n%s\n", type == GL_VERTEX_SHADER ? "vertex" : "fragment", log);
        glDeleteShader(shader);
        return 0;
    }
    return shader;
}

static GLuint link(void) {
    GLuint vs = compile(GL_VERTEX_SHADER, vertex_shader);
    GLuint fs = compile(GL_FRAGMENT_SHADER, fragment_shader);
    if (!vs || !fs) return 0;
    GLuint program = glCreateProgram();
    glAttachShader(program, vs);
    glAttachShader(program, fs);
    glLinkProgram(program);
    glDeleteShader(vs);
    glDeleteShader(fs);
    GLint ok;
    glGetProgramiv(program, GL_LINK_STATUS, &ok);
    if (!ok) {
        char log[1024];
        glGetProgramInfoLog(program, sizeof log, NULL, log);
        printf("glsl_demo: the program didn't link:\n%s\n", log);
        glDeleteProgram(program);
        return 0;
    }
    return program;
}

// A cube's faces: normal, then four corners (counter-clockwise from outside).
static const float faces[6][5][3] = {
    { { 0, 0, 1 },  { -1, -1, 1 }, { 1, -1, 1 },  { 1, 1, 1 },   { -1, 1, 1 } },
    { { 0, 0, -1 }, { 1, -1, -1 }, { -1, -1, -1 }, { -1, 1, -1 }, { 1, 1, -1 } },
    { { 1, 0, 0 },  { 1, -1, 1 },  { 1, -1, -1 }, { 1, 1, -1 },  { 1, 1, 1 } },
    { { -1, 0, 0 }, { -1, -1, -1 }, { -1, -1, 1 }, { -1, 1, 1 },  { -1, 1, -1 } },
    { { 0, 1, 0 },  { -1, 1, 1 },  { 1, 1, 1 },   { 1, 1, -1 },  { -1, 1, -1 } },
    { { 0, -1, 0 }, { -1, -1, -1 }, { 1, -1, -1 }, { 1, -1, 1 },  { -1, -1, 1 } },
};
static const float corner_uv[4][2] = { { 0, 1 }, { 1, 1 }, { 1, 0 }, { 0, 0 } };

static void cube(void) {
    glBegin(GL_QUADS);
    for (int f = 0; f < 6; f++) {
        glNormal3fv(faces[f][0]);
        for (int k = 0; k < 4; k++) {
            glTexCoord2fv(corner_uv[k]);
            glVertex3fv(faces[f][1 + k]);
        }
    }
    glEnd();
}

static GLuint checker_texture(void) {
    static uint32_t pixels[64 * 64];
    for (int y = 0; y < 64; y++)
        for (int x = 0; x < 64; x++)
            pixels[y * 64 + x] = ((x / 8 + y / 8) % 2) ? 0xffffffff : 0xff303040;
    GLuint tex;
    glGenTextures(1, &tex);
    glBindTexture(GL_TEXTURE_2D, tex);
    glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA, 64, 64, 0, GL_BGRA, GL_UNSIGNED_INT_8_8_8_8_REV, pixels);
    glGenerateMipmap(GL_TEXTURE_2D);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR_MIPMAP_LINEAR);
    return tex;
}

// A floor of tiles, lit and fogged by plain OpenGL.
static void floor_tiles(void) {
    glNormal3f(0, 1, 0);
    glBegin(GL_QUADS);
    for (int z = -10; z < 10; z++)
        for (int x = -10; x < 10; x++) {
            if ((x + z) & 1) glColor3f(0.3f, 0.5f, 0.3f); else glColor3f(0.2f, 0.35f, 0.2f);
            glVertex3f((float)x, -2, (float)z);
            glVertex3f((float)x, -2, (float)z + 1);
            glVertex3f((float)x + 1, -2, (float)z + 1);
            glVertex3f((float)x + 1, -2, (float)z);
        }
    glEnd();
}

void _start(void) {
    canvas_t c;
    if (canvas_open(&c, "GLSL demo", W, H) < 0) {
        writeln("glsl_demo: can't open a canvas");
        exit(1);
    }
    if (kgl_open(&c) < 0) {
        printf("glsl_demo: %s\n", kgl_error());
        canvas_close(&c);
        exit(1);
    }
    GLuint program = link();
    if (!program) {
        kgl_close();
        canvas_close(&c);
        exit(1);
    }
    GLint time_at = glGetUniformLocation(program, "time");
    GLuint tex = checker_texture();

    glEnable(GL_DEPTH_TEST);
    glClearColor(0.55f, 0.7f, 0.9f, 1);
    float sun[4] = { 0.3f, 1, 0.5f, 0 };
    float fog_color[4] = { 0.55f, 0.7f, 0.9f, 1 };
    glFogi(GL_FOG_MODE, GL_LINEAR);
    glFogf(GL_FOG_START, 6);
    glFogf(GL_FOG_END, 14);
    glFogfv(GL_FOG_COLOR, fog_color);

    float angle = 0, t = 0;
    int paused = 0, frames = 0;
    uint64_t started = uptime_ms();
    for (;;) {
        canvas_event_t e;
        while (canvas_poll(&c, &e) > 0) {
            if (e.kind == CANVAS_CLOSED || (e.kind == CANVAS_KEY && (e.key == 'q' || e.key == 27))) {
                uint64_t ms = uptime_ms() - started;
                printf("glsl_demo: %d frames in %lu ms\n", frames, (unsigned long)ms);
                kgl_close();
                canvas_close(&c);
                exit(0);
            }
            if (e.kind == CANVAS_KEY && e.key == ' ') paused = !paused;
        }
        if (!paused) {
            angle += 1.2f;
            t += 0.03f;
        }

        glClear(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT);
        glMatrixMode(GL_PROJECTION);
        glLoadIdentity();
        gluPerspective(60, (double)W / H, 0.1, 50);
        glMatrixMode(GL_MODELVIEW);
        glLoadIdentity();
        gluLookAt(0, 1.5, 6, 0, 0, 0, 0, 1, 0);

        // The floor, the old way.
        glUseProgram(0);
        glLightfv(GL_LIGHT0, GL_POSITION, sun);
        glEnable(GL_LIGHTING);
        glEnable(GL_LIGHT0);
        glEnable(GL_COLOR_MATERIAL);
        glEnable(GL_FOG);
        floor_tiles();
        glDisable(GL_LIGHTING);
        glDisable(GL_FOG);

        // The cube, in GLSL.
        glPushMatrix();
        glRotatef(angle, 0.3f, 1, 0.2f);
        glUseProgram(program);
        glUniform1f(time_at, t);
        glActiveTexture(GL_TEXTURE0);
        glBindTexture(GL_TEXTURE_2D, tex);
        cube();
        glPopMatrix();

        if (kgl_swap() < 0) {
            printf("glsl_demo: %s\n", kgl_error());
            break;
        }
        frames++;
        sleep_ms(10);
    }
    kgl_close();
    canvas_close(&c);
    exit(1);
}
