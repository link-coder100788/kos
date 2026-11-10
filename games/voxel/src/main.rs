#![no_std]
#![no_main]
#![feature(c_size_t)]
#![allow(unsafe_op_in_unsafe_fn)]
#![allow(warnings)]

extern crate alloc;

use core::{alloc::GlobalAlloc, mem::offset_of, panic::PanicInfo};
use alloc::vec::Vec;
use glam::{Mat4, Vec3};

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    unsafe { exit(1); }
}

unsafe extern "C"
{
    fn writeln(str: *const core::ffi::c_char);
    fn malloc(size: usize) -> *mut core::ffi::c_void;
    fn free(ptr: *mut core::ffi::c_void);
}

struct Allocator;

unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        let size = layout.size();
        malloc(size) as *mut u8
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: core::alloc::Layout) {
        free(ptr as *mut core::ffi::c_void);
    }
}

#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

type GLenum = core::ffi::c_uint;
type GLbitfield = core::ffi::c_uint;
type GLuint = core::ffi::c_uint;
type GLint = core::ffi::c_int;
type GLsizei = core::ffi::c_int;
type GLboolean = core::ffi::c_uchar;
type GLubyte = core::ffi::c_uchar;
type GLbyte = core::ffi::c_char;
type GLushort = core::ffi::c_ushort;
type GLshort = core::ffi::c_short;
type GLfloat = core::ffi::c_float;
type GLclampf = core::ffi::c_float;
type GLdouble = core::ffi::c_double;
type GLclampd = core::ffi::c_double;
type GLvoid = core::ffi::c_void;
type GLchar = core::ffi::c_char;
type GLsizeiptr = core::ffi::c_ptrdiff_t;
type GLintptr = core::ffi::c_ptrdiff_t;

const GL_FALSE: GLboolean = 0;
const GL_TRUE: u32 = 1;
const GL_TRIANGLES: u32 = 0x0004;
const GL_UNSIGNED_INT: u32 = 0x1405;
const GL_FLOAT: u32 = 0x1406;
const GL_ARRAY_BUFFER: u32 = 0x8892;
const GL_ELEMENT_ARRAY_BUFFER: u32 = 0x8893;
const GL_STATIC_DRAW: u32 = 0x88E4;
const GL_DEPTH_TEST: GLenum = 0x0B71;
const GL_COLOR_BUFFER_BIT: GLbitfield = 0x00004000;
const GL_DEPTH_BUFFER_BIT: GLbitfield = 0x00000100;
const GL_VERTEX_SHADER: GLenum = 0x8B31;
const GL_FRAGMENT_SHADER: GLenum = 0x8B30;

const VERT_SHADER: &core::ffi::CStr = cr#"
#version 330 core
layout (location = 0) in vec3 aPosition;
layout (location = 1) in vec2 aTexCoords;
uniform mat4 uMVP;
out vec2 TexCoords;
void main() {
    TexCoords = aTexCoords;
    gl_Position = uMVP * vec4(aPosition, 1.0);
}
"#;

const FRAG_SHADER: &core::ffi::CStr = cr#"
#version 330 core
in vec2 TexCoords;
out vec4 FragColor;
void main() {
    FragColor = vec4(0.35, 0.75, 0.3, 1.0);
}
"#;

unsafe extern "C"
{
    fn glGenBuffers(n: GLsizei, arrays: *mut GLuint);
    fn glBindBuffer(target: GLenum, buffer: GLuint);
    fn glBufferData(target: GLenum, size: GLsizeiptr, data: *const GLvoid, usage: GLenum);
    fn glGenVertexArrays(n: GLsizei, arrays: *mut GLuint);
    fn glBindVertexArray(array: GLuint);
    fn glVertexAttribPointer(index: GLuint, size: GLint, type_e: GLenum, normalized: GLboolean, stride: GLsizei, pointer: *const GLvoid);
    fn glEnableVertexAttribArray(index: GLuint);

    fn glDeleteBuffers(n: GLsizei, buffers: *const GLuint);
    fn glDeleteVertexArrays(n: GLsizei, arrays: *const GLuint);

    fn glDrawElements(mode: GLenum, count: GLsizei, type_e: GLenum, indices: *const GLvoid);

    fn glEnable(cap: GLenum);
    fn glClear(mask: GLbitfield);
    fn glClearColor(r: GLfloat, g: GLfloat, b: GLfloat, a: GLfloat);
    fn glViewport(x: GLint, y: GLint, width: GLsizei, height: GLsizei);

    fn glUseProgram(program: GLuint);
    fn glDeleteProgram(program: GLuint);
    fn glGetUniformLocation(program: GLuint, name: *const GLchar) -> GLint;
    fn glUniformMatrix4fv(location: GLint, count: GLsizei, transpose: GLboolean, value: *const GLfloat);
}

const CHUNK_SIZE: usize = 16;
const CHUNK_VOLUME: usize = CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Block {
    Air = 0,
    Grass = 1,
    Dirt = 2,
    Stone = 3,
}

