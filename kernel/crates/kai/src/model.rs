//! Loading K-AI's model file and running it. Each step mirrors
//! `tools/assistant/model.py`; the file format is documented in
//! `tools/assistant/export.py`.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use crate::tokenizer::{self, Token, BUCKETS, MAX_TOKENS};

/// PyTorch's LayerNorm default.
const LAYER_NORM_EPS: f32 = 1e-5;

/// A Linear layer: `y = W x + b`, with W stored [out, in] row-major.
pub(crate) struct Linear {
    weight: Vec<f32>,
    bias: Vec<f32>,
    inputs: usize,
}

struct LayerNorm {
    weight: Vec<f32>,
    bias: Vec<f32>,
}

struct Block {
    norm1: LayerNorm,
    q: Linear,
    k: Linear,
    v: Linear,
    o: Linear,
    norm2: LayerNorm,
    ff1: Linear,
    ff2: Linear,
}

pub struct Model {
    pub intents: Vec<String>,
    pub tags: Vec<String>,
    d: usize,
    heads: usize,
    embed: Vec<f32>,
    position: Vec<f32>,
    blocks: Vec<Block>,
    norm: LayerNorm,
    intent: Linear,
    tag: Linear,
}

/// The raw result of running the model on one text.
pub struct Output {
    pub tokens: Vec<Token>,
    /// One score per intent (before softmax).
    pub intent_scores: Vec<f32>,
    /// One row of scores per token, one score per tag.
    pub tag_scores: Vec<Vec<f32>>,
}

/// Reads the model file front to back.
pub(crate) struct Reader<'a> {
    pub(crate) data: &'a [u8],
    pub(crate) pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn bytes(&mut self, n: usize) -> Result<&'a [u8], &'static str> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.data.len()).ok_or("model file is truncated")?;
        let out = &self.data[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    pub(crate) fn u32(&mut self) -> Result<usize, &'static str> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()) as usize)
    }

    fn names(&mut self) -> Result<Vec<String>, &'static str> {
        let count = self.u32()?;
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            let len = self.bytes(1)?[0] as usize;
            let name = core::str::from_utf8(self.bytes(len)?).map_err(|_| "bad name in model file")?;
            out.push(String::from(name));
        }
        Ok(out)
    }

    pub(crate) fn floats(&mut self, n: usize) -> Result<Vec<f32>, &'static str> {
        let bytes = self.bytes(n.checked_mul(4).ok_or("model file is too big")?)?;
        Ok(bytes.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect())
    }

    pub(crate) fn linear(&mut self, outputs: usize, inputs: usize) -> Result<Linear, &'static str> {
        Ok(Linear { weight: self.floats(outputs * inputs)?, bias: self.floats(outputs)?, inputs })
    }

    fn layer_norm(&mut self, d: usize) -> Result<LayerNorm, &'static str> {
        Ok(LayerNorm { weight: self.floats(d)?, bias: self.floats(d)? })
    }
}

impl Linear {
    pub(crate) fn apply(&self, x: &[f32], out: &mut Vec<f32>) {
        out.clear();
        for (row, b) in self.weight.chunks_exact(self.inputs).zip(&self.bias) {
            out.push(b + row.iter().zip(x).map(|(w, v)| w * v).sum::<f32>());
        }
    }
}

impl LayerNorm {
    fn apply(&self, x: &[f32], out: &mut Vec<f32>) {
        let n = x.len() as f32;
        let mean = x.iter().sum::<f32>() / n;
        let variance = x.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / n;
        let scale = 1.0 / libm::sqrtf(variance + LAYER_NORM_EPS);
        out.clear();
        out.extend(x.iter().zip(&self.weight).zip(&self.bias).map(|((v, w), b)| (v - mean) * scale * w + b));
    }
}

/// Softmax in place, shifted by the maximum so `exp` can't overflow.
pub fn softmax(x: &mut [f32]) {
    let max = x.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let mut sum = 0.0;
    for v in x.iter_mut() {
        *v = libm::expf(*v - max);
        sum += *v;
    }
    for v in x.iter_mut() {
        *v /= sum;
    }
}

