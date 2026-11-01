//! Video: what a codec hands the video player (`ui::apps::video`).
//!
//! A codec implements `VideoDecoder`. The player runs it on a
//! `std::thread` core, which decodes a few frames ahead of the one on
//! screen, so only those few are ever in memory: the video streams through.

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use rust_h264::decoder::OrderedDecoder;
use rust_h264::error::DecodeError;
use rust_h264::nal::{parse_avcc, parse_avcc_config, parse_sps_info};
use crate::ui::gfx::Surface;

pub type VideoError = &'static str;

/// Frames per second as a fraction, e.g. 30000/1001 for NTSC's 29.97.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameRate {
    pub num: u32,
    pub den: u32,
}

impl FrameRate {
    pub const fn new(num: u32, den: u32) -> Self {
        Self { num, den }
    }

    /// How long one frame is shown, in ms (at least 1).
    pub fn frame_ms(&self) -> u64 {
        (self.den as u64 * 1000 / self.num.max(1) as u64).max(1)
    }

    /// When frame `n` (counting from 0) is shown, in ms, for codecs whose
    /// frames are evenly spaced.
    pub fn frame_time_ms(&self, n: u64) -> u64 {
        n * 1000 * self.den as u64 / self.num.max(1) as u64
    }
}

/// What a decoder knows from the file's header.
#[derive(Clone, Debug)]
pub struct VideoInfo {
    pub width: i32,
    pub height: i32,
    pub frame_rate: FrameRate,
    /// None if the file doesn't say.
    pub duration_ms: Option<u64>,
    /// Shown in the title bar, e.g. "MJPEG".
    pub codec: &'static str,
}

/// One decoded picture.
pub struct Frame {
    /// When to show it, in ms from the start of the video.
    pub pts_ms: u64,
    pub image: Surface,
}

/// A picture in YUV 4:2:0, which most codecs decode to: a full-size Y
/// (brightness) plane, and U and V (color) planes at half the width and
/// height, rounded up. Rows are `*_stride` bytes apart, which may be more
/// than the width.
pub struct Yuv420 {
    pub width: i32,
    pub height: i32,
    pub y: Vec<u8>,
    pub y_stride: usize,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
    pub uv_stride: usize,
    /// Y and UV use all of 0-255 (JPEG/MJPEG), rather than 16-235 and
    /// 16-240 (most video: H.264, MPEG, VP8...).
    pub full_range: bool,
}

impl Yuv420 {
    /// Converts to RGB with the BT.601 coefficients.
    pub fn to_surface(&self) -> Surface {
        let mut image = Surface::new(self.width, self.height, 0);
        // Fixed point, 8 fractional bits.
        let (y_mul, y_off, r_v, g_u, g_v, b_u) = if self.full_range {
            (256, 0, 359, 88, 183, 454)
        } else {
            (298, 16, 409, 100, 208, 516)
        };
        let clamp = |c: i32| (c.clamp(0, 255 << 8) >> 8) as u32;
        for row in 0..self.height as usize {
            let y_row = &self.y[row * self.y_stride..];
            let u_row = &self.u[(row / 2) * self.uv_stride..];
            let v_row = &self.v[(row / 2) * self.uv_stride..];
            let out = &mut image.pixels[row * self.width as usize..][..self.width as usize];
            for (x, pixel) in out.iter_mut().enumerate() {
                let y = (y_row[x] as i32 - y_off) * y_mul;
                let u = u_row[x / 2] as i32 - 128;
                let v = v_row[x / 2] as i32 - 128;
                let r = clamp(y + r_v * v + 128);
                let g = clamp(y - g_u * u - g_v * v + 128);
                let b = clamp(y + b_u * u + 128);
                *pixel = (r << 16) | (g << 8) | b;
            }
        }
        image
    }
}

/// A video file's decoder. It's moved to a worker core and called from
/// there only, so it may take its time, but it's never called from two
/// places at once.
pub trait VideoDecoder: Send {
    fn info(&self) -> &VideoInfo;

