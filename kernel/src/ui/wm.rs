//! The compositor and window manager.
//!
//! Draws the desktop, windows (back to front) and taskbar into a back
//! buffer, then copies just the changed region ("damage") to the screen.
//! The mouse cursor is drawn straight onto the screen on top of that, so
//! moving it only costs a couple of small copies.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use crate::arch::timer;
use crate::drivers::input::{InputEvent, Key, MouseButton};
use crate::drivers::display;
use crate::ui::apps::{self, App, MouseEvent};
use crate::ui::gfx::{rgb, text_width, Rect, Surface};
use crate::ui::window::{Request, WindowId};

pub const SCREEN_W: i32 = display::WIDTH as i32;
pub const SCREEN_H: i32 = display::HEIGHT as i32;
const SCREEN: Rect = Rect::new(0, 0, SCREEN_W, SCREEN_H);

const TITLE_H: i32 = 24;
const BORDER: i32 = 1;
const CLOSE_SIZE: i32 = 16;
const TASKBAR_H: i32 = 32;
const TASKBAR: Rect = Rect::new(0, SCREEN_H - TASKBAR_H, SCREEN_W, TASKBAR_H);
const START: Rect = Rect::new(4, SCREEN_H - TASKBAR_H + 4, 64, TASKBAR_H - 8);
const TASK_BUTTON_W: i32 = 150;
/// How far from a resizable window's edge the mouse can grab it, either
/// side of the border.
const GRAB: i32 = 4;
/// Along an edge, this close to a corner grabs the corner.
const CORNER: i32 = 14;
/// A second click on a title bar within this long maximizes.
const DOUBLE_CLICK_MS: u64 = 400;
/// Where maximized windows go.
const WORK_AREA: Rect = Rect::new(0, 0, SCREEN_W, SCREEN_H - TASKBAR_H);
const MENU_ITEM_H: i32 = 30;
const MENU_W: i32 = 170;

const TITLE_FOCUSED: (u32, u32) = (rgb(0x4a, 0x84, 0xc4), rgb(0x2c, 0x5a, 0x92));
const TITLE_UNFOCUSED: (u32, u32) = (rgb(0x7a, 0x7f, 0x8a), rgb(0x5c, 0x61, 0x6b));

/// Entries in the KOS menu.
const MENU: [(&str, fn() -> Box<dyn App>); 7] = [
    ("Terminal", || Box::new(apps::terminal::Terminal::new())),
    ("Browser", || Box::new(apps::browser::Browser::new())),
    ("Programs", || Box::new(apps::programs::Programs::new())),
    ("Task Manager", || Box::new(apps::taskmgr::TaskManager::new())),
    ("Paint", || Box::new(apps::paint::Paint::new())),
    ("K-AI", || Box::new(apps::kai::KAI::new())),
    ("About KOS", || Box::new(apps::about::About::new())),
];

/// Edges of a window being resized, as bits.
const LEFT: u8 = 1;
const RIGHT: u8 = 2;
const TOP: u8 = 4;
const BOTTOM: u8 = 8;

// 12x19 arrow: 'X' outline, '.' fill.
const CURSOR: [&[u8; 12]; 19] = [
    b"X           ", b"XX          ", b"X.X         ", b"X..X        ", b"X...X       ",
    b"X....X      ", b"X.....X     ", b"X......X    ", b"X.......X   ", b"X........X  ",
    b"X.........X ", b"X......XXXXX", b"X...X..X    ", b"X..XX..X    ", b"X.X  X..X   ",
    b"XX   X..X   ", b"X     X..X  ", b"      X..X  ", b"       XX   ",
];
const CURSOR_W: i32 = 12;
const CURSOR_H: i32 = 19;

// A 15x7 double arrow, for resizing sideways; turned on its side for up and
// down.
const RESIZE_CURSOR: [&[u8; 15]; 7] = [
    b"   XX     XX   ", b"  X.X     X.X  ", b" X..XXXXXXX..X ", b"X.............X",
    b" X..XXXXXXX..X ", b"  X.X     X.X  ", b"   XX     XX   ",
];
// An 11x11 diagonal double arrow, top left to bottom right; mirrored for
// the other diagonal.
const DIAGONAL_CURSOR: [&[u8; 11]; 11] = [
    b"XXXXX      ", b"X...X      ", b"X..X       ", b"X.X.X      ", b"XX X.X     ", b"    X.X    ",
    b"     X.X XX", b"      X.X.X", b"       X..X", b"      X...X", b"      XXXXX",
];

