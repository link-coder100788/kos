//! A web browser: fetches pages over HTTP(S), renders HTML with the
//! lighthouse engine, and can show any response as plain source text.
//!
//! Links are followed on click, Back and Forward walk the history, form
//! fields take clicks and typing and submit as GET requests, and images
//! load in the background after the page, a couple at a time.
//!
//! Rendering runs right here in the app (on the UI task) when a page
//! arrives; see `page.rs`. Drawing paints only what's in view, offset by
//! the scroll position, so a long page never needs a page-sized image.

mod form;
mod page;

use alloc::collections::{BTreeMap, VecDeque};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use crate::arch::timer;
use crate::drivers::input::{Key, MouseButton};
use crate::net::{self, FetchResult, Pending, Url};
use crate::ui::apps::{draw_button, App, MouseEvent, ACCENT, BG, FG};
use crate::ui::gfx::{Rect, Surface, GLYPH_W};
use form::Kind;
use page::{ImageState, Page, MAX_IMAGES};

/// Size when it opens.
const START_W: i32 = 780;
const START_H: i32 = 520;
const BAR_H: i32 = 40;
const STATUS_H: i32 = 24;
const LINE_H: i32 = 16;
const PAD: i32 = 8;
const SCROLLBAR_W: i32 = 8;
const BACK: Rect = Rect::new(PAD, 7, 28, 26);
const FORWARD: Rect = Rect::new(BACK.x + BACK.w + 4, 7, 28, 26);
/// After the window last changed width, how long to wait before laying
/// the page out again, so dragging the edge doesn't lay it out every step.
const RELAYOUT_DELAY_MS: u64 = 150;
/// Pixels per wheel click or arrow key press in the rendered view.
const SCROLL_STEP: i32 = 48;
/// Image downloads in flight at once. https requests share one TLS task,
/// so more would just queue, and hold up the next page you click.
const PARALLEL_IMAGES: usize = 2;
/// Bigger images aren't decoded: decoding runs on the UI task.
const MAX_IMAGE_BYTES: usize = 1024 * 1024;

const START_PAGE: &str = r#"<html><body>
<h1>KOS Browser</h1>
<p>Type an address above and press Enter (or click Go). Pages are fetched
over HTTP or HTTPS and rendered by <b>lighthouse</b>, a small HTML and CSS
engine. Click <b>Source</b> (or press Ctrl+U) to see a page's HTML instead.</p>
<h2>Try</h2>
<ul>
<li><a href="https://www.google.com/">https://www.google.com/</a></li>
<li><a href="https://en.wikipedia.org/wiki/Operating_system">https://en.wikipedia.org/wiki/Operating_system</a></li>
<li><a href="https://example.com/">https://example.com/</a></li>
<li>http://10.0.2.2:8000/ for a server on your computer, e.g. run
<code>python3 -m http.server</code> in some folder</li>
</ul>
<h2>Using it</h2>
<p>Click links to follow them, and <b>&lt;</b> / <b>&gt;</b> to go back and
forward. Click a text box to type in it, and press Enter to submit its form.
Ctrl+L goes to the address bar. Scroll with the mouse wheel, the arrow
keys, Page Up / Page Down, Home and End.</p>
<h2>Limits</h2>
<p>PNG, JPEG and BMP images only; no JavaScript, no external stylesheets,
and only simple CSS. Forms can only be sent with GET. HTTPS checks certs.</p>
</body></html>"#;

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Page,
    Source,
}

/// Where typing goes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Address,
    Field(usize),
    /// Nothing: keys only scroll.
    Page,
}

/// How a load should change the history.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Nav {
    /// A new page: drop anything forward of here, then add it.
    New,
    /// Moving within the history (Back/Forward); already positioned.
    History,
}

struct Loading {
    pending: Pending<FetchResult>,
    url: String,
    started: u64,
    nav: Nav,
}

pub struct Browser {
    address: String,
    focus: Focus,
    view: View,
    /// The current page's URL (after redirects), which links resolve against.
    url: Option<Url>,
    /// The current page's HTML, kept to render again once images load.
    html: String,
    page: Option<Page>,
    /// Pixels scrolled in the page view.
    page_scroll: i32,
    /// The response as display lines, for the source view.
    lines: Vec<String>,
    /// Lines scrolled in the source view.
    scroll: usize,
    status: String,
    /// The link under the pointer, shown in the status line.
    hover: Option<String>,
    loading: Option<Loading>,
    history: Vec<String>,
    /// Index of the current page in `history`.
    at: usize,
    images: BTreeMap<String, ImageState>,
    image_queue: VecDeque<String>,
    image_loads: Vec<(String, Pending<FetchResult>)>,
    caret_visible: bool,
    last_blink: u64,
    /// The response as text, for the source view.
    source: String,
    /// Size of the window's content area.
    w: i32,
    h: i32,
    /// When to lay the page out again for a new width.
    relayout_at: Option<u64>,
}

