//! The digit recogniser behind Paint's "Read": a small convolutional
//! network trained in `tools/digitclass`. `prepare` mirrors `prepare.py`
//! and `DigitModel::run` mirrors `model.py`; the file format is documented
//! in `export.py` there.

use core::fmt::Display;

use alloc::vec;
use alloc::vec::Vec;
use crate::model::{softmax, Linear, Reader};

/// The network's input is SIZE x SIZE, the digit shrunk to fit BOX x BOX.
pub const SIZE: usize = 28;
const BOX: usize = 20;
const CLASSES: usize = 10;
/// Columns of ink with fewer pixels than this are specks, not digits.
const MIN_INK: usize = 8;

/// A 3x3 convolution padded by 1, so it keeps the size. Weights are
/// [outputs, inputs, 3, 3] row-major.
struct Conv {
    weight: Vec<f32>,
    bias: Vec<f32>,
    inputs: usize,
}

pub struct DigitModel {
    /// Stroke width as a fraction of the digit's longer side, as in
    /// training.
    stroke: f32,
    conv1: Conv,
    conv2: Conv,
    fc1: Linear,
    fc2: Linear,
}

/// What the network made of one digit.
#[derive(Clone, Copy, Debug)]
pub struct Reading {
    pub digit: u8,
    /// How sure it is, 0 to 1.
    pub confidence: f32,
    /// The runner-up, for "4 or 9?".
    pub second: (u8, f32),
}

impl Display for Reading {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}: {} ({}: {})", self.digit, self.confidence, self.second.0, self.second.1)
    }
}

impl Conv {
    /// `x` is [inputs, n, n]; returns [outputs, n, n].
    fn apply(&self, x: &[f32], n: usize) -> Vec<f32> {
        let mut out = vec![0.0; self.bias.len() * n * n];
        for (o, plane) in out.chunks_exact_mut(n * n).enumerate() {
            plane.fill(self.bias[o]);
            for (i, input) in x.chunks_exact(n * n).enumerate() {
                let kernel = &self.weight[(o * self.inputs + i) * 9..][..9];
                for (k, &w) in kernel.iter().enumerate() {
                    let (ky, kx) = (k / 3, k % 3);
                    // out[y][x] += w * in[y + ky - 1][x + kx - 1], skipping
                    // the padding (zeros).
                    for y in (1usize.saturating_sub(ky))..n.min(n + 1 - ky) {
                        let src = &input[(y + ky - 1) * n..][..n];
                        let dst = &mut plane[y * n..][..n];
                        for x in (1usize.saturating_sub(kx))..n.min(n + 1 - kx) {
                            dst[x] += w * src[x + kx - 1];
                        }
                    }
                }
            }
        }
        out
    }
}

/// relu, then 2x2 max pooling: [channels, n, n] -> [channels, n/2, n/2].
fn relu_pool(x: &[f32], n: usize) -> Vec<f32> {
    let m = n / 2;
    let mut out = Vec::with_capacity(x.len() / 4);
    for plane in x.chunks_exact(n * n) {
        for y in 0..m {
            for x in 0..m {
                let at = |dy: usize, dx: usize| plane[(2 * y + dy) * n + 2 * x + dx];
                out.push(at(0, 0).max(at(0, 1)).max(at(1, 0)).max(at(1, 1)).max(0.0));
            }
        }
    }
    out
}

fn round_half_up(x: f64) -> i64 {
    libm::floor(x + 0.5) as i64
}

/// Average stroke width of the pixels set in `mask` (w x h): for a stroke
/// of width s and length L, the area is about s*L and the edge pixels 2*L.
fn stroke_width(mask: &[bool], w: usize, h: usize) -> f64 {
    let set = |x: isize, y: isize| x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && mask[y as usize * w + x as usize];
    let (mut area, mut edge) = (0usize, 0usize);
    for y in 0..h as isize {
        for x in 0..w as isize {
            if set(x, y) {
                area += 1;
                if !(set(x - 1, y) && set(x + 1, y) && set(x, y - 1) && set(x, y + 1)) {
                    edge += 1;
                }
            }
        }
    }
    if edge == 0 { 0.0 } else { 2.0 * area as f64 / edge as f64 }
}

