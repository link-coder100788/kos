//! Plays a video. Decoding happens in `std::thread` jobs, which keep a few
//! frames decoded ahead in `State::frames`; `tick` shows each one when its
//! time comes. Space plays and pauses, Left / Right skip 5 seconds, Home
//! goes back to the start, and the bar at the bottom seeks.

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use crate::arch::timer;
use crate::drivers::input::{Key, MouseButton};
use crate::std::lock::irq_mutex::IrqMutex;
use crate::ui::apps::{draw_button, App, MouseEvent, BG, FG, ACCENT};
use crate::ui::gfx::{text_width, Rect, Surface, GLYPH_H};
use crate::ui::video::{Frame, VideoDecoder, VideoError, VideoInfo};

const MAX_W: i32 = 960;
const MAX_H: i32 = 640;
/// Narrowest window, so the controls fit.
const MIN_W: i32 = 320;
const BAR_H: i32 = 32;
const BUTTON_W: i32 = 64;
const SKIP_MS: u64 = 5000;
/// Memory for frames decoded ahead.
const BUFFER_BYTES: usize = 24 * 1024 * 1024;

/// What the player and its decode jobs share.
struct Shared {
    state: IrqMutex<State>,
}

struct State {
    /// None while a decode job has it.
    decoder: Option<Box<dyn VideoDecoder>>,
    /// A decode job is queued or running.
    job: bool,
    /// Decoded and not shown yet, oldest first.
    frames: VecDeque<Frame>,
    /// How many frames to keep decoded ahead.
    capacity: usize,
    /// A seek the decoder hasn't done yet.
    seek_to: Option<u64>,
    /// The decoder has nothing more (until a seek).
    ended: bool,
    error: Option<VideoError>,
    /// The window closed: a running job drops the decoder and returns.
    closed: bool,
}

impl Shared {
    /// Queues a decode job if there's work and none is queued already.
    fn kick(self: &Arc<Self>) {
        let mut s = self.state.lock();
        let wanted = s.seek_to.is_some() || (!s.ended && s.frames.len() < s.capacity);
        if s.job || s.closed || !wanted {
            return;
        }
        s.job = true;
        drop(s);
        let shared = self.clone();
        crate::std::thread::spawn(move || decode(shared));
    }
}

/// A decode job: decodes until `capacity` frames are waiting (or the video
/// ends), then returns, so several videos -- and other jobs -- take turns on
/// the thread core. The player's `tick` queues the next one.
fn decode(shared: Arc<Shared>) {
    let Some(mut decoder) = shared.state.lock().decoder.take() else { return };
    loop {
        let seek = {
            let mut s = shared.state.lock();
            if s.closed {
                return;
            }
            let seek = s.seek_to.take();
            if seek.is_none() && (s.ended || s.frames.len() >= s.capacity) {
                s.decoder = Some(decoder);
                s.job = false;
                return;
            }
            seek
        };
        if let Some(ms) = seek {
            if let Err(e) = decoder.seek(ms) {
                let mut s = shared.state.lock();
                s.error = Some(e);
                s.ended = true;
            }
            continue;
        }
        // Decoding takes a while: not under the lock.
        let result = decoder.next_frame();
        let mut s = shared.state.lock();
        // Decoded from before a seek that came in meanwhile.
        if s.seek_to.is_some() {
            continue;
        }
        match result {
            Ok(Some(frame)) => s.frames.push_back(frame),
            Ok(None) => s.ended = true,
            Err(e) => {
                s.error = Some(e);
                s.ended = true;
            }
        }
    }
}

/// The position in the video, in ms; it runs while playing.
struct Clock {
    base_ms: u64,
    /// Uptime when it was last started, or None while paused.
    started: Option<u64>,
}

impl Clock {
    fn now(&self) -> u64 {
        self.base_ms + self.started.map_or(0, |t| timer::uptime_ms() - t)
    }

    fn playing(&self) -> bool {
        self.started.is_some()
    }

    fn set(&mut self, ms: u64) {
        self.base_ms = ms;
        if self.started.is_some() {
            self.started = Some(timer::uptime_ms());
        }
    }

    fn play(&mut self) {
        if self.started.is_none() {
            self.started = Some(timer::uptime_ms());
        }
    }

    fn pause(&mut self) {
        self.base_ms = self.now();
        self.started = None;
    }
}