    /// The next frame in presentation order, or None at the end.
    fn next_frame(&mut self) -> Result<Option<Frame>, VideoError>;

    /// Moves so the next frame is the keyframe at or before `ms` (or the
    /// first frame). The player skips frames before `ms` itself.
    fn seek(&mut self, ms: u64) -> Result<(), VideoError>;
}

// ---- MP4 / MOV -----------------------------------------------------------------

/// Plays the video track of an MP4 (or QuickTime .mov) file.
///
/// The file is a tree of boxes: each starts with its size and a four-letter
/// type. `moov` describes the tracks, and each track's sample table (in
/// `moov/trak/mdia/minf/stbl`) says where its samples -- compressed frames
/// -- are in the file and when to show them. `new` flattens that into
/// `samples`; `next_frame` decodes one sample at a time.
pub struct Mp4Decoder {
    data: Vec<u8>,
    info: VideoInfo,
    /// The codec's setup box from the sample description, if it has one:
    /// `avcC` for H.264 (its SPS and PPS), `hvcC` for H.265, and so on.
    pub config: Option<Vec<u8>>,
    /// Sample times are in 1/timescale seconds.
    timescale: u64,
    samples: Vec<Mp4Sample>,
    /// The next sample to decode.
    next: usize,
    /// For H.264 tracks.
    h264: Option<H264>,
    /// Decoded and not handed out yet: a sample can give several frames.
    pending: VecDeque<Frame>,
}

/// H.264 decoding state, which carries over from sample to sample: most
/// frames are stored as changes to earlier ones.
struct H264 {
    /// Hands frames back in display order, each with the time (ms) of the
    /// sample it came from.
    decoder: OrderedDecoder<u64>,
    /// Bytes in the length before each NAL in a sample.
    length_size: usize,
    /// Y and UV use all of 0-255, as the SPS says.
    full_range: bool,
}

impl H264 {
    /// A decoder primed with the SPS and PPS from the `avcC` box.
    fn new(avcc: &[u8]) -> Result<Self, VideoError> {
        let config = parse_avcc_config(avcc)?;
        let sps = config.sps_nals.first().ok_or("H.264 setup has no SPS")?;
        let full_range = parse_sps_info(sps).map_err(h264_error)?.full_range;
        let mut decoder = OrderedDecoder::new();
        for nal in config.sps_nals.iter().chain(&config.pps_nals) {
            decoder.decode_nal_with_meta(nal, 0).map_err(h264_error)?;
        }
        Ok(Self { decoder, length_size: config.length_size, full_range })
    }

    fn to_frame(&self, (frame, pts_ms): (rust_h264::decoder::Frame, u64)) -> Frame {
        let width = frame.width as usize;
        let yuv = Yuv420 {
            width: frame.width as i32,
            height: frame.height as i32,
            y: frame.y,
            y_stride: width,
            u: frame.u,
            v: frame.v,
            uv_stride: width / 2,
            full_range: self.full_range,
        };
        Frame { pts_ms, image: yuv.to_surface() }
    }
}

fn h264_error(e: DecodeError) -> VideoError {
    match e {
        DecodeError::UnexpectedEof => "H.264: truncated data",
        DecodeError::InvalidSyntax(why) | DecodeError::Unsupported(why) => why,
    }
}

struct Mp4Sample {
    offset: usize,
    size: usize,
    /// When to show it, in `timescale` units.
    time: u64,
    /// A keyframe: decodable without the samples before it.
    sync: bool,
}

