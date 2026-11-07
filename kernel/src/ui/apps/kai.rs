use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use crate::arch::timer;
use crate::drivers::input::Key;
use crate::ui::apps::App;
use crate::ui::gfx::{Rect, rgb, Surface};

#[derive(PartialEq, Eq)]
enum Sender {
    Ai,
    User
}

pub struct Message {
    pub sender: Sender,
    pub msg: String,
}

impl Message {
    fn new(sender: Sender, msg: String) -> Self {
        Self { sender, msg }
    }
}

pub struct KAI {
    pub message: String,
    pub chat: Vec<Message>,
    cursor_visible: bool,
    cursor_ticks: u8,
}

impl KAI {
    pub fn new() -> Self {
        Self { message: String::new(), chat: Vec::new(), cursor_visible: false, cursor_ticks: 0 }
    }

    fn latest(&self, sender: Sender) -> Option<&Message> {
        self.chat.iter().rev().find(|e| e.sender == sender)
    }

    pub fn say(&mut self, msg: String) {
        self.chat.push(Message::new(Sender::User, msg));
    }

    pub fn reply(&mut self) -> String {
        use crate::assistant::*;
        let args = self.latest(Sender::User).map(|m| m.msg.as_str()).unwrap_or("");
        let mut reply = String::new();
        let (debug, text) = match args.strip_prefix("--debug") {
            Some(rest) => (true, rest.trim()),
            None => (false, args.trim()),
        };
        if text.is_empty() {
            reply.push_str(&format!("K-AI: {}", tools::HELP));
            reply.push_str(&format!("(`kai --debug <request>` shows what I understood, without acting.)"));
            return reply;
        }
        let model = match model() {
            Ok(model) => model,
            Err(e) => {
                reply.push_str(&format!("kai: {}", e));
                return reply;
            }
        };
        let started = timer::now();
        let u = model.understand(text);
        let micros = (timer::now() - started) * 1_000_000 / timer::frequency();
        if debug {
            reply.push_str(&format!("intent   {} ({:.2})   runner-up {} ({:.2})", u.intent, u.confidence, u.second.0, u.second.1));
            for (name, value) in &u.slots {
                reply.push_str(&format!("slot     {} = {:?}", name, value));
            }
            reply.push_str(&format!("took     {} us", micros));
            return reply;
        }
        if u.confidence < CONFIDENT {
            reply.push_str(&format!("K-AI: I'm not sure what you mean. Should I {}, or {}? Try saying it another way.",
                tools::describe(&u.intent), tools::describe(&u.second.0)));
            return reply;
        }
        if tools::dangerous(&u.intent) && u.confidence < tools::DANGEROUS_CONFIDENT {
            reply.push_str(&format!("K-AI: I think you want me to {}, but I'm not sure enough to offer. Try saying it another way.",
                tools::describe(&u.intent)));
            return reply;
        }
        match tools::act(&u, text) {
            tools::Outcome::Done(answer) => reply.push_str(&format!("K-AI: {}", answer)),
            tools::Outcome::Confirm { question, action } => {
                reply.push_str(&format!("K-AI: {} (yes/no)", question));
                //*PENDING.lock() = Some(action);
            }
        }
        return reply;
    }
}

impl App for KAI {
    fn title(&self) -> String {
        String::from("K-AI Chat")
    }

    fn size(&self) -> (i32, i32) {
        (720, 540)
    }

