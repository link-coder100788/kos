//! Keyboard and mouse events, independent of which device produced them.
//! Input drivers push into `EVENTS` from IRQ context; the UI task drains it.

use crate::std::sync::spsc::SpscQueue;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// A printable character, already adjusted for Shift.
    Char(char),
    Enter,
    Backspace,
    Tab,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Delete,
    Shift,
    Ctrl,
    Alt,
    CapsLock,
    /// A key with no mapping here: the raw Linux key code.
    Other(u16),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputEvent {
    /// `ctrl` is whether a Ctrl key was held when this key went down.
    Key { key: Key, pressed: bool, ctrl: bool },
    /// Absolute pointer position in screen pixels.
    MouseMove { x: usize, y: usize },
    MouseButton { button: MouseButton, pressed: bool },
    /// Wheel clicks; positive is up.
    Scroll(i32),
}

/// Producers: input IRQ handlers (all on core 0, never nested, so a single
/// logical producer). Consumer: the UI task.
pub static EVENTS: SpscQueue<InputEvent, 256> = SpscQueue::new();

/// Queues an event and wakes the UI. Drops the event if the UI has fallen
/// 256 events behind.
pub fn push(event: InputEvent) {
    crate::stats::inc(&crate::stats::INPUT_EVENTS);
    log::trace!("{:?}", event);
    let _ = EVENTS.push(event);
    crate::ui::task::wake_ui();
}

/// Linux input key codes (include/uapi/linux/input-event-codes.h) for the
/// main block of a US keyboard, unshifted and shifted.
const KEYMAP: [(u8, u8); 58] = [
    (0, 0), (0x1b, 0x1b),
    (b'1', b'!'), (b'2', b'@'), (b'3', b'#'), (b'4', b'$'), (b'5', b'%'),
    (b'6', b'^'), (b'7', b'&'), (b'8', b'*'), (b'9', b'('), (b'0', b')'),
    (b'-', b'_'), (b'=', b'+'), (0x08, 0x08), (b'\t', b'\t'),
    (b'q', b'Q'), (b'w', b'W'), (b'e', b'E'), (b'r', b'R'), (b't', b'T'),
    (b'y', b'Y'), (b'u', b'U'), (b'i', b'I'), (b'o', b'O'), (b'p', b'P'),
    (b'[', b'{'), (b']', b'}'), (b'\n', b'\n'), (0, 0),
    (b'a', b'A'), (b's', b'S'), (b'd', b'D'), (b'f', b'F'), (b'g', b'G'),
    (b'h', b'H'), (b'j', b'J'), (b'k', b'K'), (b'l', b'L'),
    (b';', b':'), (b'\'', b'"'), (b'`', b'~'), (0, 0), (b'\\', b'|'),
    (b'z', b'Z'), (b'x', b'X'), (b'c', b'C'), (b'v', b'V'), (b'b', b'B'),
    (b'n', b'N'), (b'm', b'M'), (b',', b'<'), (b'.', b'>'), (b'/', b'?'),
    (0, 0), (b'*', b'*'), (0, 0), (b' ', b' '),
];

/// Translates a Linux key code, given the Shift / Caps Lock state.
pub fn translate_key(code: u16, shift: bool, caps: bool) -> Key {
    match code {
        1 => Key::Escape,
        14 => Key::Backspace,
        15 => Key::Tab,
        28 | 96 => Key::Enter,
        29 | 97 => Key::Ctrl,
        42 | 54 => Key::Shift,
        56 | 100 => Key::Alt,
        58 => Key::CapsLock,
        102 => Key::Home,
        103 => Key::Up,
        104 => Key::PageUp,
        105 => Key::Left,
        106 => Key::Right,
        107 => Key::End,
        108 => Key::Down,
        109 => Key::PageDown,
        111 => Key::Delete,
        c if (c as usize) < KEYMAP.len() && KEYMAP[c as usize].0 != 0 => {
            let (lower, upper) = KEYMAP[c as usize];
            let letter = lower.is_ascii_lowercase();
            let shifted = if letter { shift ^ caps } else { shift };
            Key::Char(if shifted { upper } else { lower } as char)
        }
        c => Key::Other(c),
    }
}
