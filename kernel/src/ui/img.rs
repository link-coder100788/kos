//! Image decoding.

use crate::ui::gfx::Surface;

fn u16_at(d: &[u8], at: usize) -> u16 { u16::from_le_bytes([d[at], d[at + 1]]) }
fn u32_at(d: &[u8], at: usize) -> u32 { u32::from_le_bytes(d[at..at + 4].try_into().unwrap()) }

/// Decodes an uncompressed 24- or 32-bit BMP.
pub fn decode_bmp(data: &[u8]) -> Result<Surface, &'static str> {
    if data.len() < 54 || &data[0..2] != b"BM" {
        return Err("not a BMP file");
    }
    let pixel_offset = u32_at(data, 10) as usize;
    let width = u32_at(data, 18) as i32;
    let height = u32_at(data, 22) as i32;
    let bpp = u16_at(data, 28) as usize;
    let compression = u32_at(data, 30);
    if compression != 0 && !(compression == 3 && bpp == 32) {
        return Err("compressed BMPs aren't supported");
    }
    if bpp != 24 && bpp != 32 {
        return Err("only 24- and 32-bit BMPs are supported");
    }
    if width <= 0 || height == 0 || width > 4096 || height.abs() > 4096 {
        return Err("unsupported BMP size");
    }
    // Rows are padded to 4 bytes and stored bottom-up unless height < 0.
    let bytes_pp = bpp / 8;
    let stride = (width as usize * bytes_pp + 3) & !3;
    let rows = height.unsigned_abs() as usize;
    if pixel_offset + stride * rows > data.len() {
        return Err("truncated BMP");
    }
    let mut image = Surface::new(width, rows as i32, 0);
    for row in 0..rows {
        let src_row = if height > 0 { rows - 1 - row } else { row };
        let src = &data[pixel_offset + src_row * stride..];
        for x in 0..width as usize {
            let p = &src[x * bytes_pp..];
            let (b, g, r) = (p[0] as u32, p[1] as u32, p[2] as u32);
            image.pixels[row * width as usize + x] = (r << 16) | (g << 8) | b;
        }
    }
    Ok(image)
}

/// Decodes a png
pub fn decode_png(data: &[u8]) -> Result<Surface, &'static str> {
    let img = nopng::decode_image(data).map_err(|_| "invalid PNG")?;
    if img.width.get() == 0 || img.height.get() == 0 || img.width.get() > 4096 || img.height.get() > 4096 { return Err("unsupported PNG size"); }
    let rgba = nopng::reformat_pixels(&img.pixel_format, &img.pixels, &nopng::PixelFormat::Rgba8).map_err(|_| "unsupported format")?;
    let width = img.width.get() as usize;
    let height = img.height.get() as usize;
    let mut image = Surface::new(width as i32, height as i32, 0);
    for (i, p) in rgba.chunks_exact(4).enumerate() {
        let (r, g, b) = (p[0] as u32, p[1] as u32, p[2] as u32);
        image.pixels[i] = (r << 16) | (g << 8) | b;
    }
    Ok(image)
}

/// Decodes a PNG, blending see-through pixels onto `background` (0xRRGGBB)
/// instead of dropping their transparency. Web pages need this: their
/// logos and icons are mostly transparent PNGs meant for a white page.
pub fn decode_png_over(data: &[u8], background: u32) -> Result<Surface, &'static str> {
    let img = nopng::decode_image(data).map_err(|_| "invalid PNG")?;
    if img.width.get() == 0 || img.height.get() == 0 || img.width.get() > 4096 || img.height.get() > 4096 { return Err("unsupported PNG size"); }
    let rgba = nopng::reformat_pixels(&img.pixel_format, &img.pixels, &nopng::PixelFormat::Rgba8).map_err(|_| "unsupported format")?;
    let mut image = Surface::new(img.width.get() as i32, img.height.get() as i32, 0);
    let blend = |fg: u8, shift: u32, alpha: u32| (fg as u32 * alpha + ((background >> shift) & 0xff) * (255 - alpha)) / 255;
    for (i, p) in rgba.chunks_exact(4).enumerate() {
        let a = p[3] as u32;
        image.pixels[i] = (blend(p[0], 16, a) << 16) | (blend(p[1], 8, a) << 8) | blend(p[2], 0, a);
    }
    Ok(image)
}

/// Decodes a jpeg
pub fn decode_jpeg(data: &[u8]) -> Result<Surface, &'static str> {
    use rusty_jpeg::Decoder;
    let mut decoder = Decoder::new(data);
    let pixels = decoder.decode().map_err(|_| "invalid jpeg")?;
    let info = decoder.info().ok_or("missing jpeg info")?;
    let width = info.width as usize;
    let height = info.height as usize;
    if width == 0 || height == 0 || width > 4096 || height > 4096 { return Err("unsupported JPEG size"); }
    let mut image = Surface::new(width as i32, height as i32, 0);
    match info.pixel_format {
        rusty_jpeg::PixelFormat::RGB24 => {
            if pixels.len() != width * height * 3 { return Err("invalid JPEG pixel data"); }
            for (i, p) in pixels.chunks_exact(3).enumerate() {
                image.pixels[i] = ((p[0] as u32) << 16) | ((p[1] as u32) << 8) | (p[2] as u32);
            }
        }
        rusty_jpeg::PixelFormat::L8 => {
            if pixels.len() != width * height {
                return Err("invalid JPEG pixel data");
            }
            for (i, &p) in pixels.iter().enumerate() {
                let gray = p as u32;
                image.pixels[i] = (gray << 16) | (gray << 8) | gray;
            }
        }
        _ => return Err("unsupported JPEG pixel format"),
    }
    Ok(image)
}

/// Decodes an heic
pub fn decode_heic(data: &[u8]) -> Result<Surface, &'static str> {
    use heic::{DecoderConfig, PixelLayout};
    let out = DecoderConfig::new().decode(data, PixelLayout::Rgba8).map_err(|_| "invalid heic")?;
    let width = out.width as usize;
    let height = out.height as usize;
    if width == 0 || height == 0 || width > 4096 || height > 4096 { return Err("unsupported HEIC size"); }
    let mut image = Surface::new(width as i32, height as i32, 0);
    for (i, p) in out.data.chunks_exact(4).enumerate() {
        image.pixels[i] = ((p[0] as u32) << 16) | ((p[1] as u32) << 8) | (p[2] as u32);
    }
    Ok(image)
}
