//! The layout engine: turn a styled tree into a tree of boxes with absolute
//! rectangles.
//!
//! Lighthouse implements the CSS box model for block boxes plus a simple inline
//! text flow. The algorithm follows the classic two pass block layout:
//!
//! - Widths are resolved top down. A block box fills its containing block width,
//!   resolving `auto` widths and `auto` margins, honoring padding and borders.
//! - Positions and heights are resolved as children are laid out. Block siblings
//!   stack vertically, each starting below the previous sibling's margin box.
//!   A block with `height: auto` grows to contain its children.
//!
//! Inline content (text and inline elements) inside a block is laid out into
//! lines, word by word: a word that doesn't fit on the current line starts
//! the next one, `<br>` forces a break, and `white-space: pre` keeps the
//! source's own line breaks. Each text box records where its words landed as
//! `fragments`, one per line it touches. Text is measured with fixed-width
//! `TextMetrics`, by default half the font size per character.
//!
//! When a block contains a mix of block and inline children, the inline runs are
//! wrapped in anonymous block boxes so the block formatting context stays clean.

use crate::prelude::*;
use crate::css::{Unit, Value};
use crate::style::{Display, StyledNode};
use core::fmt::Write as _;

/// A rectangle in absolute device coordinates. The origin is the top left.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub width: f32,
    /// Height.
    pub height: f32,
}

impl Rect {
    /// Expand this rectangle by the given edge sizes on all four sides.
    pub fn expanded_by(self, edge: EdgeSizes) -> Rect {
        Rect {
            x: self.x - edge.left,
            y: self.y - edge.top,
            width: self.width + edge.left + edge.right,
            height: self.height + edge.top + edge.bottom,
        }
    }
}

/// The four edge sizes around a box, used for margin, border and padding.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct EdgeSizes {
    /// Left edge size.
    pub left: f32,
    /// Right edge size.
    pub right: f32,
    /// Top edge size.
    pub top: f32,
    /// Bottom edge size.
    pub bottom: f32,
}

/// The full box model dimensions of a laid out box.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Dimensions {
    /// The content box, in absolute coordinates.
    pub content: Rect,
    /// Padding around the content.
    pub padding: EdgeSizes,
    /// Border around the padding.
    pub border: EdgeSizes,
    /// Margin around the border.
    pub margin: EdgeSizes,
}

impl Dimensions {
    /// The content plus padding.
    pub fn padding_box(self) -> Rect {
        self.content.expanded_by(self.padding)
    }
    /// The padding box plus border.
    pub fn border_box(self) -> Rect {
        self.padding_box().expanded_by(self.border)
    }
    /// The border box plus margin.
    pub fn margin_box(self) -> Rect {
        self.border_box().expanded_by(self.margin)
    }
}

/// The kind of a layout box.
#[derive(Debug, Clone)]
pub enum BoxType<'a> {
    /// A block level box for a styled node.
    BlockNode(&'a StyledNode<'a>),
    /// An inline level box for a styled node.
    InlineNode(&'a StyledNode<'a>),
    /// An anonymous block box that groups inline content.
    AnonymousBlock,
}

/// A node in the layout tree.
#[derive(Debug, Clone)]
pub struct LayoutBox<'a> {
    /// The absolute geometry of this box. For text and inline elements it's
    /// the bounding box of everything they placed.
    pub dimensions: Dimensions,
    /// What kind of box this is.
    pub box_type: BoxType<'a>,
    /// Child boxes.
    pub children: Vec<LayoutBox<'a>>,
    /// For a text box: its words as laid out, one fragment per line.
    pub fragments: Vec<TextFragment>,
}

/// A run of words from one text node that landed on one line.
#[derive(Debug, Clone, PartialEq)]
pub struct TextFragment {
    /// The words, separated by single spaces.
    pub text: String,
    /// Where they are, in absolute coordinates.
    pub rect: Rect,
}

/// How big text is, for a given font size in px. Lighthouse has no fonts, so
/// all characters are the same width. A renderer with a real (fixed-width)
/// font passes its own metrics so layout matches what it draws.
#[derive(Debug, Clone, Copy)]
pub struct TextMetrics {
    /// The advance of one character.
    pub char_width: fn(f32) -> f32,
    /// The height of one line.
    pub line_height: fn(f32) -> f32,
}