/// The mouse pointer's shape.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pointer {
    Arrow,
    /// Over (or dragging) these edges of a window.
    Resize(u8),
}

impl Pointer {
    /// Width and height, and where the hot spot is inside it.
    fn layout(self) -> (i32, i32, i32, i32) {
        match self {
            Pointer::Arrow => (CURSOR_W, CURSOR_H, 0, 0),
            Pointer::Resize(edges) if edges & (TOP | BOTTOM) == 0 => (15, 7, 7, 3),
            Pointer::Resize(edges) if edges & (LEFT | RIGHT) == 0 => (7, 15, 3, 7),
            Pointer::Resize(_) => (11, 11, 5, 5),
        }
    }

    /// The pixel at (col, row): b'X' outline, b'.' fill, b' ' none.
    fn pixel(self, col: usize, row: usize) -> u8 {
        match self {
            Pointer::Arrow => CURSOR[row][col],
            Pointer::Resize(edges) if edges & (TOP | BOTTOM) == 0 => RESIZE_CURSOR[row][col],
            Pointer::Resize(edges) if edges & (LEFT | RIGHT) == 0 => RESIZE_CURSOR[col][row],
            // Top left or bottom right: as drawn; the other corners mirror.
            Pointer::Resize(edges) if (edges & LEFT != 0) == (edges & TOP != 0) => DIAGONAL_CURSOR[row][col],
            Pointer::Resize(_) => DIAGONAL_CURSOR[row][10 - col],
        }
    }

    /// The screen rectangle it covers with its hot spot at `at`.
    fn rect(self, at: (i32, i32)) -> Rect {
        let (w, h, hx, hy) = self.layout();
        Rect::new(at.0 - hx, at.1 - hy, w, h)
    }
}

struct Window {
    id: WindowId,
    title: String,
    /// Top-left of the frame (border included).
    x: i32,
    y: i32,
    surface: Surface,
    app: Box<dyn App>,
    /// Rolled up to just its title bar.
    shaded: bool,
    /// The app lets the user resize the window.
    resizable: bool,
    /// While maximized: the frame to go back to.
    restore: Option<Rect>,
}

impl Window {
    fn frame(&self) -> Rect {
        Rect::new(self.x, self.y, self.surface.width + 2 * BORDER, self.content_height() + TITLE_H + 2 * BORDER)
    }

    /// Empty (zero height) while the window is shaded.
    fn content(&self) -> Rect {
        Rect::new(self.x + BORDER, self.y + BORDER + TITLE_H, self.surface.width, self.content_height())
    }

    fn content_height(&self) -> i32 {
        if self.shaded { 0 } else { self.surface.height }
    }

    fn title_bar(&self) -> Rect {
        Rect::new(self.x + BORDER, self.y + BORDER, self.surface.width, TITLE_H)
    }

    fn close_box(&self) -> Rect {
        let bar = self.title_bar();
        Rect::new(bar.right() - CLOSE_SIZE - 4, bar.y + (TITLE_H - CLOSE_SIZE) / 2, CLOSE_SIZE, CLOSE_SIZE)
    }

    /// The button that maximizes the window and back (resizable ones only).
    fn maximize_box(&self) -> Rect {
        self.close_box().offset(-CLOSE_SIZE - 4, 0)
    }

    /// The button that rolls the window up to its title bar and back.
    fn shade_box(&self) -> Rect {
        let buttons = if self.resizable { 2 } else { 1 };
        self.close_box().offset(-(CLOSE_SIZE + 4) * buttons, 0)
    }

    /// Which edges (if any) of the window's frame (x, y) is close enough to
    /// grab, near a corner counting as both of its edges.
    fn edges_at(&self, x: i32, y: i32) -> u8 {
        if !self.resizable || self.shaded || self.restore.is_some() {
            return 0;
        }
        let f = self.frame();
        let near = |a: i32, edge: i32| (a - edge).abs() <= GRAB;
        let inside_x = x >= f.x - GRAB && x < f.right() + GRAB;
        let inside_y = y >= f.y - GRAB && y < f.bottom() + GRAB;
        let mut edges = 0;
        if inside_y && near(x, f.x) { edges |= LEFT; }
        if inside_y && near(x, f.right() - 1) { edges |= RIGHT; }
        if inside_x && near(y, f.y) { edges |= TOP; }
        if inside_x && near(y, f.bottom() - 1) { edges |= BOTTOM; }
        if edges & (TOP | BOTTOM) != 0 {
            if x < f.x + CORNER { edges |= LEFT; } else if x >= f.right() - CORNER { edges |= RIGHT; }
        }
        if edges & (LEFT | RIGHT) != 0 {
            if y < f.y + CORNER { edges |= TOP; } else if y >= f.bottom() - CORNER { edges |= BOTTOM; }
        }
        edges
    }