impl Mp4Decoder {
    pub fn new(data: Vec<u8>) -> Result<Self, VideoError> {
        let moov = mp4_child(&data, b"moov").ok_or("not an MP4 file (no moov box)")?;
        let trak = mp4_boxes(moov)
            .filter(|(kind, _)| kind == b"trak")
            .map(|(_, trak)| trak)
            .find(|trak| mp4_path(trak, &[b"mdia", b"hdlr"]).and_then(|h| h.get(8..12)) == Some(b"vide"))
            .ok_or("no video track")?;
        let mdia = mp4_child(trak, b"mdia").ok_or("no mdia box")?;
        let mdhd = mp4_child(mdia, b"mdhd").ok_or("no mdhd box")?;
        let (timescale, duration) = if mdhd.first() == Some(&1) {
            (be32(mdhd, 20)? as u64, be64(mdhd, 24)?)
        } else {
            (be32(mdhd, 12)? as u64, be32(mdhd, 16)? as u64)
        };
        if timescale == 0 {
            return Err("MP4 track has no timescale");
        }
        let stbl = mp4_path(mdia, &[b"minf", b"stbl"]).ok_or("no sample table")?;
        let table = |kind: &[u8; 4]| mp4_child(stbl, kind);

        // The sample description: the codec, the frame size, and the
        // codec's setup boxes after a fixed 78-byte header.
        let stsd = table(b"stsd").ok_or("no stsd box")?;
        let (format, entry) = mp4_boxes(stsd.get(8..).unwrap_or(&[])).next().ok_or("empty stsd box")?;
        let (width, height) = (be16(entry, 24)? as i32, be16(entry, 26)? as i32);
        let extra = entry.get(78..).unwrap_or(&[]);
        let config = mp4_boxes(extra)
            .find(|(kind, _)| matches!(kind, b"avcC" | b"hvcC" | b"av1C" | b"vpcC"))
            .map(|(_, body)| body.to_vec());
        let codec = match &format {
            b"avc1" | b"avc3" => "H.264",
            b"hvc1" | b"hev1" => "H.265",
            b"av01" => "AV1",
            b"vp09" => "VP9",
            b"jpeg" | b"mjpa" => "MJPEG",
            // MPEG-4's generic entry; esds says what's really inside.
            b"mp4v" => match mp4_child(extra, b"esds").and_then(esds_object_type) {
                Some(0x6c) => "MJPEG",
                Some(0x20) => "MPEG-4 Part 2",
                _ => "MPEG-4",
            },
            _ => "unknown",
        };

        // Sizes.
        let stsz = table(b"stsz").ok_or("no stsz box (stz2 isn't supported)")?;
        let fixed_size = be32(stsz, 4)? as usize;
        let count = be32(stsz, 8)? as usize;
        if count > data.len() {
            return Err("bad MP4 sample count");
        }
        let size_of = |i: usize| -> Result<usize, VideoError> {
            if fixed_size != 0 { Ok(fixed_size) } else { Ok(be32(stsz, 12 + i * 4)? as usize) }
        };

        // Offsets: samples sit back to back in chunks; stsc says how many
        // per chunk, and stco / co64 where each chunk starts.
        let (chunk_table, wide) = match (table(b"stco"), table(b"co64")) {
            (Some(stco), _) => (stco, false),
            (None, Some(co64)) => (co64, true),
            _ => return Err("no chunk offsets"),
        };
        let chunks = be32(chunk_table, 4)? as usize;
        let chunk_offset = |i: usize| -> Result<usize, VideoError> {
            Ok(if wide { be64(chunk_table, 8 + i * 8)? as usize } else { be32(chunk_table, 8 + i * 4)? as usize })
        };
        let stsc = table(b"stsc").ok_or("no stsc box")?;
        let runs = be32(stsc, 4)? as usize;
        let mut samples = Vec::with_capacity(count);
        'runs: for run in 0..runs {
            let first = be32(stsc, 8 + run * 12)? as usize;
            let per_chunk = be32(stsc, 12 + run * 12)? as usize;
            let end = if run + 1 < runs { be32(stsc, 8 + (run + 1) * 12)? as usize } else { chunks + 1 };
            for chunk in first.max(1)..end.min(chunks + 1) {
                let mut offset = chunk_offset(chunk - 1)?;
                for _ in 0..per_chunk {
                    if samples.len() == count {
                        break 'runs;
                    }
                    let size = size_of(samples.len())?;
                    samples.push(Mp4Sample { offset, size, time: 0, sync: true });
                    offset = offset.saturating_add(size);
                }
            }
        }

