//! Forms: the fields on a page, how big they are, how they're drawn, and
//! turning a submitted form into the URL to load.
//!
//! Only GET forms are supported: their fields go in the URL's query string
//! (`/search?q=kos&btnG=Google+Search`), which is all a search box needs.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use lighthouse::dom::{ElementData, Node, NodeType};
use crate::net::Url;
use crate::ui::apps::{draw_button, ACCENT, FG};
use crate::ui::gfx::{Rect, Surface, GLYPH_W};

/// Fields are one text line tall, plus a border and some padding.
pub const FIELD_H: i32 = 22;

pub struct Form {
    /// Where to send it: the `action`, resolved against the page.
    pub action: Option<Url>,
    /// `method="get"` (the default); POST isn't supported.
    pub get: bool,
}

pub enum Kind {
    Text { password: bool },
    Hidden,
    Checkbox,
    Radio,
    Submit,
    /// `type="button"` and `type="reset"`: shown, but they do nothing.
    Button,
    /// Options as (value, label).
    Select { options: Vec<(String, String)>, selected: usize },
}

pub struct Field {
    pub kind: Kind,
    pub name: String,
    pub value: String,
    pub checked: bool,
    /// Index into the page's forms, if the field is inside one.
    pub form: Option<usize>,
    /// Where it was laid out, in page coordinates; empty if it isn't shown.
    pub rect: Rect,
    /// Its DOM node's address, to find its box after layout.
    pub(super) node: usize,
}

impl Field {
    pub fn label(&self) -> &str {
        match &self.kind {
            Kind::Select { options, selected } => options.get(*selected).map_or("", |o| o.1.as_str()),
            _ => &self.value,
        }
    }

    pub fn is_text(&self) -> bool {
        matches!(self.kind, Kind::Text { .. })
    }
}

/// The `type` of an `<input>`, lowercased, defaulting to text.
fn input_type(e: &ElementData) -> String {
    e.get_attribute("type").map_or_else(|| String::from("text"), |t| t.trim().to_ascii_lowercase())
}

fn text_content(node: &Node, out: &mut String) {
    match &node.node_type {
        NodeType::Text(t) => out.push_str(t),
        NodeType::Element(_) => node.children.iter().for_each(|c| text_content(c, out)),
    }
}

fn collapsed_text(node: &Node) -> String {
    let mut text = String::new();
    text_content(node, &mut text);
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Layout size for a form field element, or `None` if it isn't one (or
/// isn't shown, like a hidden input).
pub fn replaced_size(e: &ElementData, node: &Node) -> Option<(f32, f32)> {
    let chars = |n: usize| (n as i32 * GLYPH_W + 12) as f32;
    let size = match e.tag_name.as_str() {
        "input" => match input_type(e).as_str() {
            "hidden" => return None,
            "checkbox" | "radio" => (14.0, 14.0),
            "submit" | "button" | "reset" | "image" => (chars(button_label(e, None).chars().count()) + 8.0, FIELD_H as f32),
            _ => {
                let size = e.get_attribute("size").and_then(|s| s.trim().parse::<usize>().ok()).unwrap_or(20);
                (chars(size.clamp(2, 80)), FIELD_H as f32)
            }
        },
        "button" => (chars(collapsed_text(node).chars().count().max(1)) + 8.0, FIELD_H as f32),
        "select" => {
            let longest = options(node).iter().map(|o| o.1.chars().count()).max().unwrap_or(4);
            (chars(longest) + 16.0, FIELD_H as f32)
        }
        "textarea" => {
            let cols = e.get_attribute("cols").and_then(|s| s.trim().parse::<usize>().ok()).unwrap_or(20);
            (chars(cols.clamp(2, 80)), FIELD_H as f32)
        }
        _ => return None,
    };
    Some(size)
}

fn button_label(e: &ElementData, node: Option<&Node>) -> String {
    if let Some(v) = e.get_attribute("value") {
        return v.clone();
    }
    if let Some(node) = node {
        if e.tag_name == "button" {
            return collapsed_text(node);
        }
    }
    String::from(if input_type(e) == "reset" { "Reset" } else { "Submit" })
}

/// A `<select>`'s options as (value, label), and which is selected.
fn options(node: &Node) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for child in &node.children {
        if let NodeType::Element(e) = &child.node_type {
            match e.tag_name.as_str() {
                "option" => {
                    let label = collapsed_text(child);
                    let value = e.get_attribute("value").cloned().unwrap_or_else(|| label.clone());
                    out.push((value, label));
                }
                "optgroup" => out.extend(options(child)),
                _ => {}
            }
        }
    }
    out
}

fn selected_option(node: &Node) -> usize {
    let mut index = 0;
    let mut selected = 0;
    fn walk(node: &Node, index: &mut usize, selected: &mut usize) {
        for child in &node.children {
            if let NodeType::Element(e) = &child.node_type {
                match e.tag_name.as_str() {
                    "option" => {
                        if e.attributes.contains_key("selected") {
                            *selected = *index;
                        }
                        *index += 1;
                    }
                    "optgroup" => walk(child, index, selected),
                    _ => {}
                }
            }
        }
    }
    walk(node, &mut index, &mut selected);
    selected
}