/// Splits text into display lines `cols` wide: tabs expanded, control
/// characters dropped, long lines wrapped.
fn layout(text: &str, cols: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for raw in text.split('\n') {
        let mut line = String::new();
        let mut push = |line: &mut String, c: char| {
            // Wrap only when a character doesn't fit, so a line that exactly
            // fills the width doesn't leave an empty line behind it.
            if line.len() == cols {
                lines.push(core::mem::take(line));
            }
            line.push(c);
        };
        for c in raw.chars() {
            match c {
                '\t' => {
                    for _ in 0..4 - line.len() % 4 {
                        push(&mut line, ' ');
                    }
                }
                c if c.is_control() => {}
                c if (c as u32) < 128 => push(&mut line, c),
                _ => push(&mut line, '?'),
            }
        }
        lines.push(line);
    }
    lines
}

fn is_html(content_type: Option<&str>, body: &str) -> bool {
    match content_type {
        Some(t) => t.to_ascii_lowercase().contains("html"),
        None => body.trim_start().starts_with('<'),
    }
}

/// Decodes a PNG, JPEG or BMP, recognised by its first bytes.
fn decode_image(data: &[u8]) -> Result<Surface, &'static str> {
    if data.len() > MAX_IMAGE_BYTES {
        Err("image too big")
    } else if data.starts_with(b"\x89PNG") {
        // Pages are white unless they say otherwise; close enough.
        crate::ui::img::decode_png_over(data, 0x00ffffff)
    } else if data.starts_with(&[0xff, 0xd8]) {
        crate::ui::img::decode_jpeg(data)
    } else if data.starts_with(b"BM") {
        crate::ui::img::decode_bmp(data)
    } else {
        Err("unsupported image format")
    }
}

impl Browser {
    pub fn new() -> Self {
        Self::with_url(None)
    }

    /// Opens the browser, optionally loading `url` right away.
    pub fn with_url(url: Option<&str>) -> Self {
        let start = Url::parse("about.kos/").unwrap();
        let mut browser = Self {
            address: String::new(),
            focus: Focus::Address,
            view: View::Page,
            url: None,
            html: String::from(START_PAGE),
            page: Some(page::render(START_PAGE, &start, &BTreeMap::new(), page::page_width(START_W))),
            page_scroll: 0,
            lines: Vec::new(),
            scroll: 0,
            status: String::from("Ready"),
            hover: None,
            loading: None,
            history: Vec::new(),
            at: 0,
            images: BTreeMap::new(),
            image_queue: VecDeque::new(),
            image_loads: Vec::new(),
            caret_visible: true,
            last_blink: 0,
            source: String::from(START_PAGE),
            w: START_W,
            h: START_H,
            relayout_at: None,
        };
        browser.lines = layout(&browser.source, browser.cols());
        if let Some(url) = url {
            browser.navigate(url.to_string(), Nav::New);
        }
        browser
    }

    // ---- layout: everything below depends on the window's size ----

    fn go_rect(&self) -> Rect {
        Rect::new(self.w - PAD - 62, 7, 62, 26)
    }

    fn view_rect(&self) -> Rect {
        Rect::new(self.go_rect().x - 8 - 78, 7, 78, 26)
    }

    fn address_rect(&self) -> Rect {
        let x = FORWARD.x + FORWARD.w + 8;
        Rect::new(x, 7, (self.view_rect().x - 8 - x).max(40), 26)
    }

    fn content(&self) -> Rect {
        Rect::new(0, BAR_H, self.w, self.h - BAR_H - STATUS_H)
    }

    /// Columns and rows of the source view.
    fn cols(&self) -> usize {
        ((self.w - 2 * PAD - SCROLLBAR_W) / GLYPH_W).max(1) as usize
    }

    fn rows(&self) -> usize {
        ((self.h - BAR_H - STATUS_H - PAD) / LINE_H).max(1) as usize
    }

    fn page_width(&self) -> i32 {
        page::page_width(self.w)
    }