        // Times: stts gives decode times as runs of equal gaps; ctts, if
        // there, how far after that each is shown.
        let stts = table(b"stts").ok_or("no stts box")?;
        let mut first_delta = 0;
        let (mut i, mut time) = (0, 0u64);
        for run in 0..be32(stts, 4)? as usize {
            let n = be32(stts, 8 + run * 8)? as usize;
            let delta = be32(stts, 12 + run * 8)? as u64;
            if first_delta == 0 {
                first_delta = delta;
            }
            for sample in samples.iter_mut().skip(i).take(n) {
                sample.time = time;
                time += delta;
            }
            i += n;
        }
        if let Some(ctts) = table(b"ctts") {
            // Version 1 offsets can be negative.
            let signed = ctts.first() == Some(&1);
            let mut i = 0;
            for run in 0..be32(ctts, 4)? as usize {
                let n = be32(ctts, 8 + run * 8)? as usize;
                let raw = be32(ctts, 12 + run * 8)?;
                let offset = if signed { raw as i32 as i64 } else { raw as i64 };
                for sample in samples.iter_mut().skip(i).take(n) {
                    sample.time = (sample.time as i64 + offset).max(0) as u64;
                }
                i += n;
            }
        }
        // Keyframes: without stss, every sample is one.
        if let Some(stss) = table(b"stss") {
            samples.iter_mut().for_each(|s| s.sync = false);
            for k in 0..be32(stss, 4)? as usize {
                if let Some(s) = samples.get_mut((be32(stss, 8 + k * 4)? as usize).wrapping_sub(1)) {
                    s.sync = true;
                }
            }
        }

        let info = VideoInfo {
            width,
            height,
            frame_rate: FrameRate::new(timescale as u32, first_delta.max(1) as u32),
            duration_ms: Some(duration * 1000 / timescale),
            codec,
        };
        let h264 = match codec {
            "H.264" => Some(H264::new(config.as_deref().ok_or("H.264 track has no avcC box")?)?),
            _ => None,
        };
        Ok(Self { data, info, config, timescale, samples, next: 0, h264, pending: VecDeque::new() })
    }
}

impl VideoDecoder for Mp4Decoder {
    fn info(&self) -> &VideoInfo {
        &self.info
    }

    fn next_frame(&mut self) -> Result<Option<Frame>, VideoError> {
        loop {
            if let Some(frame) = self.pending.pop_front() {
                return Ok(Some(frame));
            }
            let Some(sample) = self.samples.get(self.next) else {
                // The end: take what the H.264 decoder is still holding.
                let Some(h264) = &mut self.h264 else { return Ok(None) };
                let rest = h264.decoder.flush_with_meta();
                if rest.is_empty() {
                    return Ok(None);
                }
                self.pending.extend(rest.into_iter().map(|f| h264.to_frame(f)));
                continue;
            };
            let pts_ms = sample.time * 1000 / self.timescale;
            let bytes = sample.offset.checked_add(sample.size)
                .and_then(|end| self.data.get(sample.offset..end))
                .ok_or("MP4 sample is past the end of the file")?;
            self.next += 1;
            match self.info.codec {
                // Every MJPEG sample is a whole JPEG picture.
                "MJPEG" => {
                    let image = crate::ui::img::decode_jpeg(bytes)?;
                    return Ok(Some(Frame { pts_ms, image }));
                }
                // A sample is NALs, each after its length. Most give no
                // frame right away: the decoder holds pictures back to put
                // them in display order.
                "H.264" => {
                    let h264 = self.h264.as_mut().ok_or("no H.264 decoder")?;
                    for nal in parse_avcc(bytes, h264.length_size) {
                        let frames = h264.decoder.decode_nal_with_meta(&nal, pts_ms).map_err(h264_error)?;
                        self.pending.extend(frames.into_iter().map(|f| h264.to_frame(f)));
                    }
                }
                _ => return Err("no decoder for this MP4's codec yet"),
            }
        }
    }