impl Default for TextMetrics {
    fn default() -> Self {
        TextMetrics {
            char_width: |font_size| font_size * CHAR_WIDTH_RATIO,
            line_height: |font_size| font_size * LINE_HEIGHT_RATIO,
        }
    }
}

/// Gives the size of a *replaced element*: one whose content the engine
/// doesn't lay out itself, like an image or a form field. `None` means the
/// element isn't replaced and is laid out normally.
pub type ReplacedSize<'f> = &'f dyn Fn(&StyledNode) -> Option<(f32, f32)>;

/// Everything layout needs from the renderer.
#[derive(Clone, Copy)]
pub struct LayoutOptions<'f> {
    /// How big text is.
    pub metrics: TextMetrics,
    /// Sizes of replaced elements. A replaced element becomes one box of
    /// that size (on a line, or as a block), and its children are dropped;
    /// drawing it is up to the renderer.
    pub replaced: ReplacedSize<'f>,
}

fn nothing_replaced(_: &StyledNode) -> Option<(f32, f32)> {
    None
}

impl Default for LayoutOptions<'_> {
    fn default() -> Self {
        LayoutOptions { metrics: TextMetrics::default(), replaced: &nothing_replaced }
    }
}

impl<'a> LayoutBox<'a> {
    fn new(box_type: BoxType<'a>) -> Self {
        LayoutBox {
            dimensions: Dimensions::default(),
            box_type,
            children: Vec::new(),
            fragments: Vec::new(),
        }
    }

    fn styled_node(&self) -> Option<&'a StyledNode<'a>> {
        match self.box_type {
            BoxType::BlockNode(n) | BoxType::InlineNode(n) => Some(n),
            BoxType::AnonymousBlock => None,
        }
    }

    /// Where should the next inline child go: an existing anonymous block, or a
    /// new one.
    fn get_inline_container(&mut self) -> &mut LayoutBox<'a> {
        match self.box_type {
            BoxType::InlineNode(_) | BoxType::AnonymousBlock => self,
            BoxType::BlockNode(_) => {
                let needs_new = !matches!(
                    self.children.last().map(|c| &c.box_type),
                    Some(BoxType::AnonymousBlock)
                );
                if needs_new {
                    self.children.push(LayoutBox::new(BoxType::AnonymousBlock));
                }
                self.children.last_mut().unwrap()
            }
        }
    }
}

/// Lay out a styled tree inside a viewport and return the layout tree.
///
/// The viewport width bounds the initial containing block. Its height starts at
/// zero and grows as content is laid out.
pub fn layout_tree<'a>(node: &'a StyledNode<'a>, viewport: Dimensions) -> LayoutBox<'a> {
    layout_tree_with(node, viewport, &LayoutOptions::default())
}

/// [`layout_tree`] with the renderer's text metrics and replaced elements.
pub fn layout_tree_with<'a>(node: &'a StyledNode<'a>, mut viewport: Dimensions, options: &LayoutOptions) -> LayoutBox<'a> {
    viewport.content.height = 0.0;
    let mut root = build_layout_tree(node);
    root.layout(viewport, options);
    root
}

/// Build the box tree from the styled tree, choosing box types from `display`
/// and skipping `display: none` subtrees.
pub fn build_layout_tree<'a>(styled: &'a StyledNode<'a>) -> LayoutBox<'a> {
    let mut root = LayoutBox::new(match styled.display() {
        Display::Inline => BoxType::InlineNode(styled),
        Display::Block | Display::None => BoxType::BlockNode(styled),
    });

    // In a block formatting context (a node with at least one block child),
    // whitespace only text between blocks is not rendered, matching browsers.
    let block_context = styled
        .children
        .iter()
        .any(|c| c.display() == Display::Block);

    for child in &styled.children {
        match child.display() {
            Display::Block => root.children.push(build_layout_tree(child)),
            Display::Inline => {
                if block_context && is_whitespace_text(child) {
                    continue;
                }
                root.get_inline_container()
                    .children
                    .push(build_layout_tree(child));
            }
            Display::None => {}
        }
    }
    root
}

