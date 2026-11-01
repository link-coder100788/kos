//! A rendered page: lighthouse's display list plus everything the Browser
//! draws or handles itself (links, images, form fields), all in page
//! coordinates, so scrolling is just an offset.
//!
//! Rendering keeps only owned data. lighthouse's DOM, styled tree and
//! layout tree borrow from each other, so they're dropped once collected.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::Cell;
use lighthouse::css::{Color, Value};
use lighthouse::dom::{ElementData, Node, NodeType};
use lighthouse::layout::{self, BoxType, Dimensions, LayoutBox, LayoutOptions, TextMetrics};
use lighthouse::paint::{self, DisplayCommand, DisplayList};
use lighthouse::style::{self, StyledNode};
use lighthouse::html;
use crate::net::Url;
use crate::ui::gfx::{rgb, Rect, Surface, GLYPH_H, GLYPH_W};
use super::form::{self, Field, Form};

/// The width pages are laid out at in a Browser `window_w` wide: its
/// content area minus the scrollbar.
pub fn page_width(window_w: i32) -> i32 {
    window_w - super::SCROLLBAR_W - 4
}
/// Every rendering stage recurses once per level of nesting, about 500
/// bytes of stack each, and this runs on the UI task's 64 KiB stack. Real
/// pages nest under 30 deep; anything deeper is flattened by the parser.
const MAX_DEPTH: usize = 48;
/// Images fetched per page, at most.
pub const MAX_IMAGES: usize = 24;
/// Size of an image whose size isn't known until it's loaded.
const PLACEHOLDER: (f32, f32) = (24.0, 24.0);

/// The 8x8 font is drawn twice as tall as wide; big text doubles that.
fn glyph_scale(font_size: f32) -> i32 {
    if font_size >= 24.0 { 2 } else { 1 }
}

/// What lighthouse should measure text with, so lines wrap where our
/// fixed-width font actually ends.
const METRICS: TextMetrics = TextMetrics {
    char_width: |font_size| (GLYPH_W * glyph_scale(font_size)) as f32,
    line_height: |font_size| ((2 * GLYPH_H + 3) * glyph_scale(font_size)) as f32,
};

/// The font only has ASCII. Swap common symbols for a lookalike, one
/// character for one so text stays as wide as layout measured it.
pub fn ascii(c: char) -> char {
    match c {
        c if c.is_ascii() => c,
        '\u{a0}' => ' ',
        '\u{2018}' | '\u{2019}' | '\u{2032}' => '\'',
        '\u{201c}' | '\u{201d}' | '\u{2033}' => '"',
        '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
        '\u{2022}' | '\u{b7}' => '*',
        '\u{2026}' => '.',
        '\u{ab}' | '\u{2039}' => '<',
        '\u{bb}' | '\u{203a}' => '>',
        '\u{d7}' => 'x',
        '\u{a9}' => 'c',
        '\u{ae}' => 'r',
        '\u{b0}' => 'o',
        _ => '?',
    }
}

fn color(c: Color) -> u32 {
    rgb(c.r, c.g, c.b)
}

fn to_rect(r: layout::Rect) -> Rect {
    Rect::new(r.x as i32, r.y as i32, r.width as i32, r.height as i32)
}

pub struct Link {
    pub rect: Rect,
    pub url: String,
}

pub struct ImageBox {
    pub rect: Rect,
    pub url: String,
    pub alt: String,
}

/// A fetched (or failed) image, shared by every box showing that URL.
pub enum ImageState {
    Loading,
    Loaded(Surface),
    Failed,
}

pub struct Page {
    commands: DisplayList,
    pub height: i32,
    pub links: Vec<Link>,
    pub images: Vec<ImageBox>,
    pub forms: Vec<Form>,
    pub fields: Vec<Field>,
    /// Some image was laid out at a guessed size; render again once the
    /// images have loaded and their real sizes are known.
    pub needs_image_sizes: bool,
}

