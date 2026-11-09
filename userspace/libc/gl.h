#ifndef C_GL_H
#define C_GL_H

// OpenGL for canvases, drawn by the Mac's GPU, with GLSL and HLSL shaders.
//
// It's the classic OpenGL 1.x API (glBegin/glEnd, vertex arrays, the
// matrix stack, textures, lights, fog), plus shaders written in GLSL
// (OpenGL's own shader language) or HLSL (Direct3D's).
// Drawing needs KOS's own QEMU (tools/qemu, with its kos-gl device): run
// KOS from KOS.app, or `make qemu` and then `make run-gui`.
// Elsewhere kgl_open fails.
//
//     canvas_t c;
//     canvas_open(&c, "Triangle", 640, 480);
//     if (kgl_open(&c) < 0) { printf("%s\n", kgl_error()); exit(1); }
//     for (;;) {
//         ... canvas_poll(&c, &e) ...
//         glClear(GL_COLOR_BUFFER_BIT);
//         glBegin(GL_TRIANGLES);
//         glColor3f(1, 0, 0); glVertex2f(-0.5f, -0.5f);
//         glColor3f(0, 1, 0); glVertex2f(0.5f, -0.5f);
//         glColor3f(0, 0, 1); glVertex2f(0, 0.5f);
//         glEnd();
//         kgl_swap();                  // shows the frame in the canvas
//     }
//
// ---- GLSL ----
//
// GLSL shaders are compiled by the Mac's OpenGL as GLSL 4.10 (the default
// if the source has no #version; 150 to 410 work). Make a program with
// OpenGL's glCreateShader, glShaderSource, glCompileShader,
// glCreateProgram, glAttachShader and glLinkProgram, or in one call with
// kgl_glsl_program. glUseProgram it, and every draw runs through it
// instead of the lights, fog and textures below.
//
// - Vertex inputs come from glVertex, glColor, glNormal and glTexCoord
//   (or the arrays): `in vec4 kos_position` (w is 1 unless you give one),
//   `in vec4 kos_color`, `in vec3 kos_normal` and `in vec2 kos_texcoord`,
//   or any names with layout(location = 0 to 3) in that order.
// - The vertex shader's outputs and the fragment shader's inputs are
//   matched by name. Write gl_Position, and one `out vec4` color.
// - Uniforms are set with glGetUniformLocation and glUniform*; matrices
//   are column-major, as OpenGL's always are: kos_mvp * kos_position.
// - Declare uniform mat4 kos_mvp (projection * modelview), kos_modelview
//   or kos_projection and they're set from the matrix stack at each draw.
// - A sampler2D samples texture unit 0, or the unit given with
//   glUniform1i(glGetUniformLocation(p, "name"), unit).
// - Compile errors are in glGetShaderInfoLog (or kgl_error).
//
// See userspace/prog/glsl_demo.c.
//
// ---- HLSL ----
//
// kgl_hlsl_program compiles a vertex and a pixel shader (on the Mac, so
// the first compile takes a moment) into a program for glUseProgram.
// While it's in use, every draw runs through it instead of the lights,
// fog and textures below.
//
// - Vertex inputs come from glVertex, glColor, glNormal and glTexCoord
//   (or the arrays): semantics POSITION (float4: w is 1 unless you give
//   one), COLOR (float4), NORMAL (float3) and TEXCOORD0 (float2).
// - The vertex shader's output and the pixel shader's input are matched
//   by semantic, as in Direct3D. Output SV_Position, and SV_Target from
//   the pixel shader.
// - Globals (and cbuffer members) are uniforms: glGetUniformLocation(p,
//   "time") then glUniform1f(loc, t). A matrix given to glUniformMatrix4fv
//   is in OpenGL's column-major order; multiply with mul(m, v).
// - Declare float4x4 kos_mvp (projection * modelview), kos_modelview or
//   kos_projection and they're set from the matrix stack at each draw.
// - Texture2D t : register(t1) samples the texture bound to texture
//   unit 1 (glActiveTexture(GL_TEXTURE1), glBindTexture). Texture
//   coordinates (0, 0) are the first pixel given to glTexImage2D.
// - Clip space is OpenGL's: z goes from -w to w (Direct3D-style
//   projections, with 0 to w, still work).
//
// See userspace/prog/hlsl_demo.c.
//
// ---- what's here ----
//
// No geometry shaders, and no buffer objects (draws send their vertices
// each time). One context at a time (kgl_open), drawing to one canvas. Not here:
// display lists, glTexSubImage2D, line widths, point sizes, stencils and
// the texture matrix. Up to 4 lights (GL_LIGHT0-3), Gouraud-shaded,
// without attenuation or spotlights; glTexEnv is always GL_MODULATE.