/// Finds every form and field in the document, in order.
pub fn collect(node: &Node, base: &Url, form: Option<usize>, forms: &mut Vec<Form>, fields: &mut Vec<Field>) {
    let NodeType::Element(e) = &node.node_type else { return };
    let mut form = form;
    let field = |kind, value: String| Field {
        kind,
        name: e.get_attribute("name").cloned().unwrap_or_default(),
        value,
        checked: e.attributes.contains_key("checked"),
        form,
        rect: Rect::new(0, 0, 0, 0),
        node: node as *const Node as usize,
    };
    match e.tag_name.as_str() {
        "form" => {
            let action = e.get_attribute("action").map_or("", |a| a.as_str());
            let get = e.get_attribute("method").is_none_or(|m| !m.eq_ignore_ascii_case("post"));
            forms.push(Form { action: base.join(action).ok(), get });
            form = Some(forms.len() - 1);
        }
        "input" => {
            let value = e.get_attribute("value").cloned().unwrap_or_default();
            let kind = match input_type(e).as_str() {
                "hidden" => Kind::Hidden,
                "checkbox" => Kind::Checkbox,
                "radio" => Kind::Radio,
                "submit" | "image" => Kind::Submit,
                "button" | "reset" => Kind::Button,
                "password" => Kind::Text { password: true },
                _ => Kind::Text { password: false },
            };
            let value = match kind {
                Kind::Submit | Kind::Button => button_label(e, None),
                Kind::Checkbox | Kind::Radio if value.is_empty() => String::from("on"),
                _ => value,
            };
            fields.push(field(kind, value));
            return;
        }
        "button" => {
            let submits = e.get_attribute("type").is_none_or(|t| t.eq_ignore_ascii_case("submit"));
            let mut f = field(if submits { Kind::Submit } else { Kind::Button }, collapsed_text(node));
            // The label is the text; what's submitted is `value`.
            if let Some(v) = e.get_attribute("value") {
                f.value = v.clone();
            }
            fields.push(f);
            return;
        }
        "select" => {
            fields.push(field(Kind::Select { options: options(node), selected: selected_option(node) }, String::new()));
            return;
        }
        "textarea" => {
            fields.push(field(Kind::Text { password: false }, collapsed_text(node)));
            return;
        }
        _ => {}
    }
    for child in &node.children {
        collect(child, base, form, forms, fields);
    }
}

/// `application/x-www-form-urlencoded`: letters, digits and `-_.~` as is,
/// spaces as `+`, everything else as `%XX` of its UTF-8 bytes.
fn encode(s: &str, out: &mut String) {
    for b in s.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
}

/// The URL that submitting form `index` loads. `submitter` is the button
/// that was pressed, whose name and value are sent too.
pub fn submit_url(forms: &[Form], fields: &[Field], index: usize, submitter: Option<usize>) -> Result<String, String> {
    let form = &forms[index];
    if !form.get {
        return Err("this form uses POST, which isn't supported yet".to_string());
    }
    let action = form.action.as_ref().ok_or("this form's action isn't an http(s) URL")?;
    let mut query = String::new();
    for (i, f) in fields.iter().enumerate() {
        if f.form != Some(index) || f.name.is_empty() {
            continue;
        }
        let value = match &f.kind {
            Kind::Text { .. } | Kind::Hidden => f.value.as_str(),
            Kind::Checkbox | Kind::Radio if f.checked => f.value.as_str(),
            Kind::Select { options, selected } => options.get(*selected).map_or("", |o| o.0.as_str()),
            Kind::Submit if Some(i) == submitter => f.value.as_str(),
            _ => continue,
        };
        if !query.is_empty() {
            query.push('&');
        }
        encode(&f.name, &mut query);
        query.push('=');
        encode(value, &mut query);
    }
    // The fields replace whatever query the action had.
    let mut url = action.clone();
    if let Some(i) = url.path.find('?') {
        url.path.truncate(i);
    }
    url.path.push('?');
    url.path.push_str(&query);
    Ok(url.to_string())
}

/// Draws a field at `r` (window coordinates); `caret` shows the text
/// cursor in a focused text field.
pub fn draw(s: &mut Surface, f: &Field, r: Rect, focused: bool, caret: bool) {
    let outline = if focused { ACCENT } else { 0x00767676 };
    match &f.kind {
        Kind::Hidden => {}
        Kind::Submit | Kind::Button => draw_button(s, r, f.label(), focused),
        Kind::Checkbox | Kind::Radio => {
            s.fill_rect(r, 0x00ffffff);
            s.rect_outline(r, outline);
            if f.checked {
                s.fill_rect(r.inset(3), FG);
            }
        }
        Kind::Text { .. } | Kind::Select { .. } => {
            s.fill_rect(r, 0x00ffffff);
            s.rect_outline(r, outline);
            let text: String = match f.kind {
                Kind::Text { password: true } => f.value.chars().map(|_| '*').collect(),
                _ => f.label().chars().map(|c| if c.is_ascii() { c } else { '?' }).collect(),
            };
            // Show the end of text too long for the box, as you type.
            let room = ((r.w - 12) / GLYPH_W).max(0) as usize;
            let skip = if f.is_text() { text.chars().count().saturating_sub(room) } else { 0 };
            let shown: String = text.chars().skip(skip).take(room).collect();
            let old_clip = s.clip;
            s.set_clip(r.inset(1).intersect(old_clip));
            let ty = r.y + (r.h - 16) / 2;
            let w = s.draw_text(r.x + 5, ty, &shown, FG, 1, 2);
            if focused && caret && f.is_text() {
                s.fill_rect(Rect::new(r.x + 5 + w + 1, ty, 2, 16), FG);
            }
            if matches!(f.kind, Kind::Select { .. }) {
                s.draw_text(r.right() - 12, ty, "v", FG, 1, 2);
            }
            s.clip = old_clip;
        }
    }
}