    fn navigate(&mut self, url: String, nav: Nav) {
        let url = url.trim().to_string();
        if url.is_empty() {
            return;
        }
        self.address = url.clone();
        self.status = format!("Loading {} ...", url);
        self.loading = Some(Loading { pending: net::fetch(&url), url, started: timer::uptime_ms(), nav });
    }

    fn go_back(&mut self) {
        if self.at > 0 && self.loading.is_none() {
            self.at -= 1;
            self.navigate(self.history[self.at].clone(), Nav::History);
        }
    }

    fn go_forward(&mut self) {
        if self.at + 1 < self.history.len() && self.loading.is_none() {
            self.at += 1;
            self.navigate(self.history[self.at].clone(), Nav::History);
        }
    }

    fn toggle_view(&mut self) {
        // Without a rendered page there's only the source to show.
        self.view = match self.view {
            View::Source if self.page.is_some() => View::Page,
            _ => View::Source,
        };
    }

    fn max_scroll(&self) -> usize {
        self.lines.len().saturating_sub(self.rows())
    }

    fn max_page_scroll(&self) -> i32 {
        self.page.as_ref().map_or(0, |p| (p.height - self.content().h).max(0))
    }

    /// Scrolls the current view by `lines` source lines, or as many steps
    /// on the page.
    fn scroll_by(&mut self, lines: isize) -> bool {
        match self.view {
            View::Source => {
                let new = (self.scroll as isize + lines).clamp(0, self.max_scroll() as isize) as usize;
                let changed = new != self.scroll;
                self.scroll = new;
                changed
            }
            View::Page => {
                let pixels = (lines as i32).saturating_mul(SCROLL_STEP);
                let new = self.page_scroll.saturating_add(pixels).clamp(0, self.max_page_scroll());
                let changed = new != self.page_scroll;
                self.page_scroll = new;
                changed
            }
        }
    }

    /// Jumps to the top or the bottom of the current view.
    fn scroll_to(&mut self, end: bool) -> bool {
        let (before, after) = match self.view {
            View::Source => {
                let before = self.scroll;
                self.scroll = if end { self.max_scroll() } else { 0 };
                (before as i32, self.scroll as i32)
            }
            View::Page => {
                let before = self.page_scroll;
                self.page_scroll = if end { self.max_page_scroll() } else { 0 };
                (before, self.page_scroll)
            }
        };
        before != after
    }

    /// Scroll amount for a page of content in the current view.
    fn page_lines(&self) -> isize {
        match self.view {
            View::Source => self.rows() as isize - 1,
            View::Page => ((self.content().h - SCROLL_STEP) / SCROLL_STEP).max(1) as isize,
        }
    }

    fn finish(&mut self, result: FetchResult, loading: Loading) {
        let ms = timer::uptime_ms() - loading.started;
        let url = match &result {
            Ok(resp) if !resp.url.is_empty() => resp.url.clone(),
            _ => loading.url.clone(),
        };
        if loading.nav == Nav::New {
            self.history.truncate(self.at + usize::from(!self.history.is_empty()));
            self.history.push(url.clone());
            self.at = self.history.len() - 1;
        } else {
            self.history[self.at] = url.clone();
        }
        self.address = url.clone();
        self.url = Url::parse(&url).ok();
        self.images.clear();
        self.image_queue.clear();
        self.image_loads.clear();
        self.hover = None;
        self.focus = Focus::Page;
        match result {
            Ok(resp) => {
                let text = String::from_utf8_lossy(&resp.body);
                self.source = text.to_string();
                self.lines = layout(&self.source, self.cols());
                let kind = resp.header("content-type");
                self.status = format!("{} {}  |  {} bytes, {}  |  {} ms", resp.status, resp.reason, resp.body.len(),
                    kind.unwrap_or("unknown type"), ms);
                match (&self.url, is_html(kind, &text)) {
                    (Some(base), true) => {
                        self.html = text.into_owned();
                        let render_started = timer::uptime_ms();
                        let page = page::render(&self.html, base, &BTreeMap::new(), self.page_width());
                        let render_ms = timer::uptime_ms() - render_started;
                        log::debug!("browser: rendered {} in {} ms: {} px tall, {} links, {} images, {} fields",
                            url, render_ms, page.height, page.links.len(), page.images.len(), page.fields.len());
                        self.status.push_str(&format!(", rendered in {} ms", render_ms));
                        for image in &page.images {
                            if !image.url.is_empty() && !self.images.contains_key(&image.url) && self.images.len() < MAX_IMAGES {
                                self.images.insert(image.url.clone(), ImageState::Loading);
                                self.image_queue.push_back(image.url.clone());
                            }
                        }
                        self.page = Some(page);
                        self.view = View::Page;
                    }
                    _ => {
                        self.html.clear();
                        self.page = None;
                        self.view = View::Source;
                    }
                }
            }
            Err(e) => {
                self.source = format!("Couldn't load {}\n\n{}", url, e);
                self.lines = layout(&self.source, self.cols());
                self.html.clear();
                self.page = None;
                self.view = View::Source;
                self.status = format!("Error: {}", e);
            }
        }
        self.scroll = 0;
        self.page_scroll = 0;
    }