fn is_whitespace_text(node: &StyledNode) -> bool {
    matches!(&node.node.node_type, crate::dom::NodeType::Text(t) if t.trim().is_empty())
}

const DEFAULT_FONT_SIZE: f32 = 16.0;
const CHAR_WIDTH_RATIO: f32 = 0.5;
const LINE_HEIGHT_RATIO: f32 = 1.2;

impl LayoutBox<'_> {
    fn layout(&mut self, containing_block: Dimensions, options: &LayoutOptions) {
        // Every box type currently uses the block layout path. Inline flow is
        // handled inside `layout_children` for boxes with inline content.
        self.layout_block(containing_block, options);
    }

    fn layout_block(&mut self, containing_block: Dimensions, options: &LayoutOptions) {
        self.calculate_block_width(containing_block);
        self.calculate_block_position(containing_block);
        if let Some(size) = self.styled_node().and_then(|s| (options.replaced)(s)) {
            self.layout_replaced_block(size);
        } else {
            self.layout_children(options);
        }
        self.calculate_block_height();
    }

    /// A replaced element laid out as a block (`img { display: block }`):
    /// its own size, scaled down to fit if the block is narrower.
    fn layout_replaced_block(&mut self, (width, height): (f32, f32)) {
        self.children.clear();
        let available = self.dimensions.content.width;
        let scale = if width > available && width > 0.0 { available / width } else { 1.0 };
        self.dimensions.content.width = width * scale;
        self.dimensions.content.height = height * scale;
    }

    fn calculate_block_width(&mut self, containing_block: Dimensions) {
        let style = self.styled_node();
        let auto = Value::Keyword("auto".to_string());
        let zero = Value::Length(0.0, Unit::Px);

        let get = |name: &str, fallback: &str, default: &Value| match style {
            Some(s) => s.lookup(name, fallback, default),
            None => default.clone(),
        };

        let mut width = get("width", "width", &auto);

        let margin_left = get("margin-left", "margin", &zero);
        let margin_right = get("margin-right", "margin", &zero);
        let border_left = get("border-left-width", "border-width", &zero);
        let border_right = get("border-right-width", "border-width", &zero);
        let padding_left = get("padding-left", "padding", &zero);
        let padding_right = get("padding-right", "padding", &zero);

        let total: f32 = [
            &margin_left,
            &margin_right,
            &border_left,
            &border_right,
            &padding_left,
            &padding_right,
            &width,
        ]
        .iter()
        .map(|v| v.to_px())
        .sum();

        let mut margin_left = margin_left;

        let container_width = containing_block.content.width;
        if width != auto && margin_left == auto && total > container_width {
            margin_left = zero.clone();
        }

        let underflow = container_width - total;
        let width_is_auto = width == auto;
        let ml_is_auto = margin_left == auto;
        let mr_is_auto = margin_right == auto;

        // Resolve `auto` width and `auto` left margin. The right margin only ever
        // absorbs slack, which the containment clip below computes directly, so it
        // is never assigned here.
        match (width_is_auto, ml_is_auto, mr_is_auto) {
            (false, true, false) => margin_left = Value::Length(underflow, Unit::Px),
            (false, true, true) => margin_left = Value::Length(underflow / 2.0, Unit::Px),
            (true, _, _) => {
                if ml_is_auto {
                    margin_left = zero.clone();
                }
                width = if underflow >= 0.0 {
                    Value::Length(underflow, Unit::Px)
                } else {
                    zero.clone()
                };
            }
            _ => {}
        }

        let mut content_w = width.to_px();

        // `box-sizing: border-box`: the specified width covers content, padding
        // and border, so subtract them to recover the content width.
        if border_box_sizing(style) {
            content_w = (content_w
                - padding_left.to_px()
                - padding_right.to_px()
                - border_left.to_px()
                - border_right.to_px())
            .max(0.0);
        }

        // Containment clip, the documented overflow behavior. An in-flow block
        // box is clipped so its border box can never escape its containing block.
        // The horizontal extent (left margin, borders, padding, content) is
        // distributed within the container width in that fixed priority order,
        // and the right margin absorbs any slack. Every value stays non-negative,
        // so containment holds even when the specified box is far too wide.
        let mut remaining = container_width.max(0.0);
        let take = |remaining: &mut f32, want: f32| {
            let got = want.max(0.0).min(*remaining);
            *remaining -= got;
            got
        };
        let ml = take(&mut remaining, margin_left.to_px());
        let border_l = take(&mut remaining, border_left.to_px());
        let border_r = take(&mut remaining, border_right.to_px());
        let pad_l = take(&mut remaining, padding_left.to_px());
        let pad_r = take(&mut remaining, padding_right.to_px());
        content_w = take(&mut remaining, content_w);
        let mr = remaining.max(0.0);

        let d = &mut self.dimensions;
        d.content.width = content_w;
        d.padding.left = pad_l;
        d.padding.right = pad_r;
        d.border.left = border_l;
        d.border.right = border_r;
        d.margin.left = ml;
        d.margin.right = mr;
    }

    fn calculate_block_position(&mut self, containing_block: Dimensions) {
        let style = self.styled_node();
        let zero = Value::Length(0.0, Unit::Px);
        let get = |name: &str, fallback: &str| match style {
            Some(s) => s.lookup(name, fallback, &zero),
            None => zero.clone(),
        };

        let d = &mut self.dimensions;
        d.margin.top = get("margin-top", "margin").to_px();
        d.margin.bottom = get("margin-bottom", "margin").to_px();
        d.border.top = get("border-top-width", "border-width").to_px();
        d.border.bottom = get("border-bottom-width", "border-width").to_px();
        d.padding.top = get("padding-top", "padding").to_px();
        d.padding.bottom = get("padding-bottom", "padding").to_px();

        d.content.x = containing_block.content.x + d.margin.left + d.border.left + d.padding.left;
        d.content.y = containing_block.content.height
            + containing_block.content.y
            + d.margin.top
            + d.border.top
            + d.padding.top;
    }

    fn layout_children(&mut self, options: &LayoutOptions) {
        // Inline children (text and inline boxes) get line based flow. Any box
        // whose children are inline nodes is treated as an inline formatting
        // context. Anonymous blocks always hold inline content.
        if self.has_inline_children() {
            self.layout_inline_children(options);
        } else {
            let d = &mut self.dimensions;
            for child in &mut self.children {
                child.layout(*d, options);
                d.content.height += child.dimensions.margin_box().height;
            }
        }
    }

    fn has_inline_children(&self) -> bool {
        matches!(self.box_type, BoxType::AnonymousBlock)
            || (!self.children.is_empty()
                && self
                    .children
                    .iter()
                    .all(|c| matches!(c.box_type, BoxType::InlineNode(_))))
            || self.is_text_leaf()
    }

    fn is_text_leaf(&self) -> bool {
        self.children.is_empty()
            && matches!(self.box_type, BoxType::InlineNode(n) if is_text(n))
    }

    fn font_size(&self) -> f32 {
        match self.styled_node().and_then(|s| s.value("font-size")) {
            Some(v) => {
                let px = v.to_px();
                if px > 0.0 {
                    px
                } else {
                    DEFAULT_FONT_SIZE
                }
            }
            None => DEFAULT_FONT_SIZE,
        }
    }

    fn layout_inline_children(&mut self, options: &LayoutOptions) {
        let content_width = self.dimensions.content.width;

        // A leaf inline box that directly carries text has no inline children,
        // so give it a single line sized to its text.
        if self.is_text_leaf() {
            let fs = self.font_size();
            let text_w = inline_text_width(self.styled_node().unwrap(), fs, options).min(content_width.max(fs));
            self.dimensions.content.width = if content_width > 0.0 {
                content_width
            } else {
                text_w
            };
            self.dimensions.content.height = (options.metrics.line_height)(fs);
            return;
        }

        let mut flow = Flow {
            origin_x: self.dimensions.content.x,
            origin_y: self.dimensions.content.y,
            width: content_width,
            options,
            x: 0.0,
            line_top: 0.0,
            line_height: 0.0,
            widest: 0.0,
            pending_space: false,
        };
        let mut boxes = core::mem::take(&mut self.children);
        for child in &mut boxes {
            flow.place(child);
        }
        flow.end_line();
        self.children = boxes;
        self.dimensions.content.height = flow.line_top;
        if content_width <= 0.0 {
            self.dimensions.content.width = flow.widest;
        }
    }

    fn calculate_block_height(&mut self) {
        // Respect an explicit height, otherwise keep the height accumulated
        // while laying out children.
        if let Some(style) = self.styled_node() {
            if let Some(Value::Length(h, Unit::Px)) = style.value("height") {
                let mut h = h;
                if border_box_sizing(Some(style)) {
                    let d = &self.dimensions;
                    h -= d.padding.top + d.padding.bottom + d.border.top + d.border.bottom;
                }
                self.dimensions.content.height = h.max(0.0);
            }
        }
    }
}