/// The biggest `iw` x `ih` rectangle (keeping its shape) that fits in a
/// `w` x `h` area, centered in it.
fn fit(iw: i32, ih: i32, w: i32, h: i32) -> Rect {
    let (iw, ih, w, h) = (iw.max(1) as i64, ih.max(1) as i64, w.max(1) as i64, h.max(1) as i64);
    let (vw, vh) = if iw * h > ih * w { (w, (ih * w / iw).max(1)) } else { ((iw * h / ih).max(1), h) };
    Rect::new(((w - vw) / 2) as i32, ((h - vh) / 2) as i32, vw as i32, vh as i32)
}

pub struct VideoPlayer {
    name: String,
    info: VideoInfo,
    shared: Arc<Shared>,
    /// On screen.
    current: Option<Frame>,
    clock: Clock,
    /// Size of the window's content area.
    w: i32,
    h: i32,
    /// Where frames are drawn.
    video: Rect,
    /// While dragging along the seek bar: where it'll seek on release.
    scrub: Option<u64>,
    /// The time shown in the bar, in whole seconds, to redraw when it moves.
    shown_s: u64,
    /// The video reached its end and stopped there.
    at_end: bool,
}

impl VideoPlayer {
    /// Starts decoding on the thread core; it plays once the window opens.
    pub fn new(name: String, decoder: Box<dyn VideoDecoder>) -> Self {
        let info = decoder.info().clone();
        // Opens at the video's own size, shrunk to fit if it's big.
        let (iw, ih) = (info.width.max(1), info.height.max(1));
        let (vw, vh) = if iw <= MAX_W && ih <= MAX_H - BAR_H {
            (iw, ih)
        } else {
            let r = fit(iw, ih, MAX_W, MAX_H - BAR_H);
            (r.w, r.h)
        };
        let w = vw.max(MIN_W);
        let h = vh + BAR_H;
        let frame_bytes = iw as usize * ih as usize * 4;
        let shared = Arc::new(Shared {
            state: IrqMutex::new(State {
                decoder: Some(decoder),
                job: false,
                frames: VecDeque::new(),
                capacity: (BUFFER_BYTES / frame_bytes).clamp(2, 16),
                seek_to: None,
                ended: false,
                error: None,
                closed: false,
            }),
        });
        shared.kick();
        Self {
            name,
            info,
            shared,
            current: None,
            clock: Clock { base_ms: 0, started: Some(timer::uptime_ms()) },
            w,
            h,
            video: fit(iw, ih, w, h - BAR_H),
            scrub: None,
            shown_s: 0,
            at_end: false,
        }
    }

    fn seek(&mut self, ms: u64) {
        let ms = self.info.duration_ms.map_or(ms, |d| ms.min(d));
        {
            let mut s = self.shared.state.lock();
            s.frames.clear();
            s.seek_to = Some(ms);
            s.ended = false;
            s.error = None;
        }
        self.shared.kick();
        self.clock.set(ms);
        self.at_end = false;
    }

    fn toggle(&mut self) {
        if self.clock.playing() {
            self.clock.pause();
        } else {
            if self.at_end {
                self.seek(0);
            }
            self.clock.play();
        }
    }

    fn button_rect(&self) -> Rect {
        Rect::new(6, self.h - BAR_H + 4, BUTTON_W, BAR_H - 8)
    }

    /// The time as text: "1:05 / 3:20", or "1:05" when the length isn't known.
    fn time_text(&self, ms: u64) -> String {
        let mmss = |ms: u64| format!("{}:{:02}", ms / 60_000, ms / 1000 % 60);
        match self.info.duration_ms {
            Some(d) => format!("{} / {}", mmss(ms), mmss(d)),
            None => mmss(ms),
        }
    }

    /// The seek bar's track. Its width doesn't depend on the time shown, so
    /// it doesn't jitter.
    fn track_rect(&self) -> Rect {
        let widest = self.time_text(self.info.duration_ms.unwrap_or(0).max(600_000));
        let left = 6 + BUTTON_W + 10;
        let right = self.w - 10 - text_width(&widest, 1);
        Rect::new(left, self.h - BAR_H / 2 - 3, (right - left).max(0), 6)
    }

    /// Where in the video `x` on the seek bar is, if the length is known.
    fn track_ms(&self, x: i32) -> Option<u64> {
        let d = self.info.duration_ms?;
        let track = self.track_rect();
        let x = (x - track.x).clamp(0, track.w.max(1));
        Some(d * x as u64 / track.w.max(1) as u64)
    }

    fn on_track(&self, x: i32, y: i32) -> bool {
        let track = self.track_rect();
        x >= track.x - 4 && x <= track.right() + 4 && y >= self.h - BAR_H
    }
}

