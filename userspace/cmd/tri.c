#include "../libc/canvas.h"
#include "../libc/gl.h"
#include "../libc/stdio.h"
#include "../libc/sys.h"

#define W 800
#define H 600

static const char* vert_shader =
    "#version 330\n"
    "uniform float uTime;\n"
    "out vec4 vertexColor;\n"
    "layout (location = 0) in vec3 pos;\n"
    "void main() {\n"
    "    vec3 p = pos;\n"
    "    p.xy = mat2(cos(uTime), -sin(uTime), sin(uTime), cos(uTime)) * p.xy;\n"
    "    gl_Position = vec4(p, 1.0);\n"
    "    vertexColor = vec4(gl_VertexID == 0, gl_VertexID == 1, gl_VertexID == "
    "2, 1.0);\n"
    "}\n";

static const char* frag_shader =
    "#version 330\n"
    "uniform float uTime;\n"
    "out vec4 FragColor;\n"
    "in vec4 vertexColor;\n"
    "void main() {\n"
    "    FragColor = vec4(\n"
    "        abs(cos(vertexColor.r + floor(uTime))),\n"
    "        abs(sin(vertexColor.g + (3.14159 / 4) + floor(uTime))),\n"
    "        abs(sin(vertexColor.b + floor(uTime))),\n"
    "        vertexColor.a\n"
    "    );\n"
    "}\n";

static GLuint compile(GLenum type, const char* source) {
  GLuint shader = glCreateShader(type);
  glShaderSource(shader, 1, &source, NULL);
  glCompileShader(shader);
  GLint ok;
  glGetShaderiv(shader, GL_COMPILE_STATUS, &ok);
  if (!ok) {
    char log[1024];
    glGetShaderInfoLog(shader, sizeof log, NULL, log);
    printf("the %s shader didn't compile:\n%s\n",
           type == GL_VERTEX_SHADER ? "vertex" : "fragment", log);
    glDeleteShader(shader);
    return 0;
  }
  return shader;
}

static GLuint link(void) {
  GLuint vs = compile(GL_VERTEX_SHADER, vert_shader);
  GLuint fs = compile(GL_FRAGMENT_SHADER, frag_shader);
  if (!vs || !fs)
    return 0;
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
    printf("the program didn't link:\n%s\n", log);
    glDeleteProgram(program);
    return 0;
  }
  return program;
}

static const float verts[] = {0.0f, 0.5f, 0.0f,  -0.5f, -0.5f,
                              0.0f, 0.5f, -0.5f, 0.0f};

void _start(void) {
  canvas_t c;
  if (canvas_open(&c, "GLSL TRI", W, H) < 0) {
    writeln("Failed to open canvas");
    exit(1);
  }
  if (kgl_open(&c) < 0) {
    printf("%s\n", kgl_error());
    canvas_close(&c);
    exit(1);
  }
  GLuint program = link();
  if (!program) {
    kgl_close();
    canvas_close(&c);
    exit(1);
  }
  unsigned int VBO, VAO;
  glGenBuffers(1, &VBO);
  glBindBuffer(GL_ARRAY_BUFFER, VBO);
  glBufferData(GL_ARRAY_BUFFER, sizeof(verts), verts, GL_STATIC_DRAW);
  glGenVertexArrays(1, &VAO);
  glBindVertexArray(VAO);
  glBindBuffer(GL_ARRAY_BUFFER, VBO);
  glVertexAttribPointer(0, 3, GL_FLOAT, GL_FALSE, 3 * sizeof(float), (void*)0);
  glEnableVertexAttribArray(0);
  GLint time_at = glGetUniformLocation(program, "uTime");
  uint64_t start = uptime_ms();
  int wireframe = 0;
  for (;;) {
    canvas_event_t e;
    while (canvas_poll(&c, &e) > 0) {
      if (e.kind == CANVAS_CLOSED || (e.kind == CANVAS_KEY && e.key == 27)) {
        kgl_close();
        canvas_close(&c);
        exit(0);
      }
      if (e.kind == CANVAS_KEY && e.key == ' ')
        wireframe = !wireframe;
    }
    glClearColor(0.1f, 0.1f, 0.15f, 1.0f);
    glClear(GL_COLOR_BUFFER_BIT);
    glPolygonMode(GL_FRONT_AND_BACK, wireframe ? GL_LINE : GL_FILL);
    glUseProgram(program);
    glUniform1f(time_at, (uptime_ms() - start) / 1000.0f);
    glBindVertexArray(VAO);
    glDrawArrays(GL_TRIANGLES, 0, 3);
    if (kgl_swap() < 0) {
      printf("%s\n", kgl_error());
      break;
    }
    sleep_ms(10);
  }
  kgl_close();
  canvas_close(&c);
  exit(1);
}