    /// Smallest frame size, from the app's smallest content size.
    fn min_frame(&self) -> (i32, i32) {
        let (w, h) = self.app.min_size();
        // Room for the title and its buttons, whatever the app says.
        (w.max(4 * CLOSE_SIZE + 60) + 2 * BORDER, h.max(0) + TITLE_H + 2 * BORDER)
    }

    fn redraw_content(&mut self) {
        self.app.draw(&mut self.surface);
    }
}

pub struct Wm {
    back: Surface,
    background: Surface,
    /// Back to front: the last window is on top.
    windows: Vec<Window>,
    /// Window ids in the order they were opened, for the taskbar.
    taskbar: Vec<WindowId>,
    focused: Option<WindowId>,
    damage: Rect,
    cursor: (i32, i32),
    pointer: Pointer,
    /// Where the cursor currently is on screen, and its shape, if drawn.
    cursor_drawn: Option<((i32, i32), Pointer)>,
    /// Window being moved, and the grab point relative to its corner.
    dragging: Option<(WindowId, i32, i32)>,
    /// Window being resized: the edges being dragged, its frame and the
    /// mouse position when the drag started.
    resizing: Option<(WindowId, u8, Rect, (i32, i32))>,
    /// The last click on a title bar and when, to spot double clicks.
    title_click: Option<(WindowId, u64)>,
    /// Window that got the last mouse-down; it gets moves and the mouse-up.
    capture: Option<WindowId>,
    menu_open: bool,
    menu_hover: Option<usize>,
    clock_second: u64,
    cascade: i32,
}

fn draw_background(s: &mut Surface) {
    s.fill_gradient(SCREEN, rgb(0x1b, 0x2a, 0x49), rgb(0x46, 0x2c, 0x5a));
    let text = "KOS";
    let scale = 10;
    let x = SCREEN_W - text_width(text, scale) - 40;
    s.draw_text(x, SCREEN_H - TASKBAR_H - 8 * scale - 30, text, rgb(0x5a, 0x5e, 0x8a), scale, scale);
}

impl Wm {
    pub fn new() -> Self {
        let mut background = Surface::new(SCREEN_W, SCREEN_H, 0);
        draw_background(&mut background);
        Self {
            back: Surface::new(SCREEN_W, SCREEN_H, 0),
            background,
            windows: Vec::new(),
            taskbar: Vec::new(),
            focused: None,
            damage: SCREEN,
            cursor: (SCREEN_W / 2, SCREEN_H / 2),
            pointer: Pointer::Arrow,
            cursor_drawn: None,
            dragging: None,
            resizing: None,
            title_click: None,
            capture: None,
            menu_open: false,
            menu_hover: None,
            clock_second: u64::MAX,
            cascade: 0,
        }
    }

    fn damage(&mut self, rect: Rect) {
        self.damage = self.damage.union(rect.intersect(SCREEN));
    }

    fn index_of(&self, id: WindowId) -> Option<usize> {
        self.windows.iter().position(|w| w.id == id)
    }

    // ---- window management ------------------------------------------------

    pub fn open(&mut self, id: WindowId, app: Box<dyn App>) {
        let (w, h) = app.size();
        let frame_w = w + 2 * BORDER;
        let frame_h = h + TITLE_H + 2 * BORDER;
        // Cascade new windows from the top left, wrapping before they'd
        // run off the screen.
        let step = 32 * self.cascade;
        let x = (40 + step).min((SCREEN_W - frame_w).max(0));
        let y = (30 + step).min((SCREEN_H - TASKBAR_H - frame_h).max(0));
        self.cascade = (self.cascade + 1) % 8;
        log::debug!("open window {} '{}' {}x{} at ({}, {})", id, app.title(), w, h, x, y);
        let resizable = app.resizable();
        let mut window = Window {
            id, title: app.title(), x, y, surface: Surface::new(w, h, apps::BG), app, shaded: false, resizable, restore: None,
        };
        window.redraw_content();
        self.damage(window.frame());
        self.windows.push(window);
        self.taskbar.push(id);
        self.focus(id);
    }