struct Chunk {
    blocks: [Block; CHUNK_VOLUME],
}

impl Chunk {
    pub fn new() -> Self {
        Self { blocks: [Block::Air; CHUNK_VOLUME] }
    }

    fn index(x: usize, y: usize, z: usize) -> usize {
        x + CHUNK_SIZE * (y + CHUNK_SIZE * z)
    }

    pub fn get(&self, x: usize, y: usize, z: usize) -> Block {
        if x >= CHUNK_SIZE || y >= CHUNK_SIZE || z >= CHUNK_SIZE { return Block::Air; }
        self.blocks[Self::index(x, y, z)]
    }

    pub fn set(&mut self, x: usize, y: usize, z: usize, block: Block) {
        if x >= CHUNK_SIZE || y >= CHUNK_SIZE || z >= CHUNK_SIZE { return; }
        self.blocks[Self::index(x, y, z)] = block;
    }
}

fn generate_chunk() -> Chunk {
    let mut chunk = Chunk::new();
    for x in 0..CHUNK_SIZE {
        for z in 0..CHUNK_SIZE {
            let height = 5;
            for y in 0..height {
                let block = if y == height - 1 {
                    Block::Grass
                } else if y >= height - 3 {
                    Block::Dirt
                } else {
                    Block::Stone
                };
                chunk.set(x, y, z, block);
            }
        }
    }
    chunk
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct RawVoxelVertex {
    pub position: [f32; 3],
    pub tex_coords: [f32; 2],
}

struct Face {
    normal: [i32; 3],
    corners: [[f32; 3]; 4],
}

const FACES: [Face; 6] = [
    Face {
        normal: [1, 0, 0],
        corners: [[1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [1.0, 1.0, 1.0], [1.0, 0.0, 1.0]],
    },
    Face {
        normal: [-1, 0, 0],
        corners: [[0.0, 0.0, 1.0], [0.0, 1.0, 1.0], [0.0, 1.0, 0.0], [0.0, 0.0, 0.0]],
    },
    Face {
        normal: [0, 1, 0],
        corners: [[0.0, 1.0, 1.0], [1.0, 1.0, 1.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]],
    },
    Face {
        normal: [0, -1, 0],
        corners: [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 1.0], [0.0, 0.0, 1.0]],
    },
    Face {
        normal: [0, 0, 1],
        corners: [[1.0, 0.0, 1.0], [1.0, 1.0, 1.0], [0.0, 1.0, 1.0], [0.0, 0.0, 1.0]],
    },
    Face {
        normal: [0, 0, -1],
        corners: [[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0], [1.0, 0.0, 0.0]],
    },
];

struct ChunkMesh {
    vao: GLuint,
    vbo: GLuint,
    ebo: GLuint,
    pub index_count: GLsizei,
}

fn generate_mesh(chunk: &Chunk) -> (Vec<RawVoxelVertex>, Vec<u32>) {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let uv = [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]];
    for x in 0..CHUNK_SIZE {
        for y in 0..CHUNK_SIZE {
            for z in 0..CHUNK_SIZE {
                if chunk.get(x, y, z) == Block::Air { continue; }
                for face in FACES.iter() {
                    let nx = x as i32 + face.normal[0];
                    let ny = y as i32 + face.normal[1];
                    let nz = z as i32 + face.normal[2];
                    if nx >= 0 && nx < CHUNK_SIZE as i32 && ny >= 0 && ny < CHUNK_SIZE as i32 && nz >= 0 && nz < CHUNK_SIZE as i32 {
                        if chunk.get(nx as usize, ny as usize, nz as usize) != Block::Air { continue; }
                    }
                    let base = vertices.len() as u32;
                    for i in 0..4 {
                        let corner = face.corners[i];
                        vertices.push(RawVoxelVertex {
                            position: [x as f32 + corner[0], y as f32 + corner[1], z as f32 + corner[2]],
                            tex_coords: uv[i],
                        });
                    }
                    indices.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 3, base]);
                }
            }
        }
    }
    (vertices, indices)
}

impl ChunkMesh {
    pub unsafe fn new(verts: &[RawVoxelVertex], indices: &[u32]) -> Self {
        let (mut vao, mut vbo, mut ebo) = (0, 0, 0);
        glGenVertexArrays(1, &mut vao);
        glGenBuffers(1, &mut vbo);
        glGenBuffers(1, &mut ebo);
        glBindVertexArray(vao);
        glBindBuffer(GL_ARRAY_BUFFER, vbo);
        glBufferData(GL_ARRAY_BUFFER, (verts.len() * size_of::<RawVoxelVertex>()) as GLsizeiptr, verts.as_ptr() as *const _, GL_STATIC_DRAW);
        glBindBuffer(GL_ELEMENT_ARRAY_BUFFER, ebo);
        glBufferData(GL_ELEMENT_ARRAY_BUFFER, (indices.len() * size_of::<u32>()) as GLsizeiptr, indices.as_ptr() as *const _, GL_STATIC_DRAW);
        glVertexAttribPointer(0, 3, GL_FLOAT, GL_FALSE, size_of::<RawVoxelVertex>() as GLsizei, 0 as *const _);
        glEnableVertexAttribArray(0);
        glVertexAttribPointer(1, 2, GL_FLOAT, GL_FALSE, size_of::<RawVoxelVertex>() as GLsizei, offset_of!(RawVoxelVertex, tex_coords) as *const _);
        glEnableVertexAttribArray(1);
        glBindVertexArray(0);
        ChunkMesh { vao, vbo, ebo, index_count: indices.len() as GLsizei }
    }

