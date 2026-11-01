//! virtio-sound (device id 25), playback only: QEMU's
//! `-device virtio-sound-device,audiodev=...`.
//!
//! We open the first output stream as 16-bit PCM and keep it running for
//! good: `sound` fills period buffers (silence included) and hands them to
//! the tx queue, and the device gives each back once it has played it, so
//! the queue paces itself at the sample rate.
//!
//! Control commands go one at a time and spin until they complete, like
//! virtio-gpu's.

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use crate::arch::timer;
use crate::drivers::virtio::mmio::{find_devices, Device, VirtQueue, DESC_F_NEXT, DESC_F_WRITE};
use crate::mm::frame;
use crate::mm::layout::phys_to_virt;
use crate::std::lock::spinlock::Spinlock;

const DEVICE_ID: u32 = 25;
const CONTROLQ: u32 = 0;
const EVENTQ: u32 = 1;
const TXQ: u32 = 2;
const RXQ: u32 = 3;
const CONTROL_QUEUE_SIZE: u16 = 2;
const TX_QUEUE_SIZE: u16 = 32;
const TIMEOUT_MS: u64 = 1000;

// Requests and statuses (virtio spec 5.14.6).
const R_PCM_INFO: u32 = 0x0100;
const R_PCM_SET_PARAMS: u32 = 0x0101;
const R_PCM_PREPARE: u32 = 0x0102;
const R_PCM_START: u32 = 0x0104;
const S_OK: u32 = 0x8000;

const DIRECTION_OUTPUT: u8 = 0;
const FORMAT_S16: u8 = 5;
const RATE_44100: u8 = 6;
const RATE_48000: u8 = 7;
const MAX_STREAMS: usize = 8;

/// Buffers queued at once, each holding PERIOD_MS of sound: enough to ride
/// out the sound task sleeping a couple of 10 ms ticks.
pub const PERIODS: usize = 6;
pub const PERIOD_MS: u32 = 10;