    pub fn close(&mut self, id: WindowId) {
        let Some(i) = self.index_of(id) else { return };
        let mut window = self.windows.remove(i);
        log::debug!("close window {} '{}'", id, window.title);
        window.app.on_close();
        self.taskbar.retain(|&t| t != id);
        self.damage(window.frame());
        self.damage(TASKBAR);
        if self.focused == Some(id) {
            self.focused = None;
            if let Some(top) = self.windows.last() {
                let top = top.id;
                self.focus(top);
            }
        }
        if self.capture == Some(id) { self.capture = None; }
        if matches!(self.dragging, Some((d, _, _)) if d == id) { self.dragging = None; }
        if matches!(self.resizing, Some((r, ..)) if r == id) { self.resizing = None; }
    }

    /// Raises a window to the top and gives it the keyboard.
    fn focus(&mut self, id: WindowId) {
        let Some(i) = self.index_of(id) else { return };
        let window = self.windows.remove(i);
        self.damage(window.frame());
        if let Some(old) = self.focused.and_then(|f| self.index_of(f)) {
            let frame = self.windows[old].frame();
            self.damage(frame);
        }
        if self.focused != Some(id) {
            log::trace!("focus window {} '{}'", id, window.title);
        }
        self.windows.push(window);
        self.focused = Some(id);
        self.damage(TASKBAR);
    }

    fn toggle_shade(&mut self, i: usize) {
        let old = self.windows[i].frame();
        let window = &mut self.windows[i];
        window.shaded = !window.shaded;
        log::trace!("{} window {} '{}'", if window.shaded { "shade" } else { "unshade" }, window.id, window.title);
        let new = window.frame();
        self.damage(old.union(new));
    }

    /// Moves and resizes window `i` to `frame`, telling the app if its
    /// content area changed size.
    fn set_frame(&mut self, i: usize, frame: Rect) {
        let old = self.windows[i].frame();
        let window = &mut self.windows[i];
        window.x = frame.x;
        window.y = frame.y;
        let (w, h) = (frame.w - 2 * BORDER, frame.h - TITLE_H - 2 * BORDER);
        if (w, h) != (window.surface.width, window.surface.height) {
            window.app.resize(w, h);
            window.surface = Surface::new(w, h, apps::BG);
            window.redraw_content();
        }
        let new = self.windows[i].frame();
        self.damage(old.union(new));
    }

    fn toggle_maximize(&mut self, i: usize) {
        if !self.windows[i].resizable { return; }
        if self.windows[i].shaded {
            self.toggle_shade(i);
        }
        let to = match self.windows[i].restore.take() {
            Some(restore) => restore,
            None => {
                self.windows[i].restore = Some(self.windows[i].frame());
                WORK_AREA
            }
        };
        let window = &self.windows[i];
        log::trace!("{} window {} '{}'", if window.restore.is_some() { "maximize" } else { "restore" }, window.id, window.title);
        self.set_frame(i, to);
    }

    /// The frame window `i` would have with `edges` dragged by (dx, dy)
    /// from `start`, kept at least its minimum size and on the screen.
    fn resized_frame(&self, i: usize, edges: u8, start: Rect, dx: i32, dy: i32) -> Rect {
        let (min_w, min_h) = self.windows[i].min_frame();
        let (mut left, mut top, mut right, mut bottom) = (start.x, start.y, start.right(), start.bottom());
        if edges & LEFT != 0 { left = (left + dx).clamp(0, right - min_w); }
        if edges & RIGHT != 0 { right = (right + dx).clamp(left + min_w, SCREEN_W.max(left + min_w)); }
        if edges & TOP != 0 { top = (top + dy).clamp(0, bottom - min_h); }
        if edges & BOTTOM != 0 { bottom = (bottom + dy).clamp(top + min_h, WORK_AREA.bottom().max(top + min_h)); }
        Rect::new(left, top, right - left, bottom - top)
    }

    fn window_at(&self, x: i32, y: i32) -> Option<usize> {
        self.windows.iter().rposition(|w| w.frame().contains(x, y))
    }

    /// The window the mouse is on, and the edges it would grab there. Just
    /// outside a resizable window's border still counts as on it, unless
    /// another window is on top there.
    fn hit(&self, x: i32, y: i32) -> Option<(usize, u8)> {
        let i = self.windows.iter().rposition(|w| w.frame().contains(x, y) || w.edges_at(x, y) != 0)?;
        Some((i, self.windows[i].edges_at(x, y)))
    }