    pub unsafe fn draw(&self) {
        glBindVertexArray(self.vao);
        glDrawElements(GL_TRIANGLES, self.index_count, GL_UNSIGNED_INT, core::ptr::null());
        glBindVertexArray(0);
    }

    pub unsafe fn cleanup(&self) {
        glDeleteBuffers(1, &self.vbo);
        glDeleteBuffers(1, &self.ebo);
        glDeleteVertexArrays(1, &self.vao);
    }
}

#[repr(C)]
#[derive(Debug)]
struct Canvas {
    id: core::ffi::c_long,
    width: core::ffi::c_int,
    height: core::ffi::c_int,
    pixels: *mut core::ffi::c_uint,
}

impl Canvas {
    fn new() -> Self {
        Self { id: 0, width: 0, height: 0, pixels: 0 as *mut core::ffi::c_uint }
    }
}

#[repr(C)]
#[derive(Debug)]
struct CanvasEvent {
    kind: core::ffi::c_int,
    x: core::ffi::c_int,
    y: core::ffi::c_int,
    buttons: core::ffi::c_int,
    key: core::ffi::c_int,
}

impl CanvasEvent {
    fn new() -> Self {
        Self { kind: 0, x: 0, y: 0, buttons: 0, key: 0 }
    }
}

unsafe extern "C"
{
    fn canvas_open(c: *mut Canvas, title: *const core::ffi::c_char, width: core::ffi::c_int, height: core::ffi::c_int) -> core::ffi::c_int;
    fn canvas_poll(c: *mut Canvas, e: *mut CanvasEvent) -> core::ffi::c_int;
    fn kgl_open(c: *mut Canvas);
    fn kgl_swap() -> core::ffi::c_int;
    fn kgl_close();
    fn kgl_glsl_program(vs_source: *const GLchar, fs_source: *const GLchar) -> GLuint;
    fn kgl_error() -> *const GLchar;
    fn exit(status: core::ffi::c_int) -> !;
}

struct Shader {
    id: GLuint,
    mvp_location: GLint,
}

impl Shader {
    pub unsafe fn new() -> Self {
        let id = kgl_glsl_program(VERT_SHADER.as_ptr() as *const _, FRAG_SHADER.as_ptr() as *const _);
        let mvp_location = glGetUniformLocation(id, c"uMVP".as_ptr());
        Self { id, mvp_location }
    }

    pub unsafe fn bind(&self) {
        glUseProgram(self.id);
    }

    pub unsafe fn set_mvp(&self, mvp: &glam::Mat4) {
        glUniformMatrix4fv(self.mvp_location, 1, GL_FALSE, mvp.as_ref() as *const _);
    }

    pub unsafe fn cleanup(&self) {
        glDeleteProgram(self.id);
    }
}

fn create_mvp() -> Mat4 {
    let model = Mat4::IDENTITY;
    let view = glam::camera::rh::view::look_at_mat4(
        Vec3::new(25.0, 20.0, 25.0),
        Vec3::new(8.0, 2.0, 8.0),
        Vec3::Y
    );
    let projection = glam::camera::rh::proj::opengl::perspective(
        70.0_f32.to_radians(),
        800.0 / 600.0,
        0.1,
        1000.0
    );
    projection * view * model
}

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    main();
    unsafe { exit(0) }
}

fn main() {
    let chunk = generate_chunk();
    let (verts, indices) = generate_mesh(&chunk);
    unsafe {
        let mut c = Canvas::new();
        canvas_open(&mut c, c"Voxel".as_ptr(), 800, 600);
        kgl_open(&mut c);
        glViewport(0, 0, 800, 600);
        glEnable(GL_DEPTH_TEST);
        glClearColor(0.53, 0.81, 0.92, 1.0);
        let shader = Shader::new();
        if shader.id == 0 {
            kgl_close();
            writeln(c"Shader issue".as_ptr());
            exit(1);
        }
        let mesh = ChunkMesh::new(&verts, &indices);
        let mvp = create_mvp();
        loop {
            let mut e = CanvasEvent::new();
            while canvas_poll(&mut c, &mut e) > 0 {
                if e.kind == 5 {
                    mesh.cleanup();
                    shader.cleanup();
                    kgl_close();
                    exit(0);
                }
            }
            glClear(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT);
            shader.bind();
            shader.set_mvp(&mvp);
            mesh.draw();
            kgl_swap();
        }
    }
}