// Config space: jacks, streams, chmaps (u32 each).
const CFG_STREAMS: usize = 4;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct QueryInfo {
    code: u32,
    start_id: u32,
    count: u32,
    size: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct PcmInfo {
    hda_fn_nid: u32,
    features: u32,
    formats: u64,
    rates: u64,
    direction: u8,
    channels_min: u8,
    channels_max: u8,
    padding: [u8; 5],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct PcmHdr {
    code: u32,
    stream_id: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct PcmSetParams {
    hdr: PcmHdr,
    buffer_bytes: u32,
    period_bytes: u32,
    features: u32,
    channels: u8,
    format: u8,
    rate: u8,
    padding: u8,
}

/// Requests are built at the start of the control page, responses land
/// halfway in.
const RESP_OFF: usize = 2048;

// Each tx buffer is one page: the transfer header, the device's status,
// then the samples.
const XFER_OFF: usize = 0;
const STATUS_OFF: usize = 64;
const DATA_OFF: usize = 128;

pub struct Snd {
    dev: Device,
    control: VirtQueue,
    tx: VirtQueue,
    control_page: usize,
    buffers: [usize; PERIODS],
    stream: u32,
    pub rate: u32,
    pub channels: u32,
    /// Frames (samples per channel) in one period.
    pub period_frames: usize,
}

static SND: Spinlock<Option<Snd>> = Spinlock::new(None);
static AVAILABLE: AtomicBool = AtomicBool::new(false);
/// Periods the device has played, for tests and `stats`.
pub static PERIODS_PLAYED: AtomicU32 = AtomicU32::new(0);

impl Snd {
    /// Sends `req` and waits for the response's status (None if the device
    /// never answered).
    fn command<T>(&mut self, req: T, resp_len: usize) -> Option<u32> {
        let req_len = core::mem::size_of::<T>();
        debug_assert!(req_len <= RESP_OFF && resp_len <= RESP_OFF);
        let virt = phys_to_virt(self.control_page);
        unsafe {
            write_volatile(virt as *mut T, req);
            write_volatile((virt + RESP_OFF) as *mut u32, 0);
        }
        self.control.set_buffer(0, self.control_page, req_len as u32, DESC_F_NEXT, 1);
        self.control.set_buffer(1, self.control_page + RESP_OFF, resp_len as u32, DESC_F_WRITE, 0);
        self.control.make_available(0);
        self.control.notify(&self.dev, CONTROLQ);
        let deadline = timer::uptime_ms() + TIMEOUT_MS;
        while self.control.pop_used().is_none() {
            if timer::uptime_ms() > deadline {
                log::error!("command {:#x} timed out", unsafe { read_volatile(virt as *const u32) });
                return None;
            }
            core::hint::spin_loop();
        }
        // Nobody listens for the interrupt; just keep it from staying raised.
        self.dev.ack_interrupt();
        Some(unsafe { read_volatile((virt + RESP_OFF) as *const u32) })
    }

    fn command_ok<T>(&mut self, req: T, what: &'static str) -> Result<(), &'static str> {
        match self.command(req, 4) {
            Some(S_OK) => Ok(()),
            Some(status) => {
                log::error!("{} failed: status {:#x}", what, status);
                Err(what)
            }
            None => Err(what),
        }
    }

    /// The first output stream and what it supports.
    fn find_output(&mut self, streams: u32) -> Option<(u32, PcmInfo)> {
        let count = streams.min(MAX_STREAMS as u32);
        let size = core::mem::size_of::<PcmInfo>();
        let req = QueryInfo { code: R_PCM_INFO, start_id: 0, count, size: size as u32 };
        if self.command(req, 4 + count as usize * size)? != S_OK {
            return None;
        }
        let infos = (phys_to_virt(self.control_page) + RESP_OFF + 4) as *const PcmInfo;
        (0..count).map(|i| (i, unsafe { read_volatile(infos.add(i as usize)) }))
            .find(|(_, info)| info.direction == DIRECTION_OUTPUT)
    }

    /// The samples of tx buffer `i`, to fill before `submit`.
    pub fn samples(&mut self, i: usize) -> &mut [i16] {
        let len = self.period_frames * self.channels as usize;
        unsafe { core::slice::from_raw_parts_mut((phys_to_virt(self.buffers[i]) + DATA_OFF) as *mut i16, len) }
    }

    /// Queues tx buffer `i` for playing.
    pub fn submit(&mut self, i: usize) {
        let page = self.buffers[i];
        let d = (i * 3) as u16;
        let bytes = (self.period_frames * self.channels as usize * 2) as u32;
        unsafe { write_volatile((phys_to_virt(page) + XFER_OFF) as *mut u32, self.stream) };
        self.tx.set_buffer(d, page + XFER_OFF, 4, DESC_F_NEXT, d + 1);
        self.tx.set_buffer(d + 1, page + DATA_OFF, bytes, DESC_F_NEXT, d + 2);
        self.tx.set_buffer(d + 2, page + STATUS_OFF, 8, DESC_F_WRITE, 0);
        self.tx.make_available(d);
        self.tx.notify(&self.dev, TXQ);
    }

    /// A tx buffer the device has finished playing, if any.
    pub fn reap(&mut self) -> Option<usize> {
        let (id, _) = self.tx.pop_used()?;
        self.dev.ack_interrupt();
        PERIODS_PLAYED.fetch_add(1, Ordering::Relaxed);
        Some(id as usize / 3)
    }
}

/// Finds the device and starts its first output stream. The stream then
/// needs a steady supply of periods: see `sound`.
pub fn init() -> Result<(), &'static str> {
    let dev = find_devices(DEVICE_ID).next().ok_or("no sound device")?;
    if !dev.begin_init(0) {
        return Err("sound device rejected our features");
    }
    let control = VirtQueue::new(&dev, CONTROLQ, CONTROL_QUEUE_SIZE).ok_or("control queue unavailable")?;
    // Unused, but set up so the device sees every queue ready.
    VirtQueue::new(&dev, EVENTQ, CONTROL_QUEUE_SIZE).ok_or("event queue unavailable")?;
    let tx = VirtQueue::new(&dev, TXQ, TX_QUEUE_SIZE).ok_or("tx queue unavailable")?;
    VirtQueue::new(&dev, RXQ, CONTROL_QUEUE_SIZE).ok_or("rx queue unavailable")?;
    let control_page = frame::alloc_zeroed().ok_or("out of memory")?;
    let mut buffers = [0; PERIODS];
    for b in &mut buffers {
        *b = frame::alloc_zeroed().ok_or("out of memory")?;
    }
    dev.finish_init();
    let streams = (0..4).fold(0u32, |v, i| v | (dev.read_config_u8(CFG_STREAMS + i) as u32) << (8 * i));
    let mut snd = Snd { dev, control, tx, control_page, buffers, stream: 0, rate: 0, channels: 0, period_frames: 0 };

    let (stream, info) = snd.find_output(streams).ok_or("no output stream")?;
    if info.formats & (1 << FORMAT_S16) == 0 {
        return Err("output stream can't do 16-bit samples");
    }
    let (rate_code, rate) = if info.rates & (1 << RATE_48000) != 0 {
        (RATE_48000, 48000)
    } else if info.rates & (1 << RATE_44100) != 0 {
        (RATE_44100, 44100)
    } else {
        return Err("output stream can't do 48 or 44.1 kHz");
    };
    let channels = info.channels_min.max(1) as u32;
    let period_frames = (rate * PERIOD_MS / 1000) as usize;
    let period_bytes = period_frames as u32 * channels * 2;
    if DATA_OFF + period_bytes as usize > crate::mm::layout::PAGE_SIZE {
        return Err("period doesn't fit a page");
    }
    snd.stream = stream;
    snd.rate = rate;
    snd.channels = channels;
    snd.period_frames = period_frames;

    let hdr = |code| PcmHdr { code, stream_id: stream };
    snd.command_ok(PcmSetParams {
        hdr: hdr(R_PCM_SET_PARAMS),
        buffer_bytes: period_bytes * PERIODS as u32,
        period_bytes,
        features: 0,
        channels: channels as u8,
        format: FORMAT_S16,
        rate: rate_code,
        padding: 0,
    }, "PCM_SET_PARAMS")?;
    snd.command_ok(hdr(R_PCM_PREPARE), "PCM_PREPARE")?;
    // The device wants some sound queued before it starts: silence.
    for i in 0..PERIODS {
        snd.samples(i).fill(0);
        snd.submit(i);
    }
    snd.command_ok(hdr(R_PCM_START), "PCM_START")?;
    log::info!("virtio-snd at slot {}: {} Hz, 16-bit, {} channel(s)", snd.dev.slot(), rate, channels);
    *SND.lock() = Some(snd);
    AVAILABLE.store(true, Ordering::Release);
    Ok(())
}

pub fn available() -> bool {
    AVAILABLE.load(Ordering::Acquire)
}

/// Runs `f` on the device, if there is one.
pub fn with<R>(f: impl FnOnce(&mut Snd) -> R) -> Option<R> {
    SND.lock().as_mut().map(f)
}