    pub fn handle_request(&mut self, req: Request) {
        match req {
            Request::Open { id, app } => self.open(id, app),
            Request::Close(id) => self.close(id),
        }
    }

    // ---- input -------------------------------------------------------------

    pub fn handle_input(&mut self, event: InputEvent) {
        match event {
            InputEvent::MouseMove { x, y } => self.mouse_move(x as i32, y as i32),
            InputEvent::MouseButton { button, pressed } => {
                let (x, y) = self.cursor;
                if pressed { self.mouse_down(x, y, button) } else { self.mouse_up(x, y, button) }
            }
            InputEvent::Scroll(clicks) => {
                if let Some(i) = self.focused.and_then(|f| self.index_of(f)) {
                    self.send_mouse(i, MouseEvent::Scroll(clicks));
                }
            }
            InputEvent::Key { key, pressed: true, ctrl } => {
                if key == Key::Escape && self.menu_open {
                    self.set_menu(false);
                } else if let Some(i) = self.focused.and_then(|f| self.index_of(f)) {
                    if self.windows[i].app.on_key(key, ctrl) {
                        self.redraw_window(i);
                    }
                }
            }
            InputEvent::Key { .. } => {}
        }
    }

    fn redraw_window(&mut self, i: usize) {
        self.windows[i].redraw_content();
        let content = self.windows[i].content();
        self.damage(content);
    }

    fn send_mouse(&mut self, i: usize, event: MouseEvent) {
        if self.windows[i].app.on_mouse(event) {
            self.redraw_window(i);
        }
    }

    fn mouse_move(&mut self, x: i32, y: i32) {
        self.cursor = (x, y);
        if let Some((id, edges, start, (sx, sy))) = self.resizing {
            if let Some(i) = self.index_of(id) {
                let frame = self.resized_frame(i, edges, start, x - sx, y - sy);
                self.set_frame(i, frame);
            }
            return;
        }
        self.pointer = match self.hit(x, y) {
            Some((_, edges)) if edges != 0 && self.capture.is_none() && self.dragging.is_none() => Pointer::Resize(edges),
            _ => Pointer::Arrow,
        };
        if let Some((id, gx, gy)) = self.dragging {
            if let Some(i) = self.index_of(id) {
                let old = self.windows[i].frame();
                // Keep the title bar reachable.
                self.windows[i].x = (x - gx).clamp(-old.w + 60, SCREEN_W - 60);
                self.windows[i].y = (y - gy).clamp(0, SCREEN_H - TASKBAR_H - TITLE_H);
                let new = self.windows[i].frame();
                self.damage(old.union(new));
            }
            return;
        }
        if self.menu_open {
            let hover = self.menu_item_at(x, y);
            if hover != self.menu_hover {
                self.menu_hover = hover;
                self.damage(self.menu_rect());
            }
        }
        // Moves go to the window holding the mouse, otherwise the one under it.
        let target = match self.capture {
            Some(id) => self.index_of(id).map(|i| (i, true)),
            None => self.window_at(x, y).map(|i| (i, false)),
        };
        if let Some((i, dragging)) = target {
            let c = self.windows[i].content();
            self.send_mouse(i, MouseEvent::Move { x: x - c.x, y: y - c.y, dragging });
        }
    }