#include <stddef.h>
#include <stdint.h>
#include "canvas.h"

typedef unsigned int GLenum;
typedef unsigned int GLbitfield;
typedef unsigned int GLuint;
typedef int GLint;
typedef int GLsizei;
typedef unsigned char GLboolean;
typedef unsigned char GLubyte;
typedef signed char GLbyte;
typedef unsigned short GLushort;
typedef short GLshort;
typedef float GLfloat;
typedef float GLclampf;
typedef double GLdouble;
typedef double GLclampd;
typedef void GLvoid;
typedef char GLchar;

#define GL_FALSE 0
#define GL_TRUE 1

// glGetError
#define GL_NO_ERROR          0
#define GL_INVALID_ENUM      0x0500
#define GL_INVALID_VALUE     0x0501
#define GL_INVALID_OPERATION 0x0502
#define GL_STACK_OVERFLOW    0x0503
#define GL_STACK_UNDERFLOW   0x0504
#define GL_OUT_OF_MEMORY     0x0505

// glBegin and glDrawArrays
#define GL_POINTS         0x0000
#define GL_LINES          0x0001
#define GL_LINE_LOOP      0x0002
#define GL_LINE_STRIP     0x0003
#define GL_TRIANGLES      0x0004
#define GL_TRIANGLE_STRIP 0x0005
#define GL_TRIANGLE_FAN   0x0006
#define GL_QUADS          0x0007
#define GL_QUAD_STRIP     0x0008
#define GL_POLYGON        0x0009

// glClear
#define GL_DEPTH_BUFFER_BIT   0x00000100
#define GL_STENCIL_BUFFER_BIT 0x00000400
#define GL_COLOR_BUFFER_BIT   0x00004000

// glEnable
#define GL_CULL_FACE           0x0B44
#define GL_LIGHTING            0x0B50
#define GL_COLOR_MATERIAL      0x0B57
#define GL_FOG                 0x0B60
#define GL_DEPTH_TEST          0x0B71
#define GL_NORMALIZE           0x0BA1
#define GL_ALPHA_TEST          0x0BC0
#define GL_BLEND               0x0BE2
#define GL_SCISSOR_TEST        0x0C11
#define GL_TEXTURE_2D          0x0DE1
#define GL_POLYGON_OFFSET_FILL 0x8037
#define GL_RESCALE_NORMAL      0x803A
#define GL_LIGHT0              0x4000
#define GL_LIGHT1              0x4001
#define GL_LIGHT2              0x4002
#define GL_LIGHT3              0x4003

// glDepthFunc and glAlphaFunc
#define GL_NEVER    0x0200
#define GL_LESS     0x0201
#define GL_EQUAL    0x0202
#define GL_LEQUAL   0x0203
#define GL_GREATER  0x0204
#define GL_NOTEQUAL 0x0205
#define GL_GEQUAL   0x0206
#define GL_ALWAYS   0x0207

// glBlendFunc
#define GL_ZERO                0
#define GL_ONE                 1
#define GL_SRC_COLOR           0x0300
#define GL_ONE_MINUS_SRC_COLOR 0x0301
#define GL_SRC_ALPHA           0x0302
#define GL_ONE_MINUS_SRC_ALPHA 0x0303
#define GL_DST_ALPHA           0x0304
#define GL_ONE_MINUS_DST_ALPHA 0x0305
#define GL_DST_COLOR           0x0306
#define GL_ONE_MINUS_DST_COLOR 0x0307

// glCullFace and glFrontFace
#define GL_FRONT          0x0404
#define GL_BACK           0x0405
#define GL_FRONT_AND_BACK 0x0408
#define GL_CW             0x0900
#define GL_CCW            0x0901

// glMatrixMode
#define GL_MODELVIEW  0x1700
#define GL_PROJECTION 0x1701
#define GL_TEXTURE    0x1702

// glShadeModel
#define GL_FLAT   0x1D00
#define GL_SMOOTH 0x1D01

