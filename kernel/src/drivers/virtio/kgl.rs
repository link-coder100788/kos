//! kos-gl (device id 63): the host's OpenGL, for gl.h. It's KOS's own
//! device, in the vendored QEMU (tools/qemu/hw/display/kos-gl.c), so only
//! the QEMU built from there has it.
//!
//! Programs get GL contexts, each the size of a canvas. gl.h builds a
//! stream of commands in the program's memory and `submit`s it, and the
//! device draws with the Mac's GPU, then (when asked) hands back the frame
//! for the program to present. The commands are the device's business:
//! all this driver does is carry them, keep each context to the program
//! that opened it, and close a program's contexts when it ends.
//!
//! One request at a time, as a two-descriptor chain (the request and its
//! payload, then the response and room for its data), each in a kernel
//! heap buffer (which is physically contiguous). The caller sleeps until
//! the device's interrupt.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use crate::arch::timer;
use crate::drivers::virtio::mmio::{find_devices, Device, VirtQueue, DESC_F_NEXT, DESC_F_WRITE};
use crate::mm::layout::virt_to_phys;
use crate::scheduler::{self, wait::WaitQueue, TaskId};
use crate::std::lock::spinlock::Spinlock;

const DEVICE_ID: u32 = 63;
const QUEUE: u32 = 0;
const QUEUE_SIZE: u16 = 2;
/// Compiling HLSL runs a compiler on the host, which can take a moment.
const TIMEOUT_MS: u64 = 10_000;

const REQ_CREATE: u32 = 1;
const REQ_DESTROY: u32 = 2;
const REQ_SUBMIT: u32 = 3;
const REQ_HLSL: u32 = 4;
const REQ_GLSL: u32 = 5;
const STATUS_OK: u32 = 0;

/// The biggest context, and command stream, a program can have.
pub const MAX_SIZE: u32 = 2048;
pub const MAX_COMMANDS: usize = 8 << 20;
/// Room for an error message, or a compiler's.
const MESSAGE_ROOM: usize = 16 << 10;
const REQUEST_LEN: usize = 16;
const RESPONSE_LEN: usize = 8;

struct Kgl {
    dev: Device,
    queue: VirtQueue,
}

static KGL: Spinlock<Option<Kgl>> = Spinlock::new(None);
/// The device's MMIO base, for the interrupt handler.
static BASE: AtomicUsize = AtomicUsize::new(0);
/// A request is in flight.
static BUSY: AtomicBool = AtomicBool::new(false);
/// Woken by the device's interrupt, and when a request finishes.
static WAIT: WaitQueue = WaitQueue::new();

/// Each open context: its owner and size.
struct Context {
    owner: TaskId,
    width: u32,
    height: u32,
}

static CONTEXTS: Spinlock<BTreeMap<u32, Context>> = Spinlock::new(BTreeMap::new());
static NEXT_CONTEXT: AtomicU32 = AtomicU32::new(1);

fn irq() {
    let base = BASE.load(Ordering::Acquire);
    if base != 0 {
        Device { base }.ack_interrupt();
    }
    WAIT.wake_all();
}

/// Finds the device, if this QEMU has it.
pub fn init() {
    let Some(dev) = find_devices(DEVICE_ID).next() else { return };
    if !dev.begin_init(0) {
        log::warn!("kos-gl: device rejected our features");
        return;
    }
    let Some(queue) = VirtQueue::new(&dev, QUEUE, QUEUE_SIZE) else {
        log::warn!("kos-gl: queue unavailable");
        return;
    };
    BASE.store(dev.base, Ordering::Release);
    crate::arch::gic::register(crate::arch::gic::VIRTIO_IRQ_BASE + dev.slot(), irq);
    dev.finish_init();
    log::info!("kos-gl (host OpenGL) at slot {}", dev.slot());
    *KGL.lock() = Some(Kgl { dev, queue });
}

pub fn available() -> bool {
    KGL.lock().is_some()
}

/// Sends a request and waits for the answer: its status and data (at
/// most `room` bytes).
fn request(op: u32, ctx: u32, a: u32, b: u32, payload: &[u8], room: usize) -> Result<(u32, Vec<u8>), String> {
    let mut req = Vec::with_capacity(REQUEST_LEN + payload.len());
    for word in [op, ctx, a, b] {
        req.extend_from_slice(&word.to_le_bytes());
    }
    req.extend_from_slice(payload);
    let mut resp = vec![0u8; RESPONSE_LEN + room];

    WAIT.wait_until(|| BUSY.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok());
    let sent = {
        let mut kgl = KGL.lock();
        match kgl.as_mut() {
            Some(kgl) => {
                kgl.queue.set_buffer(0, virt_to_phys(req.as_ptr() as usize), req.len() as u32, DESC_F_NEXT, 1);
                kgl.queue.set_buffer(1, virt_to_phys(resp.as_mut_ptr() as usize), resp.len() as u32, DESC_F_WRITE, 0);
                kgl.queue.make_available(0);
                kgl.queue.notify(&kgl.dev, QUEUE);
                true
            }
            None => false,
        }
    };
    let done = sent && WAIT.wait_until_deadline(Some(timer::deadline_after_ms(TIMEOUT_MS)), || {
        KGL.lock().as_mut().is_some_and(|kgl| kgl.queue.pop_used().is_some())
    });
    BUSY.store(false, Ordering::Release);
    WAIT.wake_all();
    if !sent {
        return Err(String::from("no kos-gl device"));
    }
    if !done {
        // The buffers stay the device's: leak them rather than let it
        // write into freed memory.
        core::mem::forget(req);
        core::mem::forget(resp);
        log::error!("kos-gl: request {} timed out", op);
        return Err(String::from("the GPU didn't answer"));
    }
    let word = |i: usize| u32::from_le_bytes(resp[i..i + 4].try_into().unwrap());
    let (status, len) = (word(0), (word(4) as usize).min(room));
    resp.drain(..RESPONSE_LEN);
    resp.truncate(len);
    Ok((status, resp))
}

