//! Windows opened by programs (the create-window syscall): labels and
//! buttons. Activating a button queues an event the program can poll.

use alloc::string::String;
use alloc::vec::Vec;
use crate::drivers::input::{Key, MouseButton};
use crate::ui::apps::{draw_button, App, MouseEvent, BG};
use crate::ui::gfx::{text_width, Rect, Surface, GLYPH_H};
use crate::ui::window::{push_event, WindowElementDesc, WindowEvent, WindowId};

enum Element {
    Label { x: i32, y: i32, text: String, color: u32, scale: i32 },
    Button { rect: Rect, label: String },
}

pub struct ProgramWindow {
    id: WindowId,
    title: String,
    elements: Vec<Element>,
    size: (i32, i32),
    /// Index (into `elements`) of the focused button.
    focus: Option<usize>,
    /// Button the mouse went down on.
    pressed: Option<usize>,
}

impl ProgramWindow {
    pub fn new(id: WindowId, title: String, descs: Vec<WindowElementDesc>) -> Self {
        let elements: Vec<Element> = descs.into_iter().map(|d| match d {
            WindowElementDesc::Label { x, y, text, color, scale } => {
                Element::Label { x: x as i32, y: y as i32, text, color, scale: scale.max(1) as i32 }
            }
            WindowElementDesc::Button { x, y, w, h, label } => {
                Element::Button { rect: Rect::new(x as i32, y as i32, w as i32, h as i32), label }
            }
        }).collect();
        let extent = elements.iter().fold(Rect::EMPTY, |acc, e| acc.union(match e {
            Element::Label { x, y, text, scale, .. } => Rect::new(*x, *y, text_width(text, *scale), GLYPH_H * *scale),
            Element::Button { rect, .. } => *rect,
        }));
        let size = ((extent.right() + 16).max(240), (extent.bottom() + 16).max(80));
        let focus = elements.iter().position(|e| matches!(e, Element::Button { .. }));
        Self { id, title, elements, size, focus, pressed: None }
    }

    fn button_at(&self, x: i32, y: i32) -> Option<usize> {
        self.elements.iter().position(|e| matches!(e, Element::Button { rect, .. } if rect.contains(x, y)))
    }

    fn activate(&self, index: usize) {
        push_event(self.id, WindowEvent::ElementActivated(index as u32));
    }
}

impl App for ProgramWindow {
    fn title(&self) -> String {
        self.title.clone()
    }

    fn size(&self) -> (i32, i32) {
        self.size
    }

    fn draw(&mut self, s: &mut Surface) {
        s.fill(BG);
        for (i, e) in self.elements.iter().enumerate() {
            match e {
                Element::Label { x, y, text, color, scale } => {
                    s.draw_text(*x, *y, text, *color, *scale, *scale);
                }
                Element::Button { rect, label } => draw_button(s, *rect, label, self.focus == Some(i)),
            }
        }
    }

    fn on_key(&mut self, key: Key, _ctrl: bool) -> bool {
        match key {
            Key::Tab => {
                let buttons: Vec<usize> = (0..self.elements.len()).filter(|&i| matches!(self.elements[i], Element::Button { .. })).collect();
                if buttons.is_empty() { return false; }
                let next = match self.focus.and_then(|f| buttons.iter().position(|&b| b == f)) {
                    Some(p) => buttons[(p + 1) % buttons.len()],
                    None => buttons[0],
                };
                self.focus = Some(next);
                true
            }
            Key::Enter | Key::Char(' ') => {
                if let Some(f) = self.focus { self.activate(f); }
                false
            }
            _ => false,
        }
    }

    fn on_mouse(&mut self, event: MouseEvent) -> bool {
        match event {
            MouseEvent::Down { x, y, button: MouseButton::Left } => {
                self.pressed = self.button_at(x, y);
                if self.pressed.is_some() && self.pressed != self.focus {
                    self.focus = self.pressed;
                    return true;
                }
                false
            }
            MouseEvent::Up { x, y, button: MouseButton::Left } => {
                if let Some(p) = self.pressed.take() {
                    if self.button_at(x, y) == Some(p) {
                        self.activate(p);
                    }
                }
                false
            }
            _ => false,
        }
    }

    fn on_close(&mut self) {
        push_event(self.id, WindowEvent::Closed);
    }
}