/// Each pixel becomes the maximum of the (2r+1)x(2r+1) square around it
/// (rows, then columns). Ink is never negative, so leaving out what's past
/// the edge is the same as padding with zeros.
fn grow(img: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut rows = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            rows[y * w + x] = img[y * w + x.saturating_sub(r)..y * w + (x + r + 1).min(w)].iter().copied().fold(0.0, f32::max);
        }
    }
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            out[y * w + x] = (y.saturating_sub(r)..(y + r + 1).min(h)).map(|yy| rows[yy * w + x]).fold(0.0, f32::max);
        }
    }
    out
}

/// [n_out, n_in] weights: output pixel i averages input pixels over
/// [i, i+1) * n_in / n_out, each weighted by how much of it is covered.
fn area_matrix(n_out: usize, n_in: usize) -> Vec<f32> {
    let mut m = vec![0.0f32; n_out * n_in];
    let step = n_in as f64 / n_out as f64;
    for i in 0..n_out {
        let (lo, hi) = (i as f64 * step, (i + 1) as f64 * step);
        for j in (libm::floor(lo) as usize)..n_in.min(libm::ceil(hi) as usize) {
            m[i * n_in + j] = ((hi.min(j as f64 + 1.0) - lo.max(j as f64)) / step) as f32;
        }
    }
    m
}

/// Turns a drawing (`ink`, w x h, 0 = paper and 1 = ink) into the network's
/// input, the way MNIST digits look: cropped, strokes thickened to
/// `stroke` of the longer side, shrunk into a 20x20 box by averaging areas
/// and placed in 28x28 with its centre of mass in the middle. All zeros if
/// there's no ink.
pub fn prepare(ink: &[f32], w: usize, h: usize, stroke: f32) -> Vec<f32> {
    let mut out = vec![0.0f32; SIZE * SIZE];
    let (mut top, mut bottom, mut left, mut right) = (h, 0, w, 0);
    for y in 0..h {
        for x in 0..w {
            if ink[y * w + x] >= 0.5 {
                (top, bottom, left, right) = (top.min(y), bottom.max(y + 1), left.min(x), right.max(x + 1));
            }
        }
    }
    if bottom == 0 {
        return out;
    }
    let (mut w2, mut h2) = (right - left, bottom - top);
    let mut img: Vec<f32> = (top..bottom).flat_map(|y| ink[y * w + left..y * w + right].iter().copied()).collect();

    let mask: Vec<bool> = img.iter().map(|&v| v >= 0.5).collect();
    let r = round_half_up((stroke as f64 * w2.max(h2) as f64 - stroke_width(&mask, w2, h2)) / 2.0);
    if r > 0 {
        let r = r as usize;
        let (pw, ph) = (w2 + 2 * r, h2 + 2 * r);
        let mut padded = vec![0.0f32; pw * ph];
        for y in 0..h2 {
            padded[(y + r) * pw + r..][..w2].copy_from_slice(&img[y * w2..][..w2]);
        }
        img = grow(&padded, pw, ph, r);
        (w2, h2) = (pw, ph);
    }

    let scale = BOX as f64 / w2.max(h2) as f64;
    let oh = round_half_up(h2 as f64 * scale).max(1) as usize;
    let ow = round_half_up(w2 as f64 * scale).max(1) as usize;
    let (ry, rx) = (area_matrix(oh, h2), area_matrix(ow, w2));
    // small = ry @ img @ rx^T
    let mut tmp = vec![0.0f32; oh * w2];
    for i in 0..oh {
        for y in 0..h2 {
            let wgt = ry[i * h2 + y];
            if wgt != 0.0 {
                for x in 0..w2 {
                    tmp[i * w2 + x] += wgt * img[y * w2 + x];
                }
            }
        }
    }
    let mut small = vec![0.0f32; oh * ow];
    for i in 0..oh {
        for j in 0..ow {
            small[i * ow + j] = (0..w2).map(|x| tmp[i * w2 + x] * rx[j * w2 + x]).sum();
        }
    }

    let total: f64 = small.iter().map(|&v| v as f64).sum();
    let cy: f64 = (0..oh).map(|i| small[i * ow..][..ow].iter().map(|&v| v as f64).sum::<f64>() * (i as f64 + 0.5)).sum::<f64>() / total;
    let cx: f64 = (0..ow).map(|j| (0..oh).map(|i| small[i * ow + j] as f64).sum::<f64>() * (j as f64 + 0.5)).sum::<f64>() / total;
    let y0 = round_half_up(SIZE as f64 / 2.0 - cy).clamp(0, (SIZE - oh) as i64) as usize;
    let x0 = round_half_up(SIZE as f64 / 2.0 - cx).clamp(0, (SIZE - ow) as i64) as usize;
    for i in 0..oh {
        for j in 0..ow {
            out[(y0 + i) * SIZE + x0 + j] = small[i * ow + j].clamp(0.0, 1.0);
        }
    }
    out
}