    fn draw(&mut self, s: &mut Surface) {
        fn metrics(msg: &str, max_chars: usize) -> (usize, usize) {
            let mut lines = 1;
            let mut current = 0;
            let mut max_line = 0;
            for word in msg.split_whitespace() {
                let len = word.chars().count();
                if current > 0 && current + len + 1 > max_chars {
                    max_line = max_line.max(current);
                    lines += 1;
                    current = len;
                } else {
                    current += len + if current > 0 { 1 } else { 0 };
                }
            }
            max_line = max_line.max(current);
            (lines, max_line)
        }
        let w = s.width;
        let h = s.height;
        let bg = rgb(20, 22, 29);
        let panel = rgb(29, 32, 42);
        let border = rgb(46, 50, 64);
        let text = rgb(225, 228, 238);
        let muted = rgb(130, 137, 155);
        let accent = rgb(105, 119, 255);
        let ai_bubble = rgb(39, 43, 56);
        let user_bubble = rgb(65, 77, 157);
        s.fill(bg);
        s.fill_rect(Rect::new(0, 0, w, 48), panel);
        s.fill_rect(Rect::new(0, 47, w, 1), border);
        s.fill_rect(Rect::new(14, 11, 26, 26), accent);
        s.draw_text(21, 20, "K", text, 1, 1);
        s.draw_text(50, 10, "K-AI", text, 1, 1);
        s.draw_text(50, 26, "Your personal assistant", muted, 1, 1);
        s.fill_rect(Rect::new(w - 54, 20, 7, 7), rgb(76, 205, 137));
        s.draw_text(w - 42, 19, "AI", muted, 1, 1);
        let chat_area = Rect::new(0, 48, w, h - 112);
        s.fill_rect(chat_area, bg);
        s.set_clip(chat_area);
        let padding = 14;
        let max_width = w - 50;
        let max_chars = ((max_width - 24) / 8) as usize;
        let mut total_height = 0;
        for message in &self.chat {
            let (lines, _) = metrics(&message.msg, max_chars);
            total_height += 14 + lines as i32 * 12 + 16 + 16;
        }
        let mut y = chat_area.bottom() - total_height - 8;
        let mut skip = 0;
        while y < chat_area.y + 14 && skip < self.chat.len() {
            let (lines, _) = metrics(&self.chat[skip].msg, max_chars);
            y += 14 + lines as i32 * 12 + 16 + 16;
            skip += 1;
        }
        y = y.max(chat_area.y + 14);
        for message in self.chat.iter().skip(skip) {
            let outgoing = message.sender == Sender::User;
            let bubble_color = if outgoing { user_bubble } else { ai_bubble };
            let (line_count, max_line) = metrics(&message.msg, max_chars);
            let bubble_width = max_width.min(max_line as i32 * 8 + 24).max(40);
            let bubble_height = line_count as i32 * 12 + 16;
            let x = if outgoing { w - bubble_width - padding } else { padding };
            s.draw_text(if outgoing { x + bubble_width - 32 } else { x }, y, if outgoing { "You" } else { "K-AI" }, muted, 1, 1);
            y += 14;
            s.fill_rect(Rect::new(x, y, bubble_width, bubble_height), bubble_color);
            let mut line_x = x + 12;
            let mut line_y = y + 8;
            let mut current = 0;
            for word in message.msg.split_whitespace() {
                let len = word.chars().count();
                if current > 0 && current + len + 1 > max_chars {
                    line_x = x + 12;
                    line_y += 12;
                    current = 0;
                }
                if current > 0 {
                    line_x += 8;
                    current += 1;
                }
                s.draw_text(line_x, line_y, word, text, 1, 1);
                line_x += len as i32 * 8;
                current += len;
            }
            y += bubble_height + 16;
        }
        s.reset_clip();
        let input_y = h - 64;
        s.fill_rect(Rect::new(0, input_y, w, 64), panel);
        s.fill_rect(Rect::new(0, input_y, w, 1), border);
        s.fill_rect(Rect::new(12, input_y + 12, w - 68, 40), bg);
        s.rect_outline(Rect::new(12, input_y + 12, w - 68, 40), border);
        let display = if self.message.is_empty() { "  Message K-AI..." } else { self.message.as_str() };
        s.draw_text(23, input_y + 28, display, if self.message.is_empty() { muted } else { text }, 1, 1);
        s.fill_rect(Rect::new(w - 48, input_y + 12, 36, 40), accent);
        s.draw_text(w - 37, input_y + 28, ">", text, 1, 1);
        if self.cursor_visible {
            let cursor_x = if self.message.is_empty() { 23 } else { 23 + self.message.chars().count() as i32 * 8 };
            s.draw_text(cursor_x, input_y + 28, "|", text, 1, 1);
        }
    }

    fn on_key(&mut self, key: Key, _ctrl: bool) -> bool {
        match key {
            Key::Char(c) => { self.message.push(c); }
            Key::Backspace => { self.message.pop(); }
            Key::Enter => {
                if !self.message.trim().is_empty() {
                    let msg = core::mem::take(&mut self.message);
                    self.say(msg);
                    let response = self.reply();
                    self.chat.push(Message::new(Sender::Ai, response));
                    self.cursor_visible = true;
                    self.cursor_ticks = 0;
                }
            }
            _ => { return false; }
        }
        true
    }

    fn tick(&mut self) -> bool {
        self.cursor_ticks += 1;
        if self.cursor_ticks >= 30 {
            self.cursor_ticks = 0;
            self.cursor_visible = !self.cursor_visible;
            return true;
        }
        false
    }
}