    fn mouse_down(&mut self, x: i32, y: i32, button: MouseButton) {
        if self.menu_open {
            let item = self.menu_item_at(x, y);
            self.set_menu(false);
            if let Some(item) = item {
                self.open(crate::ui::window::next_id(), (MENU[item].1)());
            }
            if item.is_some() || START.contains(x, y) {
                return;
            }
        }
        if TASKBAR.contains(x, y) {
            if START.contains(x, y) {
                self.set_menu(true);
            } else if let Some(i) = self.task_button_at(x, y) {
                let id = self.taskbar[i];
                self.focus(id);
                let top = self.windows.len() - 1;
                if self.windows[top].shaded {
                    self.toggle_shade(top);
                }
            }
            return;
        }
        let Some((i, edges)) = self.hit(x, y) else {
            // Clicked the desktop.
            if let Some(f) = self.focused.take().and_then(|f| self.index_of(f)) {
                let frame = self.windows[f].frame();
                self.damage(frame);
                self.damage(TASKBAR);
            }
            return;
        };
        let id = self.windows[i].id;
        self.focus(id);
        let i = self.windows.len() - 1;
        let window = &self.windows[i];
        if window.close_box().contains(x, y) {
            self.close(id);
        } else if window.resizable && window.maximize_box().contains(x, y) {
            self.toggle_maximize(i);
        } else if window.shade_box().contains(x, y) {
            self.toggle_shade(i);
        } else if edges != 0 && button == MouseButton::Left {
            self.resizing = Some((id, edges, window.frame(), (x, y)));
            self.pointer = Pointer::Resize(edges);
        } else if window.title_bar().contains(x, y) {
            let now = timer::uptime_ms();
            let double = matches!(self.title_click, Some((last, at)) if last == id && now - at <= DOUBLE_CLICK_MS);
            self.title_click = if double { None } else { Some((id, now)) };
            if double {
                self.toggle_maximize(i);
                return;
            }
            if let Some(restore) = window.restore {
                // Dragging a maximized window out: back to its old size,
                // with the cursor the same way along its title bar.
                let frame = window.frame();
                let gx = ((x - frame.x) as i64 * restore.w as i64 / frame.w.max(1) as i64) as i32;
                self.windows[i].restore = None;
                self.set_frame(i, Rect::new(x - gx, frame.y, restore.w, restore.h));
                self.dragging = Some((id, gx, y - frame.y));
            } else {
                self.dragging = Some((id, x - window.x, y - window.y));
            }
        } else if window.content().contains(x, y) {
            let c = window.content();
            self.capture = Some(id);
            self.send_mouse(i, MouseEvent::Down { x: x - c.x, y: y - c.y, button });
        }
    }

    fn mouse_up(&mut self, x: i32, y: i32, button: MouseButton) {
        self.dragging = None;
        if self.resizing.take().is_some() {
            self.pointer = match self.hit(x, y) {
                Some((_, edges)) if edges != 0 => Pointer::Resize(edges),
                _ => Pointer::Arrow,
            };
        }
        if let Some(i) = self.capture.take().and_then(|id| self.index_of(id)) {
            let c = self.windows[i].content();
            self.send_mouse(i, MouseEvent::Up { x: x - c.x, y: y - c.y, button });
        }
    }

    // ---- taskbar & menu ----------------------------------------------------

    fn task_button(&self, i: usize) -> Rect {
        Rect::new(START.right() + 8 + i as i32 * (TASK_BUTTON_W + 4), START.y, TASK_BUTTON_W, START.h)
    }

    fn task_button_at(&self, x: i32, y: i32) -> Option<usize> {
        (0..self.taskbar.len()).find(|&i| self.task_button(i).contains(x, y))
    }

    fn menu_rect(&self) -> Rect {
        let h = MENU.len() as i32 * MENU_ITEM_H + 8;
        Rect::new(START.x, TASKBAR.y - h, MENU_W, h)
    }

    fn menu_item_at(&self, x: i32, y: i32) -> Option<usize> {
        let menu = self.menu_rect();
        if !menu.contains(x, y) { return None; }
        let i = ((y - menu.y - 4) / MENU_ITEM_H) as usize;
        (i < MENU.len()).then_some(i)
    }

    fn set_menu(&mut self, open: bool) {
        self.menu_open = open;
        self.menu_hover = None;
        self.damage(self.menu_rect());
        self.damage(START);
    }

    fn draw_taskbar(&mut self) {
        let s = &mut self.back;
        s.fill_gradient(TASKBAR, rgb(0x2a, 0x2e, 0x38), rgb(0x16, 0x18, 0x1e));
        s.fill_rect(Rect::new(0, TASKBAR.y, SCREEN_W, 1), rgb(0x50, 0x56, 0x64));
        let (top, bottom) = if self.menu_open { TITLE_FOCUSED } else { (rgb(0x3c, 0x6c, 0xb0), rgb(0x24, 0x48, 0x80)) };
        s.fill_gradient(START, top, bottom);
        s.draw_text(START.x + (START.w - text_width("KOS", 1)) / 2, START.y + 4, "KOS", 0x00ffffff, 1, 2);
        for (i, &id) in self.taskbar.iter().enumerate() {
            let Some(w) = self.windows.iter().find(|w| w.id == id) else { continue };
            let r = Rect::new(START.right() + 8 + i as i32 * (TASK_BUTTON_W + 4), START.y, TASK_BUTTON_W, START.h);
            // Leave room for the clock at the right.
            if r.right() > SCREEN_W - 160 { break; }
            let focused = self.focused == Some(w.id);
            s.fill_rect(r, if focused { rgb(0x4a, 0x50, 0x60) } else { rgb(0x32, 0x36, 0x40) });
            s.rect_outline(r, rgb(0x5a, 0x60, 0x70));
            let max_chars = ((TASK_BUTTON_W - 16) / 8) as usize;
            let title: String = w.title.chars().take(max_chars).collect();
            s.draw_text(r.x + 8, r.y + 4, &title, 0x00ffffff, 1, 2);
        }
        // The date and local time from the clock chip, or the uptime if
        // there isn't one.
        let clock = match crate::drivers::rtc::local_now() {
            Some(t) => format!("{} {} {}  {:02}:{:02}", t.weekday_name(), t.month_name(), t.day, t.hour, t.minute),
            None => {
                let up = timer::uptime_ms() / 1000;
                format!("{:02}:{:02}:{:02}", up / 3600, up / 60 % 60, up % 60)
            }
        };
        s.draw_text(SCREEN_W - text_width(&clock, 1) - 12, START.y + 4, &clock, rgb(0xd0, 0xd4, 0xdc), 1, 2);
    }