// Lights and materials
#define GL_AMBIENT               0x1200
#define GL_DIFFUSE               0x1201
#define GL_SPECULAR              0x1202
#define GL_POSITION              0x1203
#define GL_EMISSION              0x1600
#define GL_SHININESS             0x1601
#define GL_AMBIENT_AND_DIFFUSE   0x1602
#define GL_LIGHT_MODEL_AMBIENT   0x0B53

// Fog
#define GL_FOG_DENSITY 0x0B62
#define GL_FOG_START   0x0B63
#define GL_FOG_END     0x0B64
#define GL_FOG_MODE    0x0B65
#define GL_FOG_COLOR   0x0B66
#define GL_EXP         0x0800
#define GL_EXP2        0x0801
#define GL_LINEAR      0x2601

// Vertex arrays
#define GL_VERTEX_ARRAY        0x8074
#define GL_NORMAL_ARRAY        0x8075
#define GL_COLOR_ARRAY         0x8076
#define GL_TEXTURE_COORD_ARRAY 0x8078

// Types
#define GL_BYTE           0x1400
#define GL_UNSIGNED_BYTE  0x1401
#define GL_SHORT          0x1402
#define GL_UNSIGNED_SHORT 0x1403
#define GL_INT            0x1404
#define GL_UNSIGNED_INT   0x1405
#define GL_FLOAT          0x1406
#define GL_UNSIGNED_INT_8_8_8_8_REV 0x8367

// Textures
#define GL_RGB                  0x1907
#define GL_RGBA                 0x1908
#define GL_BGRA                 0x80E1
#define GL_NEAREST              0x2600
#define GL_NEAREST_MIPMAP_NEAREST 0x2700
#define GL_LINEAR_MIPMAP_NEAREST  0x2701
#define GL_NEAREST_MIPMAP_LINEAR  0x2702
#define GL_LINEAR_MIPMAP_LINEAR   0x2703
#define GL_TEXTURE_MAG_FILTER   0x2800
#define GL_TEXTURE_MIN_FILTER   0x2801
#define GL_TEXTURE_WRAP_S       0x2802
#define GL_TEXTURE_WRAP_T       0x2803
#define GL_REPEAT               0x2901
#define GL_CLAMP_TO_EDGE        0x812F
#define GL_MIRRORED_REPEAT      0x8370
#define GL_TEXTURE0             0x84C0
#define GL_TEXTURE1             0x84C1
#define GL_TEXTURE2             0x84C2
#define GL_TEXTURE3             0x84C3

// glGetString
#define GL_VENDOR     0x1F00
#define GL_RENDERER   0x1F01
#define GL_VERSION    0x1F02
#define GL_SHADING_LANGUAGE_VERSION 0x8B8C

// Shaders and programs
#define GL_FRAGMENT_SHADER      0x8B30
#define GL_VERTEX_SHADER        0x8B31
#define GL_SHADER_TYPE          0x8B4F
#define GL_DELETE_STATUS        0x8B80
#define GL_COMPILE_STATUS       0x8B81
#define GL_LINK_STATUS          0x8B82
#define GL_INFO_LOG_LENGTH      0x8B84
#define GL_ATTACHED_SHADERS     0x8B85
#define GL_SHADER_SOURCE_LENGTH 0x8B88

// ---- KOS ---------------------------------------------------------------------------

// 1 if this QEMU can draw for gl.h (it has the kos-gl device), else 0.
int kgl_available(void);

// Starts drawing into canvas c, which must stay open, with a context the
// canvas's size. Returns 0, or -1 (see kgl_error) if there's no GPU or
// it's in use by another context of this program.
int kgl_open(canvas_t* c);

// Sends what's been drawn to the GPU, waits for it, and shows the frame
// in the canvas (it calls canvas_present). Returns 0, or -1 (see
// kgl_error) if the GPU rejected something.
int kgl_swap(void);

// Ends drawing, freeing the context's textures and programs.
void kgl_close(void);

// Compiles HLSL into a program for glUseProgram: the vertex shader is the
// function vs_entry in vs_source, and the pixel shader ps_entry in
// ps_source (the same source twice is fine). Returns the program, or 0
// with the compiler's messages in kgl_error().
GLuint kgl_hlsl_program(const char* vs_source, const char* vs_entry, const char* ps_source, const char* ps_entry);