/// The layout size of an `<img>`: CSS width/height, then the attributes,
/// then the image's own size (`known`), keeping its shape when only one
/// side is given. Records in `guessed` if it had to guess.
fn image_size(n: &StyledNode, e: &ElementData, known: Option<(f32, f32)>, guessed: &Cell<bool>, page_w: i32) -> (f32, f32) {
    let attr = |name| {
        e.get_attribute(name)
            .and_then(|v| v.trim().trim_end_matches("px").parse::<f32>().ok())
            .filter(|v| *v > 0.0)
    };
    let css = |name| match n.value(name) {
        Some(Value::Length(v, _)) if v > 0.0 => Some(v),
        _ => None,
    };
    let (w, h) = (css("width").or(attr("width")), css("height").or(attr("height")));
    let (w, h) = match (w, h, known) {
        (Some(w), Some(h), _) => (w, h),
        (Some(w), None, Some((kw, kh))) => (w, w * kh / kw),
        (None, Some(h), Some((kw, kh))) => (h * kw / kh, h),
        (None, None, Some(size)) => size,
        (w, h, None) => {
            guessed.set(true);
            (w.or(h).unwrap_or(PLACEHOLDER.0), h.or(w).unwrap_or(PLACEHOLDER.1))
        }
    };
    // Shrink anything wider than the page, keeping its shape.
    let max = (page_w - 16) as f32;
    if w > max { (max, h * max / w) } else { (w, h) }
}

/// Where an `<img>` loads from, if anywhere we can fetch and decode.
pub fn image_url(e: &ElementData, base: &Url) -> Option<String> {
    let src = e.get_attribute("src")?;
    let url = base.join(src).ok()?.to_string();
    (!url.to_ascii_lowercase().ends_with(".svg")).then_some(url)
}

/// Renders `html` `width` pixels wide, resolving links against `base`.
/// `sizes` holds the real sizes of images already loaded.
pub fn render(html: &str, base: &Url, sizes: &BTreeMap<String, (f32, f32)>, width: i32) -> Page {
    let dom = html::parse_with_max_depth(html, MAX_DEPTH);
    let sheet = style::document_stylesheet(&dom);
    let styled = style::style_tree(&dom, &sheet);

    let mut forms = Vec::new();
    let mut fields = Vec::new();
    form::collect(&dom, base, None, &mut forms, &mut fields);

    let guessed = Cell::new(false);
    let replaced = |n: &StyledNode| {
        let NodeType::Element(e) = &n.node.node_type else { return None };
        match e.tag_name.as_str() {
            "img" => {
                let known = image_url(e, base).and_then(|url| sizes.get(&url).copied());
                Some(image_size(n, e, known, &guessed, width))
            }
            _ => form::replaced_size(e, n.node),
        }
    };
    let mut viewport = Dimensions::default();
    viewport.content.width = width as f32;
    let options = LayoutOptions { metrics: METRICS, replaced: &replaced };
    let boxes = layout::layout_tree_with(&styled, viewport, &options);
    let commands = paint::build_display_list(&boxes);

    let bottom = commands.iter().map(|c| match c {
        DisplayCommand::SolidColor(_, r) | DisplayCommand::Text(_, _, r, _) => r.y + r.height,
    });
    let height = bottom.fold(boxes.dimensions.margin_box().height, f32::max) as i32;
    let mut page = Page {
        commands,
        height,
        links: Vec::new(),
        images: Vec::new(),
        forms,
        fields,
        needs_image_sizes: guessed.get(),
    };
    let field_at: BTreeMap<usize, usize> = page.fields.iter().enumerate().map(|(i, f)| (f.node, i)).collect();
    collect(&boxes, base, None, &field_at, &mut page);
    page
}