/// Whether the box uses `box-sizing: border-box`, where the specified width and
/// height include padding and border. The default is `content-box`.
fn border_box_sizing(style: Option<&StyledNode>) -> bool {
    matches!(
        style.and_then(|s| s.value("box-sizing")),
        Some(Value::Keyword(k)) if k == "border-box"
    )
}

fn is_text(node: &StyledNode) -> bool {
    matches!(node.node.node_type, crate::dom::NodeType::Text(_))
}

fn inline_text_width(node: &StyledNode, font_size: f32, options: &LayoutOptions) -> f32 {
    match &node.node.node_type {
        crate::dom::NodeType::Text(data) => data.trim().chars().count() as f32 * (options.metrics.char_width)(font_size),
        crate::dom::NodeType::Element(_) => node
            .children
            .iter()
            .map(|c| inline_text_width(c, font_size, options))
            .sum(),
    }
}

/// Places inline content line by line inside one block's content box.
/// Positions `x` and `line_top` are relative to the content box's corner.
struct Flow<'m> {
    origin_x: f32,
    origin_y: f32,
    width: f32,
    options: &'m LayoutOptions<'m>,
    /// Where the next word goes on the current line.
    x: f32,
    line_top: f32,
    /// The tallest thing on the current line so far.
    line_height: f32,
    widest: f32,
    /// Whitespace was seen since the last word, so the next word on this
    /// line gets a space before it. This is what joins `a <b>b</b>`.
    pending_space: bool,
}