// Compiles and links GLSL into a program for glUseProgram, in one go.
// Returns the program, or 0 with OpenGL's messages in kgl_error().
GLuint kgl_glsl_program(const char* vs_source, const char* fs_source);

// Why the last kgl_ call failed (or the compiler's messages).
const char* kgl_error(void);

// ---- state ---------------------------------------------------------------------------

GLenum glGetError(void);
const GLubyte* glGetString(GLenum name);
void glEnable(GLenum cap);
void glDisable(GLenum cap);
GLboolean glIsEnabled(GLenum cap);
void glClearColor(GLclampf r, GLclampf g, GLclampf b, GLclampf a);
void glClearDepth(GLclampd depth);
void glClearDepthf(GLclampf depth);
void glClear(GLbitfield mask);
void glViewport(GLint x, GLint y, GLsizei width, GLsizei height);
void glScissor(GLint x, GLint y, GLsizei width, GLsizei height);
void glBlendFunc(GLenum src, GLenum dst);
void glDepthFunc(GLenum func);
void glDepthMask(GLboolean flag);
void glColorMask(GLboolean r, GLboolean g, GLboolean b, GLboolean a);
void glCullFace(GLenum mode);
void glFrontFace(GLenum mode);
void glPolygonOffset(GLfloat factor, GLfloat units);
void glAlphaFunc(GLenum func, GLclampf ref);
// Shading is always smooth; these do nothing.
void glShadeModel(GLenum mode);
void glHint(GLenum target, GLenum mode);
// Send what's been drawn to the GPU; kgl_swap does this too.
void glFlush(void);
void glFinish(void);

// ---- matrices ------------------------------------------------------------------------

void glMatrixMode(GLenum mode);
void glLoadIdentity(void);
void glLoadMatrixf(const GLfloat* m);
void glMultMatrixf(const GLfloat* m);
void glPushMatrix(void);
void glPopMatrix(void);
void glTranslatef(GLfloat x, GLfloat y, GLfloat z);
void glRotatef(GLfloat degrees, GLfloat x, GLfloat y, GLfloat z);
void glScalef(GLfloat x, GLfloat y, GLfloat z);
void glOrtho(GLdouble left, GLdouble right, GLdouble bottom, GLdouble top, GLdouble near, GLdouble far);
void glFrustum(GLdouble left, GLdouble right, GLdouble bottom, GLdouble top, GLdouble near, GLdouble far);
void gluPerspective(GLdouble fovy_degrees, GLdouble aspect, GLdouble near, GLdouble far);
void gluLookAt(GLdouble eye_x, GLdouble eye_y, GLdouble eye_z, GLdouble at_x, GLdouble at_y, GLdouble at_z,
               GLdouble up_x, GLdouble up_y, GLdouble up_z);
void gluOrtho2D(GLdouble left, GLdouble right, GLdouble bottom, GLdouble top);

// ---- vertices ------------------------------------------------------------------------

void glBegin(GLenum mode);
void glEnd(void);
void glVertex2f(GLfloat x, GLfloat y);
void glVertex3f(GLfloat x, GLfloat y, GLfloat z);
void glVertex4f(GLfloat x, GLfloat y, GLfloat z, GLfloat w);
void glVertex2fv(const GLfloat* v);
void glVertex3fv(const GLfloat* v);
void glColor3f(GLfloat r, GLfloat g, GLfloat b);
void glColor4f(GLfloat r, GLfloat g, GLfloat b, GLfloat a);
void glColor3ub(GLubyte r, GLubyte g, GLubyte b);
void glColor4ub(GLubyte r, GLubyte g, GLubyte b, GLubyte a);
void glColor3fv(const GLfloat* v);
void glColor4fv(const GLfloat* v);
void glNormal3f(GLfloat x, GLfloat y, GLfloat z);
void glNormal3fv(const GLfloat* v);
void glTexCoord2f(GLfloat s, GLfloat t);
void glTexCoord2fv(const GLfloat* v);

// Arrays: positions, normals and texture coordinates are GL_FLOAT; colors
// GL_FLOAT or GL_UNSIGNED_BYTE.
void glEnableClientState(GLenum array);
void glDisableClientState(GLenum array);
void glVertexPointer(GLint size, GLenum type, GLsizei stride, const GLvoid* pointer);
void glColorPointer(GLint size, GLenum type, GLsizei stride, const GLvoid* pointer);
void glNormalPointer(GLenum type, GLsizei stride, const GLvoid* pointer);
void glTexCoordPointer(GLint size, GLenum type, GLsizei stride, const GLvoid* pointer);
void glDrawArrays(GLenum mode, GLint first, GLsizei count);
void glDrawElements(GLenum mode, GLsizei count, GLenum type, const GLvoid* indices);

