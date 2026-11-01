//! A deliberately silly full-screen panic image. Call from your fault
//! handlers before halting — it's cheap and doesn't allocate.

use super::display::{self, clear, put_pixel, HEIGHT, WIDTH};

const BG: u32 = 0x001a1a2e;       // near-black navy
const GLITCH: u32 = 0x00e94560;   // hot red/pink
const FACE: u32 = 0x00ffdd57;     // yellow
const INK: u32 = 0x00202020;      // near-black, for eyes/mouth

fn fill_rect(x0: usize, y0: usize, w: usize, h: usize, color: u32) {
    for y in y0..(y0 + h).min(HEIGHT) {
        for x in x0..(x0 + w).min(WIDTH) {
            put_pixel(x, y, color);
        }
    }
}

fn draw_thick_line(mut x0: isize, mut y0: isize, x1: isize, y1: isize, thickness: isize, color: u32) {
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;

    loop {
        for ty in 0..thickness {
            for tx in 0..thickness {
                let px = x0 + tx - thickness / 2;
                let py = y0 + ty - thickness / 2;
                if px >= 0 && py >= 0 {
                    put_pixel(px as usize, py as usize, color);
                }
            }
        }
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

// Tiny xorshift so consecutive panics don't all render an identical image.
static mut RNG_STATE: u32 = 0xDEAD_BEEF;
fn next_rand() -> u32 {
    unsafe {
        let mut x = RNG_STATE;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        RNG_STATE = x;
        x
    }
}

/// Full-screen "kernel panic" image: a dead cartoon face with X eyes on a
/// glitchy background. Call this at the top of your sync/IRQ fault handlers,
/// before you dump registers to UART and halt.
pub fn draw_panic_screen() {
    clear(BG);

    // Glitch bars at random heights/widths — feels appropriately "crashed."
    for _ in 0..14 {
        let y = (next_rand() as usize) % HEIGHT;
        let h = 2 + (next_rand() as usize) % 6;
        let w = 40 + (next_rand() as usize) % (WIDTH / 2);
        let x = (next_rand() as usize) % WIDTH.saturating_sub(w).max(1);
        fill_rect(x, y, w, h, GLITCH);
    }

    // The face.
    let cx = WIDTH / 2;
    let cy = HEIGHT / 2;
    let half = 140usize;
    fill_rect(cx.saturating_sub(half), cy.saturating_sub(half), half * 2, half * 2, FACE);

    // X eyes — universal cartoon shorthand for "this thing is dead."
    let eye_dx = 60isize;
    let eye_dy = 30isize;
    let eye_size = 24isize;
    for &sign in &[-1isize, 1isize] {
        let ex = cx as isize + sign * eye_dx;
        let ey = cy as isize - eye_dy;
        draw_thick_line(ex - eye_size, ey - eye_size, ex + eye_size, ey + eye_size, 6, INK);
        draw_thick_line(ex - eye_size, ey + eye_size, ex + eye_size, ey - eye_size, 6, INK);
    }

    // Frown, as a short polyline.
    let mouth_y = cy as isize + 60;
    let points = [(-70isize, 0isize), (-35, 25), (0, 32), (35, 25), (70, 0)];
    for w in points.windows(2) {
        let (x0, y0) = w[0];
        let (x1, y1) = w[1];
        draw_thick_line(cx as isize + x0, mouth_y + y0, cx as isize + x1, mouth_y + y1, 6, INK);
    }
    display::flush_for_panic();
}