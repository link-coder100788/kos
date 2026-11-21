//! The Login window: what the desktop shows until someone logs in. It
//! shares the session with the console (see `users`): logging in here
//! logs the shell in too. Checking a password takes a while (Argon2id),
//! so it happens on a thread, not the UI task.

use alloc::string::String;
use alloc::sync::Arc;
use crate::drivers::input::{Key, MouseButton};
use crate::std::lock::irq_mutex::IrqMutex;
use crate::ui::apps::{App, MouseEvent, ACCENT, BG, FG};
use crate::ui::gfx::{Rect, Surface};

const W: i32 = 380;
const H: i32 = 230;
const NAME_BOX: Rect = Rect { x: 130, y: 70, w: 220, h: 30 };
const PASSWORD_BOX: Rect = Rect { x: 130, y: 112, w: 220, h: 30 };
const BUTTON: Rect = Rect { x: 250, y: 156, w: 100, h: 30 };
const ERROR: u32 = 0x00b03030;
const MUTED: u32 = 0x00707070;

#[derive(PartialEq, Clone, Copy)]
enum Field {
    Name,
    Password,
}

/// A password check on its way: None until it's done, then whether it
/// was right.
type Check = Arc<IrqMutex<Option<bool>>>;

pub struct Login {
    name: String,
    password: String,
    field: Field,
    checking: Option<Check>,
    message: &'static str,
}

impl Login {
    pub fn new() -> Self {
        Self { name: String::new(), password: String::new(), field: Field::Name, checking: None, message: "" }
    }

    fn submit(&mut self) {
        if self.checking.is_some() {
            return;
        }
        if self.name.is_empty() {
            self.field = Field::Name;
            return;
        }
        let check: Check = Arc::new(IrqMutex::new(None));
        let (name, password, done) = (self.name.clone(), core::mem::take(&mut self.password), check.clone());
        crate::std::thread::spawn(move || {
            let account = crate::users::check_password(&name, &password);
            if let Some(a) = &account {
                crate::users::set_session_uid(a.uid);
            }
            *done.lock() = Some(account.is_some());
            crate::ui::task::wake_ui();
        });
        self.checking = Some(check);
        self.message = "Checking...";
    }

    fn draw_box(s: &mut Surface, r: Rect, text: &str, focused: bool) {
        s.fill_rect(r, 0x00ffffff);
        s.rect_outline(r, if focused { ACCENT } else { 0x00a0a0a0 });
        let width = s.draw_text(r.x + 8, r.y + 7, text, FG, 1, 2);
        if focused {
            s.fill_rect(Rect::new(r.x + 9 + width, r.y + 6, 2, r.h - 12), FG);
        }
    }
}

impl App for Login {
    fn title(&self) -> String {
        String::from("Log in")
    }

    fn size(&self) -> (i32, i32) {
        (W, H)
    }

    fn draw(&mut self, s: &mut Surface) {
        s.fill(BG);
        s.draw_text(20, 16, "Welcome to KOS", ACCENT, 2, 3);
        s.draw_text(20, NAME_BOX.y + 7, "Name", FG, 1, 2);
        s.draw_text(20, PASSWORD_BOX.y + 7, "Password", FG, 1, 2);
        Self::draw_box(s, NAME_BOX, &self.name, self.field == Field::Name);
        let dots: String = self.password.chars().map(|_| '*').collect();
        Self::draw_box(s, PASSWORD_BOX, &dots, self.field == Field::Password);
        let color = if self.checking.is_some() { MUTED } else { ERROR };
        s.draw_text(20, BUTTON.y + 7, self.message, color, 1, 2);
        s.fill_rect(BUTTON, if self.checking.is_some() { MUTED } else { ACCENT });
        s.draw_text(BUTTON.x + 22, BUTTON.y + 7, "Log in", 0x00ffffff, 1, 2);
        s.draw_text(20, H - 26, "Or log in at the console. Tab: next field", MUTED, 1, 1);
    }

    fn on_key(&mut self, key: Key, _ctrl: bool) -> bool {
        if self.checking.is_some() {
            return false;
        }
        let text = match self.field {
            Field::Name => &mut self.name,
            Field::Password => &mut self.password,
        };
        match key {
            Key::Char(c) if (' '..='~').contains(&c) && text.len() < 64 => text.push(c),
            Key::Backspace => { text.pop(); }
            Key::Tab | Key::Up | Key::Down => {
                self.field = if self.field == Field::Name { Field::Password } else { Field::Name };
            }
            Key::Enter if self.field == Field::Name => self.field = Field::Password,
            Key::Enter => self.submit(),
            _ => return false,
        }
        true
    }

    fn on_mouse(&mut self, event: MouseEvent) -> bool {
        let MouseEvent::Down { x, y, button: MouseButton::Left } = event else { return false };
        if NAME_BOX.contains(x, y) {
            self.field = Field::Name;
        } else if PASSWORD_BOX.contains(x, y) {
            self.field = Field::Password;
        } else if BUTTON.contains(x, y) {
            self.submit();
        } else {
            return false;
        }
        true
    }

    fn tick(&mut self) -> bool {
        let Some(result) = self.checking.as_ref().and_then(|c| *c.lock()) else { return false };
        self.checking = None;
        // Right: the UI task swaps this window for the desktop.
        if !result {
            self.message = "Wrong name or password";
            self.field = Field::Password;
        }
        true
    }
}
