//! The graphical desktop: a compositing window manager (`wm`) run by the
//! UI task, apps that draw window contents, and the drawing primitives.

pub mod apps;
pub mod color;
pub mod console;
pub mod font8x8;
pub mod gfx;
pub mod img;
pub mod task;
pub mod video;
pub mod window;
pub mod wm;