impl Flow<'_> {
    fn new_line(&mut self) {
        self.widest = self.widest.max(self.x);
        self.line_top += self.line_height;
        self.x = 0.0;
        self.line_height = 0.0;
        self.pending_space = false;
    }

    /// Finish the current line if anything is on it.
    fn end_line(&mut self) {
        if self.x > 0.0 || self.line_height > 0.0 {
            self.new_line();
        }
    }

    fn place(&mut self, b: &mut LayoutBox) {
        let node = match b.box_type {
            BoxType::InlineNode(node) => node,
            // A block inside inline content (say a <div> in an <a>) gets its
            // own lines, full width, between the lines around it.
            BoxType::BlockNode(_) | BoxType::AnonymousBlock => {
                self.end_line();
                let containing = Dimensions {
                    content: Rect { x: self.origin_x, y: self.origin_y, width: self.width, height: self.line_top },
                    ..Default::default()
                };
                b.layout(containing, self.options);
                self.line_top += b.dimensions.margin_box().height;
                return;
            }
        };
        let font_size = b.font_size();
        let line_height = (self.options.metrics.line_height)(font_size);
        match &node.node.node_type {
            crate::dom::NodeType::Text(text) => {
                let pre = matches!(node.value("white-space"), Some(Value::Keyword(k)) if k.starts_with("pre"));
                self.place_text(b, text, font_size, pre);
            }
            crate::dom::NodeType::Element(e) => {
                let start = (self.x, self.line_top);
                if e.tag_name == "br" {
                    self.line_height = self.line_height.max(line_height);
                    self.new_line();
                }
                // Horizontal margin, border and padding push the content
                // (and whatever follows) along; vertical ones are ignored,
                // as for inline boxes in CSS.
                let zero = Value::Length(0.0, Unit::Px);
                let edge = |side: &str| {
                    ["margin", "border", "padding"]
                        .iter()
                        .map(|kind| {
                            let (name, fallback) = if *kind == "border" {
                                (format!("border-{side}-width"), "border-width")
                            } else {
                                (format!("{kind}-{side}"), *kind)
                            };
                            node.lookup(&name, fallback, &zero).to_px().max(0.0)
                        })
                        .sum::<f32>()
                };
                if let Some((width, height)) = (self.options.replaced)(node) {
                    // One unbreakable box, like a very wide word.
                    b.children.clear();
                    let (left, right) = (edge("left"), edge("right"));
                    let mut space = if self.pending_space && self.x > 0.0 { (self.options.metrics.char_width)(font_size) } else { 0.0 };
                    if self.x > 0.0 && self.x + space + left + width + right > self.width {
                        self.new_line();
                        space = 0.0;
                    }
                    let x = self.x + space + left;
                    b.dimensions.content = Rect { x: self.origin_x + x, y: self.origin_y + self.line_top, width, height };
                    self.x = x + width + right;
                    self.line_height = self.line_height.max(height);
                    self.pending_space = false;
                    return;
                }
                self.x += edge("left");
                let mut children = core::mem::take(&mut b.children);
                for child in &mut children {
                    self.place(child);
                }
                b.children = children;
                self.x += edge("right");
                // Cover everything placed since `start`: part of one line,
                // or full-width lines if the element wrapped.
                let rect = if self.line_top == start.1 {
                    Rect { x: start.0, y: start.1, width: self.x - start.0, height: self.line_height.max(line_height) }
                } else {
                    Rect { x: 0.0, y: start.1, width: self.width, height: self.line_top + self.line_height - start.1 }
                };
                b.dimensions.content = Rect { x: self.origin_x + rect.x, y: self.origin_y + rect.y, ..rect };
            }
        }
    }

    /// Lay out a text node's words, recording one fragment per line.
    fn place_text(&mut self, b: &mut LayoutBox, text: &str, font_size: f32, pre: bool) {
        let char_width = (self.options.metrics.char_width)(font_size);
        let line_height = (self.options.metrics.line_height)(font_size);
        let mut fragments = Vec::new();
        // The fragment being built on the current line: text and its x.
        let mut current: Option<(String, f32)> = None;
        let flush = |current: &mut Option<(String, f32)>, fragments: &mut Vec<TextFragment>, flow: &Self| {
            if let Some((text, x)) = current.take() {
                let width = text.chars().count() as f32 * char_width;
                fragments.push(TextFragment {
                    text,
                    rect: Rect { x: flow.origin_x + x, y: flow.origin_y + flow.line_top, width, height: line_height },
                });
            }
        };

        if pre {
            // Keep spaces, and break exactly where the source does.
            for (i, line) in text.split('\n').enumerate() {
                if i > 0 {
                    flush(&mut current, &mut fragments, self);
                    self.line_height = self.line_height.max(line_height);
                    self.new_line();
                }
                if !line.is_empty() {
                    let line = line.replace('\t', "    ");
                    current.get_or_insert_with(|| (String::new(), self.x)).0.push_str(&line);
                    self.x += line.chars().count() as f32 * char_width;
                    self.line_height = self.line_height.max(line_height);
                }
            }
        } else {
            // Collapse whitespace and break between words. (A no-break space
            // isn't ASCII whitespace, so it keeps its words together.)
            if text.starts_with(|c: char| c.is_ascii_whitespace()) {
                self.pending_space = true;
            }
            for word in text.split_ascii_whitespace() {
                let word_width = word.chars().count() as f32 * char_width;
                let mut space = if self.pending_space && self.x > 0.0 { char_width } else { 0.0 };
                if self.x > 0.0 && self.x + space + word_width > self.width {
                    flush(&mut current, &mut fragments, self);
                    self.new_line();
                    space = 0.0;
                }
                match &mut current {
                    Some((text, _)) => {
                        if space > 0.0 {
                            text.push(' ');
                        }
                        text.push_str(word);
                    }
                    None => current = Some((String::from(word), self.x + space)),
                }
                self.x += space + word_width;
                self.line_height = self.line_height.max(line_height);
                self.pending_space = true;
            }
            // After a word, a space is owed only if the text ended in one.
            // Whitespace-only text just owes one (set at the top).
            if text.split_ascii_whitespace().next().is_some() {
                self.pending_space = text.ends_with(|c: char| c.is_ascii_whitespace());
            }
        }
        flush(&mut current, &mut fragments, self);

        b.dimensions.content = match (fragments.first(), fragments.last()) {
            (Some(first), Some(last)) => {
                let left = fragments.iter().map(|f| f.rect.x).fold(f32::MAX, f32::min);
                let right = fragments.iter().map(|f| f.rect.x + f.rect.width).fold(0.0, f32::max);
                Rect { x: left, y: first.rect.y, width: right - left, height: last.rect.y + last.rect.height - first.rect.y }
            }
            _ => Rect { x: self.origin_x + self.x, y: self.origin_y + self.line_top, width: 0.0, height: 0.0 },
        };
        b.fragments = fragments;
    }
}