fn message(data: &[u8]) -> String {
    String::from_utf8_lossy(data).into_owned()
}

/// Opens a width x height context for the calling task. Returns its id.
pub fn open(width: u32, height: u32) -> Result<u32, String> {
    if width == 0 || height == 0 || width > MAX_SIZE || height > MAX_SIZE {
        return Err(String::from("bad size"));
    }
    let ctx = NEXT_CONTEXT.fetch_add(1, Ordering::Relaxed);
    match request(REQ_CREATE, ctx, width, height, &[], MESSAGE_ROOM)? {
        (STATUS_OK, _) => {
            CONTEXTS.lock().insert(ctx, Context { owner: scheduler::current_id(), width, height });
            Ok(ctx)
        }
        (_, data) => Err(message(&data)),
    }
}

/// The size of context `ctx`, if the calling task owns it.
pub fn size(ctx: u32) -> Option<(u32, u32)> {
    let me = scheduler::current_id();
    CONTEXTS.lock().get(&ctx).filter(|c| c.owner == me).map(|c| (c.width, c.height))
}

/// Runs a stream of commands. With `frame`, then reads the frame into it
/// (width * height 0x00RRGGBB pixels, as bytes).
pub fn submit(ctx: u32, commands: &[u8], frame: Option<&mut [u8]>) -> Result<(), String> {
    let (width, height) = size(ctx).ok_or_else(|| String::from("no such context"))?;
    if commands.len() > MAX_COMMANDS {
        return Err(String::from("too many commands at once"));
    }
    let frame_len = width as usize * height as usize * 4;
    let room = if frame.is_some() { frame_len.max(MESSAGE_ROOM) } else { MESSAGE_ROOM };
    match request(REQ_SUBMIT, ctx, frame.is_some() as u32, 0, commands, room)? {
        (STATUS_OK, data) => {
            if let Some(frame) = frame {
                let n = data.len().min(frame.len());
                frame[..n].copy_from_slice(&data[..n]);
            }
            Ok(())
        }
        (_, data) => Err(message(&data)),
    }
}

/// Compiles HLSL shaders as program `program` of context `ctx` (the
/// request is kos-hlslc's). Ok(None) if they compiled, Ok(Some(messages))
/// if not.
pub fn hlsl(ctx: u32, program: u32, request_text: &[u8]) -> Result<Option<String>, String> {
    shaders(REQ_HLSL, ctx, program, request_text)
}

/// Compiles GLSL shaders (the device's GLSL request) as program
/// `program` of context `ctx`, or with program 0 only checks them. As
/// `hlsl` returns.
pub fn glsl(ctx: u32, program: u32, request_text: &[u8]) -> Result<Option<String>, String> {
    shaders(REQ_GLSL, ctx, program, request_text)
}

fn shaders(op: u32, ctx: u32, program: u32, request_text: &[u8]) -> Result<Option<String>, String> {
    size(ctx).ok_or_else(|| String::from("no such context"))?;
    if request_text.len() > MAX_COMMANDS {
        return Err(String::from("shaders too big"));
    }
    match request(op, ctx, program, 0, request_text, MESSAGE_ROOM)? {
        (STATUS_OK, _) => Ok(None),
        (_, data) => Ok(Some(message(&data))),
    }
}

/// Closes context `ctx`, if the calling task owns it.
pub fn close(ctx: u32) {
    if size(ctx).is_some() {
        CONTEXTS.lock().remove(&ctx);
        let _ = request(REQ_DESTROY, ctx, 0, 0, &[], MESSAGE_ROOM);
    }
}

/// Closes every context task `id` left open.
pub fn task_ended(id: TaskId) {
    let ended: Vec<u32> = {
        let mut contexts = CONTEXTS.lock();
        let ids: Vec<u32> = contexts.iter().filter(|(_, c)| c.owner == id).map(|(&ctx, _)| ctx).collect();
        for ctx in &ids {
            contexts.remove(ctx);
        }
        ids
    };
    for ctx in ended {
        let _ = request(REQ_DESTROY, ctx, 0, 0, &[], MESSAGE_ROOM);
    }
}
