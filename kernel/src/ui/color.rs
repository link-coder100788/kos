use core::ops::{Deref, DerefMut};

pub struct Color {
    _internal: u32,
}

pub const fn rgb_to_xrgb8888_c(r: u8, g: u8, b: u8) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

pub fn rgb_to_xrgb8888(r: u8, g: u8, b: u8) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

pub const fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let c = v * s;
    let x = c * (1.0 - (((h / 60.0) % 2.0) - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match h {
        h if h < 60.0  => (c, x, 0.0),
        h if h < 120.0 => (x, c, 0.0),
        h if h < 180.0 => (0.0, c, x),
        h if h < 240.0 => (0.0, x, c),
        h if h < 300.0 => (x, 0.0, c),
        _              => (c, 0.0, x),
    };
    (
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    )
}

pub fn xrgb8888_to_rgb(color: u32) -> (u8, u8, u8) {
    let r = ((color >> 16) & 0xFF) as u8;
    let g = ((color >> 8) & 0xFF) as u8;
    let b = (color & 0xFF) as u8;
    (r, g, b)
}

pub trait RGBAverage {
    fn average(&self) -> u8;
}

impl RGBAverage for (u8, u8, u8) {
    fn average(&self) -> u8 {
        let (r, g, b) = self.clone();
        ((r as u16 + g as u16 + b as u16) / 3) as u8
    }
}

impl Color {
    pub const fn new_rgb(r: u8, g: u8, b: u8) -> Self { Self { _internal: ((r as u32) << 16) | ((g as u32) << 8) | (b as u32) } }
    pub const fn new_hsv(h: f32, s: f32, v: f32) -> Self { let rgb = hsv_to_rgb(h, s, v); Self::new_rgb(rgb.0, rgb.1, rgb.2) }
    pub const fn new_gray(value: u8) -> Self { Self::new_rgb(value, value, value) } 
    pub const fn value(&self) -> u32 { self._internal }
    pub const fn black() -> Self { Self::new_rgb(0, 0, 0) }
    pub const fn white() -> Self { Self::new_rgb(255, 255, 255) }
    pub const fn red() -> Self { Self::new_rgb(255, 0, 0) }
    pub const fn green() -> Self { Self::new_rgb(0, 255, 0) }
    pub const fn blue() -> Self { Self::new_rgb(0, 0, 255) }
    pub const fn yellow() -> Self { Self::new_rgb(255, 255, 0) }
    pub const fn cyan() -> Self { Self::new_rgb(0, 255, 255) }
    pub const fn magenta() -> Self { Self::new_rgb(255, 0, 255) }    
}

impl Default for Color {
    fn default() -> Self { Self::new_rgb(0, 0, 0) }
}

impl Deref for Color {
    type Target = u32;
    
    fn deref(&self) -> &Self::Target { &self._internal }
}

impl DerefMut for Color {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self._internal }
}

impl PartialEq for Color {
    fn eq(&self, other: &Self) -> bool {
        let c1 = xrgb8888_to_rgb(self._internal);
        let c2 = xrgb8888_to_rgb(self._internal);
        c1.0 == c2.0 && c1.1 == c2.1 && c1.2 == c2.2
    }
}

impl PartialOrd for Color {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        let av1 = xrgb8888_to_rgb(self._internal).average();
        let av2 = xrgb8888_to_rgb(other._internal).average();
        if av1 == av2 { Some(core::cmp::Ordering::Equal) } else if av1 < av2 { Some(core::cmp::Ordering::Less) } else { Some(core::cmp::Ordering::Greater) }
    }
}