    fn seek(&mut self, ms: u64) -> Result<(), VideoError> {
        let time = ms * self.timescale / 1000;
        let mut i = self.samples.iter().rposition(|s| s.time <= time).unwrap_or(0);
        while i > 0 && !self.samples[i].sync {
            i -= 1;
        }
        self.next = i;
        self.pending.clear();
        // Start over from the keyframe, forgetting the pictures before it.
        if self.h264.is_some() {
            self.h264 = Some(H264::new(self.config.as_deref().unwrap_or(&[]))?);
        }
        Ok(())
    }
}

fn be16(data: &[u8], at: usize) -> Result<u16, VideoError> {
    let b = data.get(at..at + 2).ok_or("truncated MP4")?;
    Ok(u16::from_be_bytes([b[0], b[1]]))
}

fn be32(data: &[u8], at: usize) -> Result<u32, VideoError> {
    let b = data.get(at..at + 4).ok_or("truncated MP4")?;
    Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn be64(data: &[u8], at: usize) -> Result<u64, VideoError> {
    Ok((be32(data, at)? as u64) << 32 | be32(data, at + 4)? as u64)
}

/// The boxes directly inside `data`, as (type, contents). Stops at the
/// first one that doesn't fit.
fn mp4_boxes(data: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    let mut at = 0;
    core::iter::from_fn(move || {
        let size = be32(data, at).ok()? as u64;
        let kind: [u8; 4] = data.get(at + 4..at + 8)?.try_into().ok()?;
        let (header, size) = match size {
            0 => (8, (data.len() - at) as u64), // to the end of the file
            1 => (16, be64(data, at + 8).ok()?), // 64-bit size
            n => (8, n),
        };
        let end = at.checked_add(usize::try_from(size).ok()?)?;
        if size < header || end > data.len() {
            return None;
        }
        let body = &data[at + header as usize..end];
        at = end;
        Some((kind, body))
    })
}

fn mp4_child<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    mp4_boxes(data).find(|(k, _)| k == kind).map(|(_, body)| body)
}

/// Follows `path` down from `data`, e.g. `[b"minf", b"stbl"]`.
fn mp4_path<'a>(data: &'a [u8], path: &[&[u8; 4]]) -> Option<&'a [u8]> {
    path.iter().try_fold(data, |d, kind| mp4_child(d, kind))
}

/// The objectTypeIndication in an `esds` box: which codec an `mp4v`
/// entry holds (0x20 MPEG-4 Part 2, 0x6c JPEG).
fn esds_object_type(esds: &[u8]) -> Option<u8> {
    // Descriptors are a tag, a length of 1-4 bytes (7 bits each, high bit
    // = more), then the contents.
    let skip_header = |d: &[u8], at: usize| -> Option<usize> {
        let mut at = at + 1;
        for _ in 0..4 {
            let b = *d.get(at)?;
            at += 1;
            if b & 0x80 == 0 {
                break;
            }
        }
        Some(at)
    };
    // After the full box header: ES_Descriptor (tag 3).
    if *esds.get(4)? != 0x03 {
        return None;
    }
    let mut at = skip_header(esds, 4)?;
    let flags = *esds.get(at + 2)?;
    at += 3; // ES_ID, flags
    if flags & 0x80 != 0 { at += 2; } // dependsOn_ES_ID
    if flags & 0x40 != 0 { at += 1 + *esds.get(at)? as usize; } // URL
    if flags & 0x20 != 0 { at += 2; } // OCR_ES_ID
    // DecoderConfigDescriptor (tag 4), which starts with the type.
    if *esds.get(at)? != 0x04 {
        return None;
    }
    esds.get(skip_header(esds, at)?).copied()
}
