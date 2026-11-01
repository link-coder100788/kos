//! virtio-input (device id 18): keyboards, tablets and mice, e.g. QEMU's
//! `-device virtio-keyboard-device -device virtio-tablet-device`.
//!
//! Each device has one queue the device fills with 8-byte Linux-style
//! input events. We keep every buffer posted; the interrupt handler turns
//! finished ones into `drivers::input` events and posts them again.

use alloc::string::String;
use alloc::vec::Vec;
use core::ptr::read_volatile;
use crate::arch::gic;
use crate::drivers::input::{self, InputEvent, Key, MouseButton};
use crate::drivers::display::{HEIGHT, WIDTH};
use crate::drivers::virtio::mmio::{find_devices, Device, VirtQueue};
use crate::mm::frame;
use crate::mm::layout::phys_to_virt;
use crate::println;
use crate::std::lock::irq_mutex::IrqMutex;

const DEVICE_ID: u32 = 18;
const QUEUE_SIZE: u16 = 64;
const EVENT_SIZE: usize = 8;

// Event types and codes (Linux input-event-codes.h).
const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const EV_REL: u16 = 0x02;
const EV_ABS: u16 = 0x03;
const BTN_LEFT: u16 = 0x110;
const BTN_RIGHT: u16 = 0x111;
const BTN_MIDDLE: u16 = 0x112;
const KEY_CAPSLOCK: u16 = 58;
const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;
const REL_X: u16 = 0x00;
const REL_Y: u16 = 0x01;
const REL_WHEEL: u16 = 0x08;

// Config space selectors.
const CFG_ID_NAME: u8 = 0x01;
const CFG_ABS_INFO: u8 = 0x12;

#[repr(C)]
#[derive(Clone, Copy)]
struct RawEvent {
    ty: u16,
    code: u16,
    value: u32,
}

struct InputDevice {
    dev: Device,
    queue: VirtQueue,
    /// Kernel address of the QUEUE_SIZE event buffers.
    buffers: usize,
    /// Largest ABS_X / ABS_Y value, for tablets.
    abs_max: (u32, u32),
}

struct State {
    devices: Vec<InputDevice>,
    shift: bool,
    ctrl: bool,
    caps: bool,
    mouse: (usize, usize),
    mouse_moved: bool,
}

static STATE: IrqMutex<State> = IrqMutex::new(State {
    devices: Vec::new(),
    shift: false,
    ctrl: false,
    caps: false,
    mouse: (WIDTH / 2, HEIGHT / 2),
    mouse_moved: false,
});

/// Reads one config-space item; returns its bytes.
fn config_query(dev: &Device, select: u8, subsel: u8) -> Vec<u8> {
    dev.write_config_u8(0, select);
    dev.write_config_u8(1, subsel);
    let size = dev.read_config_u8(2) as usize;
    (0..size).map(|i| dev.read_config_u8(8 + i)).collect()
}

fn abs_max(dev: &Device, axis: u8) -> u32 {
    let info = config_query(dev, CFG_ABS_INFO, axis);
    if info.len() >= 8 { u32::from_le_bytes(info[4..8].try_into().unwrap()) } else { 0 }
}

fn init_device(dev: Device) -> Option<InputDevice> {
    if !dev.begin_init(0) {
        log::warn!("slot {} rejected our features", dev.slot());
        return None;
    }
    let mut queue = VirtQueue::new(&dev, 0, QUEUE_SIZE)?;
    let buffers_phys = frame::alloc_zeroed()?;
    for i in 0..QUEUE_SIZE {
        queue.set_writable_buffer(i, buffers_phys + i as usize * EVENT_SIZE, EVENT_SIZE as u32);
        queue.make_available(i);
    }
    let name = String::from_utf8_lossy(&config_query(&dev, CFG_ID_NAME, 0)).into_owned();
    let abs_max = (abs_max(&dev, ABS_X as u8), abs_max(&dev, ABS_Y as u8));
    dev.finish_init();
    queue.notify(&dev, 0);
    log::info!("'{}' at slot {} (abs max {:?})", name.trim(), dev.slot(), abs_max);
    Some(InputDevice { dev, queue, buffers: phys_to_virt(buffers_phys), abs_max })
}

/// Finds and starts every virtio-input device.
pub fn init() {
    let mut found = Vec::new();
    for dev in find_devices(DEVICE_ID) {
        if let Some(device) = init_device(dev) {
            found.push(device.dev.slot());
            STATE.lock().devices.push(device);
        }
    }
    for slot in found {
        gic::register(gic::VIRTIO_IRQ_BASE + slot, input_irq);
    }
}

fn scale(value: u32, max: u32, size: usize) -> usize {
    if max == 0 { return 0; }
    (value.min(max) as usize * (size - 1)) / max as usize
}

fn handle_event(state: &mut State, abs_max: (u32, u32), ev: RawEvent) {
    match ev.ty {
        EV_KEY => {
            let pressed = ev.value != 0; // 1 = down, 2 = auto-repeat, 0 = up
            let button = match ev.code {
                BTN_LEFT => Some(MouseButton::Left),
                BTN_RIGHT => Some(MouseButton::Right),
                BTN_MIDDLE => Some(MouseButton::Middle),
                _ => None,
            };
            if let Some(button) = button {
                input::push(InputEvent::MouseButton { button, pressed });
                return;
            }
            if ev.code == KEY_CAPSLOCK && ev.value == 1 {
                state.caps = !state.caps;
            }
            let key = input::translate_key(ev.code, state.shift, state.caps);
            match key {
                Key::Shift => state.shift = pressed,
                Key::Ctrl => state.ctrl = pressed,
                _ => {}
            }
            input::push(InputEvent::Key { key, pressed, ctrl: state.ctrl });
        }
        EV_ABS => {
            match ev.code {
                ABS_X => state.mouse.0 = scale(ev.value, abs_max.0, WIDTH),
                ABS_Y => state.mouse.1 = scale(ev.value, abs_max.1, HEIGHT),
                _ => return,
            }
            state.mouse_moved = true;
        }
        EV_REL => match ev.code {
            REL_X | REL_Y => {
                let delta = ev.value as i32 as isize;
                let (pos, size) = if ev.code == REL_X { (&mut state.mouse.0, WIDTH) } else { (&mut state.mouse.1, HEIGHT) };
                *pos = (*pos as isize + delta).clamp(0, size as isize - 1) as usize;
                state.mouse_moved = true;
            }
            REL_WHEEL => input::push(InputEvent::Scroll(ev.value as i32)),
            _ => {}
        },
        EV_SYN => {
            // A report is complete: send one move for all its axis updates.
            if state.mouse_moved {
                state.mouse_moved = false;
                input::push(InputEvent::MouseMove { x: state.mouse.0, y: state.mouse.1 });
            }
        }
        _ => {}
    }
}

/// Shared by every input device; checks them all.
fn input_irq() {
    let mut state = STATE.lock();
    let state = &mut *state;
    for i in 0..state.devices.len() {
        state.devices[i].dev.ack_interrupt();
        let mut reposted = false;
        while let Some((id, _len)) = state.devices[i].queue.pop_used() {
            let device = &state.devices[i];
            let ev = unsafe { read_volatile((device.buffers + id as usize * EVENT_SIZE) as *const RawEvent) };
            let abs_max = device.abs_max;
            handle_event(state, abs_max, ev);
            state.devices[i].queue.make_available(id);
            reposted = true;
        }
        if reposted {
            let device = &state.devices[i];
            device.queue.notify(&device.dev, 0);
        }
    }
}