/// Walks the laid-out page for link areas, image boxes and field boxes.
/// `link` is the URL of the `<a>` we're inside, if any.
fn collect(b: &LayoutBox, base: &Url, link: Option<&str>, field_at: &BTreeMap<usize, usize>, page: &mut Page) {
    let (BoxType::BlockNode(n) | BoxType::InlineNode(n)) = b.box_type else {
        for child in &b.children {
            collect(child, base, link, field_at, page);
        }
        return;
    };
    let mut link = link;
    let mut link_url = None;
    if let NodeType::Element(e) = &n.node.node_type {
        if e.tag_name == "a" {
            // Same-page anchors (#section) would just reload the page.
            if let Some(href) = e.get_attribute("href").filter(|h| !h.starts_with('#')) {
                link = Some(href.as_str());
            }
        }
        if e.tag_name == "img" {
            let alt = e.get_attribute("alt").cloned().unwrap_or_default();
            let url = image_url(e, base).unwrap_or_default();
            page.images.push(ImageBox { rect: to_rect(b.dimensions.content), url, alt });
        }
        if let Some(&i) = field_at.get(&(n.node as *const Node as usize)) {
            page.fields[i].rect = to_rect(b.dimensions.content);
        }
    }
    if let Some(href) = link {
        link_url = base.join(href).ok().map(|u| u.to_string());
    }
    if let Some(url) = &link_url {
        let is_image = matches!(&n.node.node_type, NodeType::Element(e) if e.tag_name == "img");
        if is_image {
            page.links.push(Link { rect: to_rect(b.dimensions.content), url: url.clone() });
        }
        for f in &b.fragments {
            page.links.push(Link { rect: to_rect(f.rect), url: url.clone() });
        }
    }
    for child in &b.children {
        collect(child, base, link, field_at, page);
    }
}

impl Page {
    /// The link at page point (x, y), if any.
    pub fn link_at(&self, x: i32, y: i32) -> Option<&str> {
        self.links.iter().find(|l| l.rect.contains(x, y)).map(|l| l.url.as_str())
    }

    /// The (shown) field at page point (x, y), if any.
    pub fn field_at(&self, x: i32, y: i32) -> Option<usize> {
        self.fields.iter().position(|f| !f.rect.is_empty() && f.rect.contains(x, y))
    }

    /// Draws the page into `area` of `s`, scrolled down by `scroll` pixels.
    pub fn draw(&self, s: &mut Surface, area: Rect, scroll: i32, images: &BTreeMap<String, ImageState>, focus: Option<usize>, caret: bool) {
        // Page coordinates -> window: the page's top is scrolled out of view.
        let (dx, dy) = (area.x, area.y - scroll);
        let visible = |r: Rect| r.y < area.bottom() && r.bottom() > area.y;
        s.set_clip(area);
        for command in &self.commands {
            match command {
                DisplayCommand::SolidColor(c, r) => {
                    let r = to_rect(*r).offset(dx, dy);
                    if c.a > 0 && visible(r) {
                        s.fill_rect(r, color(*c));
                    }
                }
                DisplayCommand::Text(text, c, r, text_style) => {
                    let r = to_rect(*r).offset(dx, dy);
                    if !visible(r) {
                        continue;
                    }
                    let scale = glyph_scale(text_style.font_size);
                    let glyph_h = 2 * GLYPH_H * scale;
                    let y = r.y + (r.h - glyph_h) / 2;
                    let text: String = text.chars().map(ascii).collect();
                    let w = s.draw_text(r.x, y, &text, color(*c), scale, 2 * scale);
                    if text_style.bold {
                        s.draw_text(r.x + 1, y, &text, color(*c), scale, 2 * scale);
                    }
                    if text_style.underline {
                        s.fill_rect(Rect::new(r.x, y + glyph_h, w, scale), color(*c));
                    }
                }
            }
        }
        for image in &self.images {
            let r = image.rect.offset(dx, dy);
            if !visible(r) || r.is_empty() {
                continue;
            }
            match images.get(&image.url) {
                Some(ImageState::Loaded(surface)) => s.blit_scaled(surface, r),
                // Not loaded (yet): a box, with the alt text if it fits.
                state => {
                    s.fill_rect(r, 0x00f0f0f0);
                    s.rect_outline(r, 0x00c0c0c0);
                    if !matches!(state, Some(ImageState::Loading)) && r.h >= 2 * GLYPH_H + 4 {
                        let old_clip = s.clip;
                        s.set_clip(r.inset(2).intersect(old_clip));
                        let alt: String = image.alt.chars().map(ascii).collect();
                        s.draw_text(r.x + 4, r.y + 3, &alt, 0x00606060, 1, 2);
                        s.clip = old_clip;
                    }
                }
            }
        }
        for (i, f) in self.fields.iter().enumerate() {
            let r = f.rect.offset(dx, dy);
            if !r.is_empty() && visible(r) {
                form::draw(s, f, r, focus == Some(i), caret);
            }
        }
        s.reset_clip();
    }
}