// ---- textures --------------------------------------------------------------------------

void glGenTextures(GLsizei n, GLuint* textures);
void glDeleteTextures(GLsizei n, const GLuint* textures);
void glActiveTexture(GLenum unit);
void glBindTexture(GLenum target, GLuint texture);
// Formats: GL_RGBA or GL_RGB with GL_UNSIGNED_BYTE (bytes R, G, B[, A]),
// or GL_BGRA with GL_UNSIGNED_INT_8_8_8_8_REV (uint32_t 0xAARRGGBB, like
// rgb() but with alpha). Rows go top to bottom. Up to 4096 x 4096.
void glTexImage2D(GLenum target, GLint level, GLint internal_format, GLsizei width, GLsizei height,
                  GLint border, GLenum format, GLenum type, const GLvoid* pixels);
// Textures start GL_LINEAR, without mipmaps, and GL_REPEAT.
void glTexParameteri(GLenum target, GLenum pname, GLint value);
void glGenerateMipmap(GLenum target);

// ---- lights, materials and fog -----------------------------------------------------------

void glLightfv(GLenum light, GLenum pname, const GLfloat* params);
void glLightModelfv(GLenum pname, const GLfloat* params);
void glMaterialfv(GLenum face, GLenum pname, const GLfloat* params);
void glMaterialf(GLenum face, GLenum pname, GLfloat param);
// glColorMaterial's settings are fixed: with GL_COLOR_MATERIAL enabled,
// glColor sets the ambient and diffuse colors.
void glColorMaterial(GLenum face, GLenum mode);
void glFogi(GLenum pname, GLint param);
void glFogf(GLenum pname, GLfloat param);
void glFogfv(GLenum pname, const GLfloat* params);

// ---- programs (see GLSL and HLSL at the top) ------------------------------------------------

// glAttachShader takes a copy of the shader's source as it is then.
GLuint glCreateShader(GLenum type);
void glDeleteShader(GLuint shader);
void glShaderSource(GLuint shader, GLsizei count, const GLchar* const* string, const GLint* length);
void glCompileShader(GLuint shader);
void glGetShaderiv(GLuint shader, GLenum pname, GLint* params);
void glGetShaderInfoLog(GLuint shader, GLsizei max, GLsizei* length, GLchar* log);
GLuint glCreateProgram(void);
void glAttachShader(GLuint program, GLuint shader);
void glDetachShader(GLuint program, GLuint shader);
void glLinkProgram(GLuint program);
void glGetProgramiv(GLuint program, GLenum pname, GLint* params);
void glGetProgramInfoLog(GLuint program, GLsizei max, GLsizei* length, GLchar* log);

void glUseProgram(GLuint program);
void glDeleteProgram(GLuint program);
GLint glGetUniformLocation(GLuint program, const char* name);
void glUniform1f(GLint location, GLfloat x);
void glUniform2f(GLint location, GLfloat x, GLfloat y);
void glUniform3f(GLint location, GLfloat x, GLfloat y, GLfloat z);
void glUniform4f(GLint location, GLfloat x, GLfloat y, GLfloat z, GLfloat w);
void glUniform1i(GLint location, GLint x);
void glUniform2i(GLint location, GLint x, GLint y);
void glUniform3i(GLint location, GLint x, GLint y, GLint z);
void glUniform4i(GLint location, GLint x, GLint y, GLint z, GLint w);
void glUniform1fv(GLint location, GLsizei count, const GLfloat* v);
void glUniform2fv(GLint location, GLsizei count, const GLfloat* v);
void glUniform3fv(GLint location, GLsizei count, const GLfloat* v);
void glUniform4fv(GLint location, GLsizei count, const GLfloat* v);
void glUniform1iv(GLint location, GLsizei count, const GLint* v);
void glUniformMatrix3fv(GLint location, GLsizei count, GLboolean transpose, const GLfloat* m);
void glUniformMatrix4fv(GLint location, GLsizei count, GLboolean transpose, const GLfloat* m);

#endif //C_GL_H