impl App for VideoPlayer {
    fn title(&self) -> String {
        format!("{} ({})", self.name, self.info.codec)
    }

    fn size(&self) -> (i32, i32) {
        (self.w, self.h)
    }

    fn resizable(&self) -> bool {
        true
    }

    fn min_size(&self) -> (i32, i32) {
        (MIN_W, BAR_H + 60)
    }

    fn resize(&mut self, w: i32, h: i32) {
        self.w = w;
        self.h = h;
        self.video = fit(self.info.width, self.info.height, w, h - BAR_H);
    }

    fn draw(&mut self, s: &mut Surface) {
        s.fill(0);
        let status = self.shared.state.lock().error;
        match (&self.current, status) {
            (_, Some(e)) => {
                s.draw_text(10, 10, e, 0x00ff8080, 1, 2);
            }
            (Some(frame), None) => s.blit_scaled(&frame.image, self.video),
            (None, None) => {
                s.draw_text(10, 10, "Loading...", 0x00c0c0c0, 1, 2);
            }
        }

        // Controls.
        let bar = Rect::new(0, self.h - BAR_H, self.w, BAR_H);
        s.fill_rect(bar, BG);
        let label = if self.clock.playing() { "Pause" } else { "Play" };
        draw_button(s, self.button_rect(), label, false);
        let pos = self.scrub.unwrap_or_else(|| self.clock.now());
        let track = self.track_rect();
        s.fill_rect(track, 0x00c8ccd4);
        if let Some(d) = self.info.duration_ms.filter(|&d| d > 0) {
            let done = (track.w as u64 * pos.min(d) / d) as i32;
            s.fill_rect(Rect::new(track.x, track.y, done, track.h), ACCENT);
            s.fill_rect(Rect::new(track.x + done - 3, track.y - 4, 6, track.h + 8), FG);
        }
        let text = self.time_text(pos);
        let tx = self.w - 6 - text_width(&text, 1);
        s.draw_text(tx, self.h - BAR_H / 2 - GLYPH_H, &text, FG, 1, 2);
        self.shown_s = pos / 1000;
    }

    fn on_key(&mut self, key: Key, _ctrl: bool) -> bool {
        match key {
            Key::Char(' ') => self.toggle(),
            Key::Left => self.seek(self.clock.now().saturating_sub(SKIP_MS)),
            Key::Right => self.seek(self.clock.now() + SKIP_MS),
            Key::Home => self.seek(0),
            _ => return false,
        }
        true
    }

    fn on_mouse(&mut self, event: MouseEvent) -> bool {
        match event {
            MouseEvent::Down { x, y, button: MouseButton::Left } => {
                if self.button_rect().contains(x, y) || self.video.contains(x, y) {
                    self.toggle();
                    true
                } else if self.on_track(x, y) {
                    self.scrub = self.track_ms(x);
                    true
                } else {
                    false
                }
            }
            MouseEvent::Move { x, dragging: true, .. } if self.scrub.is_some() => {
                self.scrub = self.track_ms(x);
                true
            }
            MouseEvent::Up { button: MouseButton::Left, .. } => match self.scrub.take() {
                Some(ms) => {
                    self.seek(ms);
                    true
                }
                None => false,
            },
            _ => false,
        }
    }

    fn tick(&mut self) -> bool {
        let mut changed = false;
        let now = self.clock.now();
        let mut s = self.shared.state.lock();
        // Show the newest frame that's due; any older ones came too late.
        let mut taken = false;
        while s.frames.front().is_some_and(|f| f.pts_ms <= now) {
            self.current = s.frames.pop_front();
            taken = true;
        }
        if s.frames.is_empty() && self.clock.playing() {
            if !s.ended {
                // The decoder is behind: hold the clock, so it slows down
                // instead of skipping frames.
                self.clock.pause();
                self.clock.play();
            } else if self.current.as_ref().map_or(true, |f| now >= f.pts_ms + self.info.frame_rate.frame_ms()) {
                // The last frame has had its turn.
                self.clock.pause();
                self.at_end = true;
                changed = true;
            }
        }
        if s.error.is_some() && self.clock.playing() {
            self.clock.pause();
            changed = true;
        }
        changed |= taken;
        drop(s);
        self.shared.kick();
        changed || self.scrub.is_none() && self.clock.now() / 1000 != self.shown_s
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        let mut s = self.shared.state.lock();
        s.closed = true;
        s.frames.clear();
    }
}