/// The column ranges [start, end) of the digits in a drawing: ink
/// separated by at least one empty column, left to right. Specks are left
/// out.
pub fn split(ink: &[f32], w: usize, h: usize) -> Vec<(usize, usize)> {
    let counts: Vec<usize> = (0..w).map(|x| (0..h).filter(|&y| ink[y * w + x] >= 0.5).count()).collect();
    let mut out = Vec::new();
    let mut x = 0;
    while x < w {
        if counts[x] == 0 {
            x += 1;
            continue;
        }
        let start = x;
        while x < w && counts[x] > 0 {
            x += 1;
        }
        if counts[start..x].iter().sum::<usize>() >= MIN_INK {
            out.push((start, x));
        }
    }
    out
}

impl DigitModel {
    /// Parses a model file (`KOSD` version 1).
    pub fn load(data: &[u8]) -> Result<DigitModel, &'static str> {
        let mut r = Reader { data, pos: 0 };
        if r.bytes(4)? != b"KOSD" {
            return Err("not a digit model file");
        }
        if r.u32()? != 1 {
            return Err("unsupported model file version");
        }
        let (size, boxed, c1, c2, hidden, classes) = (r.u32()?, r.u32()?, r.u32()?, r.u32()?, r.u32()?, r.u32()?);
        if size != SIZE || boxed != BOX || classes != CLASSES {
            return Err("model was trained for a different input");
        }
        if c1 == 0 || c2 == 0 || hidden == 0 || c1 > 1024 || c2 > 1024 || hidden > 65536 {
            return Err("model file has impossible sizes");
        }
        let stroke = f32::from_le_bytes(r.bytes(4)?.try_into().unwrap());
        let conv1 = Conv { weight: r.floats(c1 * 9)?, bias: r.floats(c1)?, inputs: 1 };
        let conv2 = Conv { weight: r.floats(c2 * c1 * 9)?, bias: r.floats(c2)?, inputs: c1 };
        let fc1 = r.linear(hidden, c2 * (SIZE / 4) * (SIZE / 4))?;
        let fc2 = r.linear(classes, hidden)?;
        if r.pos != data.len() {
            return Err("model file has extra data at the end");
        }
        Ok(DigitModel { stroke, conv1, conv2, fc1, fc2 })
    }

    /// `prepare` with the stroke width the model was trained with.
    pub fn prepare(&self, ink: &[f32], w: usize, h: usize) -> Vec<f32> {
        prepare(ink, w, h, self.stroke)
    }

    /// The network's scores (before softmax) for a prepared 28x28 input.
    pub fn run(&self, input: &[f32]) -> Vec<f32> {
        let x = relu_pool(&self.conv1.apply(input, SIZE), SIZE);
        let x = relu_pool(&self.conv2.apply(&x, SIZE / 2), SIZE / 2);
        let mut hidden = Vec::new();
        self.fc1.apply(&x, &mut hidden);
        hidden.iter_mut().for_each(|v| *v = v.max(0.0));
        let mut scores = Vec::new();
        self.fc2.apply(&hidden, &mut scores);
        scores
    }

    /// Reads one digit, or None if there's no ink.
    pub fn read(&self, ink: &[f32], w: usize, h: usize) -> Option<Reading> {
        let input = self.prepare(ink, w, h);
        if input.iter().all(|&v| v == 0.0) {
            return None;
        }
        let mut probs = self.run(&input);
        softmax(&mut probs);
        let mut order: Vec<usize> = (0..probs.len()).collect();
        order.sort_by(|&a, &b| probs[b].total_cmp(&probs[a]));
        Some(Reading { digit: order[0] as u8, confidence: probs[order[0]], second: (order[1] as u8, probs[order[1]]) })
    }

    /// Reads every digit in a drawing, left to right.
    pub fn read_all(&self, ink: &[f32], w: usize, h: usize) -> Vec<Reading> {
        split(ink, w, h)
            .into_iter()
            .filter_map(|(start, end)| {
                let cols = end - start;
                let digit: Vec<f32> = (0..h).flat_map(|y| ink[y * w + start..y * w + end].iter().copied()).collect();
                self.read(&digit, cols, h)
            })
            .collect()
    }
}