    /// Starts and collects image downloads. Returns whether any arrived.
    fn load_images(&mut self) -> bool {
        while self.image_loads.len() < PARALLEL_IMAGES {
            let Some(url) = self.image_queue.pop_front() else { break };
            let pending = net::fetch(&url);
            self.image_loads.push((url, pending));
        }
        let mut arrived = false;
        let mut i = 0;
        while i < self.image_loads.len() {
            let Some(result) = self.image_loads[i].1.poll() else {
                i += 1;
                continue;
            };
            let (url, _) = self.image_loads.remove(i);
            let started = timer::uptime_ms();
            let state = match result.map_err(|_| "fetch failed").and_then(|r| decode_image(&r.body)) {
                Ok(surface) => {
                    log::debug!("browser: decoded {}x{} {} in {} ms", surface.width, surface.height, url,
                        timer::uptime_ms() - started);
                    ImageState::Loaded(surface)
                }
                Err(e) => {
                    log::debug!("browser: image {}: {}", url, e);
                    ImageState::Failed
                }
            };
            self.images.insert(url, state);
            arrived = true;
        }
        // Once everything's in, lay the page out again if it had to guess
        // image sizes, keeping whatever was typed into its fields.
        let done = self.image_queue.is_empty() && self.image_loads.is_empty();
        if arrived && done && self.page.as_ref().is_some_and(|p| p.needs_image_sizes) {
            self.render_again();
        }
        arrived
    }

    fn render_again(&mut self) {
        // The start page has no URL of its own.
        let start = Url::parse("about.kos/").ok();
        let (Some(base), Some(old)) = (self.url.as_ref().or(start.as_ref()), &self.page) else { return };
        let sizes: BTreeMap<String, (f32, f32)> = self.images.iter()
            .filter_map(|(url, state)| match state {
                ImageState::Loaded(s) => Some((url.clone(), (s.width as f32, s.height as f32))),
                _ => None,
            })
            .collect();
        let mut page = page::render(&self.html, base, &sizes, self.page_width());
        page.needs_image_sizes = false;
        if page.fields.len() == old.fields.len() {
            for (new, old) in page.fields.iter_mut().zip(&old.fields) {
                new.value = old.value.clone();
                new.checked = old.checked;
                if let (Kind::Select { selected, .. }, Kind::Select { selected: was, .. }) = (&mut new.kind, &old.kind) {
                    *selected = *was;
                }
            }
        }
        self.page = Some(page);
        self.page_scroll = self.page_scroll.min(self.max_page_scroll());
    }

    /// Submits form `index`, pressed with button `submitter` (if any).
    fn submit(&mut self, index: usize, submitter: Option<usize>) {
        let Some(page) = &self.page else { return };
        match form::submit_url(&page.forms, &page.fields, index, submitter) {
            Ok(url) => self.navigate(url, Nav::New),
            Err(e) => self.status = format!("Can't submit: {}", e),
        }
    }

    /// A click at page coordinates (x, y).
    fn click_page(&mut self, x: i32, y: i32) {
        let Some(page) = &mut self.page else { return };
        if let Some(i) = page.field_at(x, y) {
            self.focus = Focus::Page;
            let f = &mut page.fields[i];
            let (form, name) = (f.form, f.name.clone());
            match &mut f.kind {
                Kind::Text { .. } => self.focus = Focus::Field(i),
                Kind::Checkbox => f.checked = !f.checked,
                Kind::Radio => {
                    // Only one radio button with a name is on.
                    for (j, other) in page.fields.iter_mut().enumerate() {
                        if matches!(other.kind, Kind::Radio) && other.form == form && other.name == name {
                            other.checked = j == i;
                        }
                    }
                }
                Kind::Select { options, selected } => {
                    // No drop-down list: each click picks the next option.
                    *selected = (*selected + 1) % options.len().max(1);
                }
                Kind::Submit => {
                    if let Some(form) = form {
                        self.submit(form, Some(i));
                    }
                }
                Kind::Button | Kind::Hidden => {}
            }
            return;
        }
        self.focus = Focus::Page;
        if let Some(url) = page.link_at(x, y) {
            let url = url.to_string();
            self.navigate(url, Nav::New);
        }
    }