    fn draw_menu(&mut self) {
        let menu = self.menu_rect();
        let s = &mut self.back;
        s.fill_rect(menu, rgb(0xf4, 0xf5, 0xf7));
        s.rect_outline(menu, rgb(0x30, 0x34, 0x40));
        for (i, (label, _)) in MENU.iter().enumerate() {
            let r = Rect::new(menu.x + 4, menu.y + 4 + i as i32 * MENU_ITEM_H, menu.w - 8, MENU_ITEM_H);
            let hover = self.menu_hover == Some(i);
            if hover { s.fill_rect(r, apps::ACCENT); }
            s.draw_text(r.x + 10, r.y + 7, label, if hover { 0x00ffffff } else { apps::FG }, 1, 2);
        }
    }

    // ---- rendering ---------------------------------------------------------

    /// Lets apps update (clock, terminal output, ...) and damages what changed.
    pub fn tick(&mut self) {
        for i in 0..self.windows.len() {
            if self.windows[i].app.tick() {
                self.redraw_window(i);
            }
        }
        let second = timer::uptime_ms() / 1000;
        if second != self.clock_second {
            self.clock_second = second;
            self.damage(TASKBAR);
        }
    }

    fn draw_window(back: &mut Surface, w: &Window, focused: bool) {
        let frame = w.frame();
        back.rect_outline(frame, if focused { rgb(0x20, 0x40, 0x70) } else { rgb(0x40, 0x44, 0x4c) });
        let bar = w.title_bar();
        let (top, bottom) = if focused { TITLE_FOCUSED } else { TITLE_UNFOCUSED };
        back.fill_gradient(bar, top, bottom);
        let buttons = if w.resizable { 3 } else { 2 };
        let max_chars = ((bar.w - buttons * (CLOSE_SIZE + 4) - 20) / 8).max(0) as usize;
        let title: String = w.title.chars().take(max_chars).collect();
        back.draw_text(bar.x + 8, bar.y + 4, &title, 0x00ffffff, 1, 2);
        let close = w.close_box();
        back.fill_rect(close, if focused { rgb(0xd0, 0x4a, 0x4a) } else { rgb(0x8a, 0x8e, 0x98) });
        back.line(close.x + 4, close.y + 4, close.right() - 5, close.bottom() - 5, 2, 0x00ffffff);
        back.line(close.right() - 5, close.y + 4, close.x + 4, close.bottom() - 5, 2, 0x00ffffff);
        // Maximize button: a box, or two overlapping ones while maximized.
        if w.resizable {
            let max = w.maximize_box();
            back.fill_rect(max, if focused { rgb(0x5c, 0x8e, 0xc8) } else { rgb(0x8a, 0x8e, 0x98) });
            back.rect_outline(max, if focused { rgb(0x20, 0x40, 0x70) } else { rgb(0x5c, 0x61, 0x6b) });
            if w.restore.is_some() {
                back.rect_outline(Rect::new(max.x + 6, max.y + 3, 7, 6), 0x00ffffff);
                back.fill_rect(Rect::new(max.x + 3, max.y + 6, 7, 7), if focused { rgb(0x5c, 0x8e, 0xc8) } else { rgb(0x8a, 0x8e, 0x98) });
                back.rect_outline(Rect::new(max.x + 3, max.y + 6, 7, 7), 0x00ffffff);
                back.fill_rect(Rect::new(max.x + 3, max.y + 6, 7, 2), 0x00ffffff);
            } else {
                back.rect_outline(Rect::new(max.x + 3, max.y + 3, CLOSE_SIZE - 6, CLOSE_SIZE - 6), 0x00ffffff);
                back.fill_rect(Rect::new(max.x + 3, max.y + 3, CLOSE_SIZE - 6, 2), 0x00ffffff);
            }
        }
        // Shade button: a bar while open, a downward chevron while shaded.
        let shade = w.shade_box();
        back.fill_rect(shade, if focused { rgb(0x5c, 0x8e, 0xc8) } else { rgb(0x8a, 0x8e, 0x98) });
        back.rect_outline(shade, if focused { rgb(0x20, 0x40, 0x70) } else { rgb(0x5c, 0x61, 0x6b) });
        if w.shaded {
            let mid = shade.x + CLOSE_SIZE / 2;
            back.line(shade.x + 4, shade.y + 6, mid, shade.bottom() - 6, 2, 0x00ffffff);
            back.line(shade.right() - 5, shade.y + 6, mid, shade.bottom() - 6, 2, 0x00ffffff);
        } else {
            back.fill_rect(Rect::new(shade.x + 4, shade.y + 4, CLOSE_SIZE - 8, 3), 0x00ffffff);
        }
        if !w.shaded {
            let c = w.content();
            back.blit(&w.surface, c.x, c.y);
        }
    }