impl Model {
    /// Parses a model file (`KOSM` version 1).
    pub fn load(data: &[u8]) -> Result<Model, &'static str> {
        let mut r = Reader { data, pos: 0 };
        if r.bytes(4)? != b"KOSM" {
            return Err("not a K-AI model file");
        }
        if r.u32()? != 1 {
            return Err("unsupported model file version");
        }
        let (buckets, max_tokens, d, layers, heads, ff) = (r.u32()?, r.u32()?, r.u32()?, r.u32()?, r.u32()?, r.u32()?);
        if buckets != BUCKETS || max_tokens != MAX_TOKENS {
            return Err("model was trained with a different tokenizer");
        }
        if d == 0 || heads == 0 || d % heads != 0 || layers > 64 || d > 4096 || ff > 65536 {
            return Err("model file has impossible sizes");
        }
        let intents = r.names()?;
        let tags = r.names()?;
        let embed = r.floats(buckets * d)?;
        let position = r.floats(max_tokens * d)?;
        let mut blocks = Vec::with_capacity(layers);
        for _ in 0..layers {
            blocks.push(Block {
                norm1: r.layer_norm(d)?,
                q: r.linear(d, d)?,
                k: r.linear(d, d)?,
                v: r.linear(d, d)?,
                o: r.linear(d, d)?,
                norm2: r.layer_norm(d)?,
                ff1: r.linear(ff, d)?,
                ff2: r.linear(d, ff)?,
            });
        }
        let norm = r.layer_norm(d)?;
        let intent = r.linear(intents.len(), d)?;
        let tag = r.linear(tags.len(), d)?;
        if r.pos != data.len() {
            return Err("model file has extra data at the end");
        }
        Ok(Model { intents, tags, d, heads, embed, position, blocks, norm, intent, tag })
    }

    /// Runs the model on `text`.
    pub fn run(&self, text: &str) -> Output {
        let d = self.d;
        let mut tokens = tokenizer::tokenize(text);
        tokens.truncate(MAX_TOKENS);
        let t = tokens.len();
        if t == 0 {
            return Output { tokens, intent_scores: vec![0.0; self.intents.len()], tag_scores: Vec::new() };
        }

        // Each token: the mean of its feature embeddings, plus its position.
        let lower = text.to_ascii_lowercase();
        let mut x = vec![0.0f32; t * d];
        let mut ids = Vec::new();
        for (i, token) in tokens.iter().enumerate() {
            tokenizer::features(&lower.as_bytes()[token.start..token.end], &mut ids);
            let row = &mut x[i * d..(i + 1) * d];
            for &id in &ids {
                for (v, e) in row.iter_mut().zip(&self.embed[id * d..(id + 1) * d]) {
                    *v += e;
                }
            }
            let count = ids.len() as f32;
            for (v, p) in row.iter_mut().zip(&self.position[i * d..(i + 1) * d]) {
                *v = *v / count + p;
            }
        }

        let mut scratch = Vec::new();
        for block in &self.blocks {
            self.block(block, &mut x, t, &mut scratch);
        }
        let mut normed = vec![0.0f32; t * d];
        for i in 0..t {
            self.norm.apply(&x[i * d..(i + 1) * d], &mut scratch);
            normed[i * d..(i + 1) * d].copy_from_slice(&scratch);
        }

        // Intent: from the mean over tokens. Tags: one row per token.
        let mut pooled = vec![0.0f32; d];
        for row in normed.chunks_exact(d) {
            for (p, v) in pooled.iter_mut().zip(row) {
                *p += v;
            }
        }
        pooled.iter_mut().for_each(|p| *p /= t as f32);
        let mut intent_scores = Vec::new();
        self.intent.apply(&pooled, &mut intent_scores);
        let tag_scores = normed
            .chunks_exact(d)
            .map(|row| {
                let mut out = Vec::new();
                self.tag.apply(row, &mut out);
                out
            })
            .collect();
        Output { tokens, intent_scores, tag_scores }
    }

    /// One block: `x + attention(norm(x))`, then `x + feed_forward(norm(x))`.
    fn block(&self, b: &Block, x: &mut [f32], t: usize, scratch: &mut Vec<f32>) {
        let d = self.d;
        let hd = d / self.heads;
        // Queries, keys and values for every token, each [t, d].
        let (mut q, mut k, mut v) = (vec![0.0f32; t * d], vec![0.0f32; t * d], vec![0.0f32; t * d]);
        for i in 0..t {
            b.norm1.apply(&x[i * d..(i + 1) * d], scratch);
            let y = core::mem::take(scratch);
            for (layer, out) in [(&b.q, &mut q), (&b.k, &mut k), (&b.v, &mut v)] {
                layer.apply(&y, scratch);
                out[i * d..(i + 1) * d].copy_from_slice(scratch);
            }
            *scratch = y;
        }
        // Each head attends within its own slice of the d dimensions.
        let scale = 1.0 / libm::sqrtf(hd as f32);
        let mut attended = vec![0.0f32; t * d];
        let mut weights = vec![0.0f32; t];
        for h in 0..self.heads {
            let head = h * hd..(h + 1) * hd;
            for i in 0..t {
                let qi = &q[i * d..][head.clone()];
                for (j, w) in weights.iter_mut().enumerate() {
                    let kj = &k[j * d..][head.clone()];
                    *w = qi.iter().zip(kj).map(|(a, b)| a * b).sum::<f32>() * scale;
                }
                softmax(&mut weights);
                let out = &mut attended[i * d..][head.clone()];
                for (j, w) in weights.iter().enumerate() {
                    for (o, vj) in out.iter_mut().zip(&v[j * d..][head.clone()]) {
                        *o += w * vj;
                    }
                }
            }
        }
        let mut hidden = Vec::new();
        for i in 0..t {
            let row = i * d..(i + 1) * d;
            b.o.apply(&attended[row.clone()], scratch);
            x[row.clone()].iter_mut().zip(scratch.iter()).for_each(|(a, s)| *a += s);
            b.norm2.apply(&x[row.clone()], scratch);
            b.ff1.apply(scratch, &mut hidden);
            hidden.iter_mut().for_each(|h| *h = h.max(0.0));
            b.ff2.apply(&hidden, scratch);
            x[row].iter_mut().zip(scratch.iter()).for_each(|(a, s)| *a += s);
        }
    }
}