    /// Typing into the focused text field.
    fn type_into_field(&mut self, i: usize, key: Key) -> bool {
        let Some(page) = &mut self.page else { return false };
        let Some(f) = page.fields.get_mut(i) else { return false };
        match key {
            Key::Char(c) => f.value.push(c),
            Key::Backspace => { f.value.pop(); }
            Key::Enter => {
                // Like a browser: Enter submits, as if the form's first
                // submit button were pressed.
                if let Some(form) = f.form {
                    let button = page.fields.iter().position(|b| b.form == Some(form) && matches!(b.kind, Kind::Submit));
                    self.submit(form, button);
                }
            }
            _ => return false,
        }
        true
    }

    fn draw_source(&self, s: &mut Surface) {
        let (rows, content) = (self.rows(), self.content());
        for (row, line) in self.lines.iter().skip(self.scroll).take(rows).enumerate() {
            s.draw_text(PAD, content.y + PAD / 2 + row as i32 * LINE_H, line, FG, 1, 2);
        }
        if self.lines.len() > rows {
            draw_scrollbar(s, content, rows as i32, self.lines.len() as i32, self.scroll as i32, self.max_scroll() as i32);
        }
    }
}

/// A scrollbar down the right of `content` for `visible` units out of
/// `total`, scrolled to `offset`.
fn draw_scrollbar(s: &mut Surface, content: Rect, visible: i32, total: i32, offset: i32, max_offset: i32) {
    let track = Rect::new(content.right() - SCROLLBAR_W - 2, content.y + 2, SCROLLBAR_W, content.h - 4);
    s.fill_rect(track, 0x00e0e0e0);
    let thumb_h = (track.h * visible / total.max(1)).max(16);
    let thumb_y = track.y + (track.h - thumb_h) * offset / max_offset.max(1);
    s.fill_rect(Rect::new(track.x, thumb_y, SCROLLBAR_W, thumb_h), 0x00909090);
}

impl App for Browser {
    fn title(&self) -> String {
        String::from("Browser")
    }

    fn size(&self) -> (i32, i32) {
        (self.w, self.h)
    }

    fn resizable(&self) -> bool {
        true
    }

    fn min_size(&self) -> (i32, i32) {
        (360, BAR_H + STATUS_H + 80)
    }

    fn resize(&mut self, w: i32, h: i32) {
        let width_changed = w != self.w;
        self.w = w;
        self.h = h;
        self.lines = layout(&self.source, self.cols());
        self.scroll = self.scroll.min(self.max_scroll());
        self.page_scroll = self.page_scroll.min(self.max_page_scroll());
        if width_changed && self.page.is_some() {
            self.relayout_at = Some(timer::uptime_ms() + RELAYOUT_DELAY_MS);
        }
    }