    /// Redraws the damaged region, puts it on screen, and moves the cursor.
    pub fn render(&mut self) {
        let fb = display::pixels();
        let damage = self.damage;
        // Everything copied to `fb` this frame, for the display to show.
        let mut shown = Rect::EMPTY;
        self.damage = Rect::EMPTY;
        if !damage.is_empty() {
            let started = timer::now();
            crate::stats::inc(&crate::stats::FRAMES_RENDERED);
            self.back.set_clip(damage);
            self.back.blit(&self.background, 0, 0);
            for w in &self.windows {
                if !w.frame().intersect(damage).is_empty() {
                    Self::draw_window(&mut self.back, w, self.focused == Some(w.id));
                }
            }
            if !TASKBAR.intersect(damage).is_empty() {
                self.draw_taskbar();
            }
            if self.menu_open {
                self.draw_menu();
            }
            self.back.reset_clip();
            self.copy_to_screen(fb, damage);
            shown = damage;
            log::trace!("rendered {}x{} at ({}, {}) in {} us", damage.w, damage.h, damage.x, damage.y,
                (timer::now() - started) * 1_000_000 / timer::frequency());
            // The cursor may stick out of the damage (a resize cursor is
            // centered on the window's edge): put back all of what was
            // under it, not just the damaged part.
            if let Some((at, pointer)) = self.cursor_drawn {
                if !pointer.rect(at).intersect(damage).is_empty() {
                    self.copy_to_screen(fb, pointer.rect(at));
                    shown = shown.union(pointer.rect(at));
                    self.cursor_drawn = None;
                }
            }
        }
        if self.cursor_drawn != Some((self.cursor, self.pointer)) {
            if let Some((at, pointer)) = self.cursor_drawn.take() {
                self.copy_to_screen(fb, pointer.rect(at));
                shown = shown.union(pointer.rect(at));
            }
            self.draw_cursor(fb);
            shown = shown.union(self.pointer.rect(self.cursor));
        }
        let r = shown.intersect(SCREEN);
        if !r.is_empty() {
            display::flush(r.x as usize, r.y as usize, r.w as usize, r.h as usize);
        }
    }

    fn copy_to_screen(&self, fb: &mut [u32], rect: Rect) {
        let r = rect.intersect(SCREEN);
        if r.is_empty() { return; }
        for y in r.y..r.bottom() {
            let start = (y * SCREEN_W + r.x) as usize;
            fb[start..start + r.w as usize].copy_from_slice(&self.back.pixels[start..start + r.w as usize]);
        }
    }

    fn draw_cursor(&mut self, fb: &mut [u32]) {
        let r = self.pointer.rect(self.cursor);
        for row in 0..r.h {
            for col in 0..r.w {
                let b = self.pointer.pixel(col as usize, row as usize);
                let (x, y) = (r.x + col, r.y + row);
                if b == b' ' || !SCREEN.contains(x, y) { continue; }
                fb[(y * SCREEN_W + x) as usize] = if b == b'X' { 0 } else { 0x00ffffff };
            }
        }
        self.cursor_drawn = Some((self.cursor, self.pointer));
    }
}