/// Pretty print a layout tree, one box per line with its content rectangle.
pub fn pretty(root: &LayoutBox) -> String {
    let mut out = String::new();
    pretty_into(root, &mut out, 0);
    out
}

fn pretty_into(b: &LayoutBox, out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
    let label = match &b.box_type {
        BoxType::BlockNode(n) => format!("block {}", node_label(n)),
        BoxType::InlineNode(n) => format!("inline {}", node_label(n)),
        BoxType::AnonymousBlock => "anon-block".to_string(),
    };
    let c = b.dimensions.content;
    let _ = writeln!(
        out,
        "{label}  content=(x:{:.1} y:{:.1} w:{:.1} h:{:.1})",
        c.x, c.y, c.width, c.height
    );
    if b.fragments.len() > 1 {
        for f in &b.fragments {
            for _ in 0..=depth {
                out.push_str("  ");
            }
            let r = f.rect;
            let _ = writeln!(out, "line \"{}\"  (x:{:.1} y:{:.1} w:{:.1})", f.text, r.x, r.y, r.width);
        }
    }
    for child in &b.children {
        pretty_into(child, out, depth + 1);
    }
}

fn node_label(n: &StyledNode) -> String {
    match &n.node.node_type {
        crate::dom::NodeType::Element(e) => format!("<{}>", e.tag_name),
        crate::dom::NodeType::Text(t) => format!("#text \"{}\"", t.trim()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css;
    use crate::html;
    use crate::style::style_tree;

    fn viewport(width: f32) -> Dimensions {
        Dimensions {
            content: Rect {
                x: 0.0,
                y: 0.0,
                width,
                height: 0.0,
            },
            ..Default::default()
        }
    }

    #[test]
    fn edge_expansion_grows_rect() {
        let r = Rect {
            x: 10.0,
            y: 10.0,
            width: 100.0,
            height: 50.0,
        };
        let e = EdgeSizes {
            left: 5.0,
            right: 5.0,
            top: 2.0,
            bottom: 3.0,
        };
        let out = r.expanded_by(e);
        assert_eq!(out.x, 5.0);
        assert_eq!(out.width, 110.0);
        assert_eq!(out.height, 55.0);
    }

    #[test]
    fn auto_width_fills_containing_block() {
        let dom = html::parse("<div>x</div>");
        let sheet = css::parse("div { display: block; }");
        let styled = style_tree(&dom, &sheet);
        let layout = layout_tree(&styled, viewport(500.0));
        assert_eq!(layout.children[0].dimensions.content.width, 500.0);
    }

    #[test]
    fn display_none_is_skipped_in_box_tree() {
        let dom = html::parse("<div class=a>a</div><div class=b>b</div>");
        let sheet = css::parse("div { display: block; } .a { display: none; }");
        let styled = style_tree(&dom, &sheet);
        let layout = layout_tree(&styled, viewport(400.0));
        assert_eq!(layout.children.len(), 1);
    }
}