    fn draw(&mut self, s: &mut Surface) {
        s.fill(BG);
        let (address, content) = (self.address_rect(), self.content());
        // Toolbar.
        s.fill_rect(Rect::new(0, 0, self.w, BAR_H), 0x00e4e6ea);
        draw_button(s, BACK, "<", false);
        draw_button(s, FORWARD, ">", false);
        s.fill_rect(address, 0x00ffffff);
        s.rect_outline(address, if self.focus == Focus::Address { ACCENT } else { 0x008a8f99 });
        let max_chars = ((address.w - 12) / GLYPH_W) as usize;
        let count = self.address.chars().count();
        let visible: String = self.address.chars().skip(count.saturating_sub(max_chars)).collect();
        let tx = address.x + 6;
        let ty = address.y + 5;
        let text_w = s.draw_text(tx, ty, &visible, FG, 1, 2);
        if self.caret_visible && self.focus == Focus::Address {
            s.fill_rect(Rect::new(tx + text_w + 1, ty, 2, 16), FG);
        }
        // The button names the view it switches to.
        let other = if self.view == View::Page { "Source" } else { "Page" };
        draw_button(s, self.view_rect(), other, false);
        draw_button(s, self.go_rect(), if self.loading.is_some() { "..." } else { "Go" }, false);

        s.fill_rect(content, 0x00ffffff);
        match (self.view, &self.page) {
            (View::Page, Some(page)) => {
                let focus = match self.focus {
                    Focus::Field(i) => Some(i),
                    _ => None,
                };
                page.draw(s, content, self.page_scroll, &self.images, focus, self.caret_visible);
                if page.height > content.h {
                    draw_scrollbar(s, content, content.h, page.height, self.page_scroll, self.max_page_scroll());
                }
            }
            _ => self.draw_source(s),
        }

        // Status line: the link under the pointer, or how the load went.
        let status = Rect::new(0, self.h - STATUS_H, self.w, STATUS_H);
        s.fill_rect(status, 0x00e4e6ea);
        s.fill_rect(Rect::new(0, status.y, self.w, 1), 0x00c0c4cc);
        let max_chars = ((self.w - 2 * PAD) / GLYPH_W) as usize;
        let line = self.hover.as_deref().unwrap_or(&self.status);
        let text: String = line.chars().map(page::ascii).take(max_chars).collect();
        s.draw_text(PAD, status.y + 4, &text, 0x00404040, 1, 2);
    }

    fn on_key(&mut self, key: Key, ctrl: bool) -> bool {
        match key {
            Key::Char('u') if ctrl => self.toggle_view(),
            Key::Char('l') if ctrl => self.focus = Focus::Address,
            _ if ctrl => return false,
            Key::Char(_) | Key::Backspace | Key::Enter => match self.focus {
                Focus::Address => match key {
                    Key::Char(c) => self.address.push(c),
                    Key::Backspace => { self.address.pop(); }
                    _ => self.navigate(self.address.clone(), Nav::New),
                },
                Focus::Field(i) => return self.type_into_field(i, key),
                Focus::Page => return false,
            },
            Key::Up => return self.scroll_by(-1),
            Key::Down => return self.scroll_by(1),
            Key::PageUp => return self.scroll_by(-self.page_lines()),
            Key::PageDown => return self.scroll_by(self.page_lines()),
            Key::Home => return self.scroll_to(false),
            Key::End => return self.scroll_to(true),
            _ => return false,
        }
        true
    }

    fn on_mouse(&mut self, event: MouseEvent) -> bool {
        match event {
            MouseEvent::Down { x, y, button: MouseButton::Left } => {
                let content = self.content();
                if BACK.contains(x, y) {
                    self.go_back();
                } else if FORWARD.contains(x, y) {
                    self.go_forward();
                } else if self.go_rect().contains(x, y) {
                    self.navigate(self.address.clone(), Nav::New);
                } else if self.view_rect().contains(x, y) {
                    self.toggle_view();
                } else if self.address_rect().contains(x, y) {
                    self.focus = Focus::Address;
                } else if content.contains(x, y) && self.view == View::Page {
                    self.click_page(x - content.x, y - content.y + self.page_scroll);
                } else {
                    return false;
                }
                true
            }
            MouseEvent::Move { x, y, .. } => {
                let hover = match (&self.page, self.view) {
                    (Some(page), View::Page) if self.content().contains(x, y) => {
                        let content = self.content();
                        page.link_at(x - content.x, y - content.y + self.page_scroll).map(String::from)
                    }
                    _ => None,
                };
                let changed = hover != self.hover;
                self.hover = hover;
                changed
            }
            MouseEvent::Scroll(clicks) => {
                let per_click = if self.view == View::Page { 1 } else { 3 };
                self.scroll_by(-(clicks as isize) * per_click)
            }
            _ => false,
        }
    }

    fn tick(&mut self) -> bool {
        let mut changed = false;
        if self.relayout_at.is_some_and(|at| timer::uptime_ms() >= at) {
            self.relayout_at = None;
            self.render_again();
            self.page_scroll = self.page_scroll.min(self.max_page_scroll());
            changed = true;
        }
        if let Some(loading) = &self.loading {
            if let Some(result) = loading.pending.poll() {
                let loading = self.loading.take().unwrap();
                self.finish(result, loading);
                changed = true;
            }
        }
        changed |= self.load_images();
        let now = timer::uptime_ms();
        if now - self.last_blink >= 500 {
            self.last_blink = now;
            self.caret_visible = !self.caret_visible;
            changed = true;
        }
        changed
    }
}
