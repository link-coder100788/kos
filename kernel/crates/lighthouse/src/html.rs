//! The HTML tokenizer and tree builder.
//!
//! This is a documented subset of HTML parsing, not the full HTML5 error
//! recovery algorithm. It is split into two phases exactly like a real engine:
//!
//! 1. The [`Tokenizer`] turns the source string into a flat stream of
//!    [`Token`]s (start tags, end tags, text, comments, doctype).
//! 2. The [`TreeBuilder`] consumes tokens and maintains a stack of open
//!    elements to produce the nested [`Node`] tree, applying void element and
//!    implicit close rules.
//!
//! Supported:
//! - Elements, attributes (double, single and unquoted values, and boolean
//!   attributes with no value).
//! - Text with entity decoding for `&amp; &lt; &gt; &quot; &#NN; &#xHH;`.
//! - Void elements (`br`, `img`, `input`, ...) and XML style self closing
//!   (`<br/>`).
//! - Comments `<!-- ... -->` and the doctype, both discarded.
//! - Raw text elements: everything inside `<script>` and `<style>` up to the
//!   matching end tag is one text run, never markup (and `<title>` and
//!   `<textarea>` likewise, but with entities decoded).
//! - Implicit closing: a new `<li>` closes an open `<li>`, a new `<p>` or a
//!   block element closes an open `<p>`, table cells and rows close their
//!   previous sibling, and a new `<option>` closes an open `<option>`.

use crate::prelude::*;
use crate::dom::{self, AttrMap, Node};

/// A lexical token produced by the [`Tokenizer`].
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    /// `<tag attr="v">`. `self_closing` is true for `<tag/>`.
    StartTag {
        name: String,
        attributes: AttrMap,
        self_closing: bool,
    },
    /// `</tag>`.
    EndTag { name: String },
    /// A run of character data (already entity decoded).
    Text(String),
    /// `<!-- ... -->`, kept for completeness though the tree builder drops it.
    Comment(String),
    /// `<!doctype ...>`.
    Doctype,
}

/// Turns an HTML source string into a stream of [`Token`]s.
pub struct Tokenizer<'a> {
    input: &'a [u8],
    pos: usize,
}

impl<'a> Tokenizer<'a> {
    /// Create a tokenizer over the given source.
    pub fn new(input: &'a str) -> Self {
        Tokenizer {
            input: input.as_bytes(),
            pos: 0,
        }
    }

    fn eof(&self) -> bool {
        self.pos >= self.input.len()
    }

    fn peek(&self) -> u8 {
        self.input[self.pos]
    }

    fn starts_with(&self, s: &str) -> bool {
        self.input[self.pos..].starts_with(s.as_bytes())
    }

    /// Produce the full token stream.
    pub fn tokenize(mut self) -> Vec<Token> {
        let mut tokens = Vec::new();
        while !self.eof() {
            if self.peek() == b'<' {
                if self.starts_with("<!--") {
                    tokens.push(self.consume_comment());
                } else if self.starts_with("<!") || self.starts_with("<?") {
                    tokens.push(self.consume_bogus_declaration());
                } else if self.starts_with("</") {
                    if let Some(t) = self.consume_end_tag() {
                        tokens.push(t);
                    }
                } else if self.pos + 1 < self.input.len() && is_tag_name_start(self.input[self.pos + 1]) {
                    let tag = self.consume_start_tag();
                    let raw = match &tag {
                        Token::StartTag { name, self_closing: false, .. } if raw_text_kind(name).is_some() => {
                            Some(name.clone())
                        }
                        _ => None,
                    };
                    tokens.push(tag);
                    if let Some(name) = raw {
                        let text = self.consume_raw_text(&name);
                        if !text.is_empty() {
                            tokens.push(Token::Text(text));
                        }
                    }
                } else {
                    // A stray '<' that is not a tag start is literal text.
                    let text = self.consume_text();
                    if !text.is_empty() {
                        tokens.push(Token::Text(text));
                    }
                }
            } else {
                let text = self.consume_text();
                if !text.is_empty() {
                    tokens.push(Token::Text(text));
                }
            }
        }
        tokens
    }

    fn consume_text(&mut self) -> String {
        let start = self.pos;
        // Always consume at least one byte to guarantee progress.
        if !self.eof() && self.peek() == b'<' {
            self.pos += 1;
        }
        while !self.eof() && self.peek() != b'<' {
            self.pos += 1;
        }
        let raw = core::str::from_utf8(&self.input[start..self.pos]).unwrap_or("");
        decode_entities(raw)
    }

    /// Consume the contents of a raw text element up to (not including) its
    /// end tag, e.g. a script's code, where `a<b` is a comparison, not a tag.
    fn consume_raw_text(&mut self, name: &str) -> String {
        let start = self.pos;
        while !self.eof() {
            if self.peek() == b'<' && self.is_end_tag_for(name) {
                break;
            }
            self.pos += 1;
        }
        let raw = core::str::from_utf8(&self.input[start..self.pos]).unwrap_or("");
        match raw_text_kind(name) {
            Some(RawText::Escapable) => decode_entities(raw),
            _ => raw.to_string(),
        }
    }

    /// Whether the input at `pos` is `</name` followed by a non-name byte.
    fn is_end_tag_for(&self, name: &str) -> bool {
        let rest = &self.input[self.pos..];
        let n = name.len();
        rest.len() >= n + 2
            && rest.starts_with(b"</")
            && rest[2..2 + n].eq_ignore_ascii_case(name.as_bytes())
            && rest.get(2 + n).is_none_or(|&c| !is_tag_name_char(c))
    }

    fn consume_comment(&mut self) -> Token {
        self.pos += 4; // skip "<!--"
        let start = self.pos;
        while !self.eof() && !self.starts_with("-->") {
            self.pos += 1;
        }
        let raw = core::str::from_utf8(&self.input[start..self.pos]).unwrap_or("");
        let comment = raw.to_string();
        if self.starts_with("-->") {
            self.pos += 3;
        }
        Token::Comment(comment)
    }

    fn consume_bogus_declaration(&mut self) -> Token {
        // Consume "<!doctype ...>" or a processing instruction up to '>'.
        while !self.eof() && self.peek() != b'>' {
            self.pos += 1;
        }
        if !self.eof() {
            self.pos += 1; // skip '>'
        }
        Token::Doctype
    }

    fn consume_start_tag(&mut self) -> Token {
        self.pos += 1; // skip '<'
        let name = self.consume_tag_name();
        let mut attributes = AttrMap::new();
        let mut self_closing = false;
        loop {
            self.skip_whitespace();
            if self.eof() {
                break;
            }
            match self.peek() {
                b'>' => {
                    self.pos += 1;
                    break;
                }
                b'/' => {
                    self.pos += 1;
                    if !self.eof() && self.peek() == b'>' {
                        self_closing = true;
                        self.pos += 1;
                        break;
                    }
                }
                _ => {
                    let (name, value) = self.consume_attribute();
                    if !name.is_empty() {
                        attributes.entry(name).or_insert(value);
                    }
                }
            }
        }
        Token::StartTag {
            name,
            attributes,
            self_closing,
        }
    }

    fn consume_end_tag(&mut self) -> Option<Token> {
        self.pos += 2; // skip "</"
        let name = self.consume_tag_name();
        while !self.eof() && self.peek() != b'>' {
            self.pos += 1;
        }
        if !self.eof() {
            self.pos += 1; // skip '>'
        }
        if name.is_empty() {
            None
        } else {
            Some(Token::EndTag { name })
        }
    }

    fn consume_tag_name(&mut self) -> String {
        let start = self.pos;
        while !self.eof() && is_tag_name_char(self.peek()) {
            self.pos += 1;
        }
        core::str::from_utf8(&self.input[start..self.pos])
            .unwrap_or("")
            .to_ascii_lowercase()
    }

    fn consume_attribute(&mut self) -> (String, String) {
        let name = self.consume_attribute_name();
        self.skip_whitespace();
        if !self.eof() && self.peek() == b'=' {
            self.pos += 1;
            self.skip_whitespace();
            let value = self.consume_attribute_value();
            (name, value)
        } else {
            (name, String::new())
        }
    }

    fn consume_attribute_name(&mut self) -> String {
        let start = self.pos;
        while !self.eof() {
            let c = self.peek();
            if c.is_ascii_whitespace() || c == b'=' || c == b'>' || c == b'/' {
                break;
            }
            self.pos += 1;
        }
        core::str::from_utf8(&self.input[start..self.pos])
            .unwrap_or("")
            .to_ascii_lowercase()
    }

    fn consume_attribute_value(&mut self) -> String {
        if self.eof() {
            return String::new();
        }
        let quote = self.peek();
        if quote == b'"' || quote == b'\'' {
            self.pos += 1;
            let start = self.pos;
            while !self.eof() && self.peek() != quote {
                self.pos += 1;
            }
            let raw = core::str::from_utf8(&self.input[start..self.pos]).unwrap_or("");
            let value = decode_entities(raw);
            if !self.eof() {
                self.pos += 1; // closing quote
            }
            value
        } else {
            let start = self.pos;
            while !self.eof() {
                let c = self.peek();
                if c.is_ascii_whitespace() || c == b'>' {
                    break;
                }
                self.pos += 1;
            }
            let raw = core::str::from_utf8(&self.input[start..self.pos]).unwrap_or("");
            decode_entities(raw)
        }
    }

    fn skip_whitespace(&mut self) {
        while !self.eof() && self.peek().is_ascii_whitespace() {
            self.pos += 1;
        }
    }
}

/// How an element's contents are tokenized when they are not markup.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RawText {
    /// Taken literally (`script`, `style`).
    Literal,
    /// Taken literally except that entities are decoded (`title`, `textarea`).
    Escapable,
}

/// Elements whose contents are text up to their end tag, never markup.
pub fn raw_text_kind(tag: &str) -> Option<RawText> {
    match tag {
        "script" | "style" => Some(RawText::Literal),
        "title" | "textarea" => Some(RawText::Escapable),
        _ => None,
    }
}

fn is_tag_name_start(c: u8) -> bool {
    c.is_ascii_alphabetic()
}

fn is_tag_name_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'-' || c == b':' || c == b'_'
}

/// Decode the small set of supported HTML entities.
pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'&' {
            if let Some((decoded, consumed)) = match_entity(&s[i..]) {
                out.push(decoded);
                i += consumed;
                continue;
            }
        }
        // Push one UTF-8 char starting at i.
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn match_entity(s: &str) -> Option<(char, usize)> {
    let named: &[(&str, char)] = &[
        ("&amp;", '&'),
        ("&lt;", '<'),
        ("&gt;", '>'),
        ("&quot;", '"'),
        ("&apos;", '\''),
        ("&nbsp;", '\u{00a0}'),
        // Common on real pages.
        ("&copy;", '\u{00a9}'),
        ("&reg;", '\u{00ae}'),
        ("&trade;", '\u{2122}'),
        ("&middot;", '\u{00b7}'),
        ("&bull;", '\u{2022}'),
        ("&hellip;", '\u{2026}'),
        ("&ndash;", '\u{2013}'),
        ("&mdash;", '\u{2014}'),
        ("&lsquo;", '\u{2018}'),
        ("&rsquo;", '\u{2019}'),
        ("&ldquo;", '\u{201c}'),
        ("&rdquo;", '\u{201d}'),
        ("&laquo;", '\u{00ab}'),
        ("&raquo;", '\u{00bb}'),
        ("&times;", '\u{00d7}'),
        ("&euro;", '\u{20ac}'),
        ("&pound;", '\u{00a3}'),
        ("&deg;", '\u{00b0}'),
    ];
    for (name, ch) in named {
        if s.starts_with(name) {
            return Some((*ch, name.len()));
        }
    }
    if let Some(rest) = s.strip_prefix("&#") {
        let (is_hex, digits_start) = if rest.starts_with('x') || rest.starts_with('X') {
            (true, 1)
        } else {
            (false, 0)
        };
        let digits: String = rest[digits_start..]
            .chars()
            .take_while(|c| if is_hex { c.is_ascii_hexdigit() } else { c.is_ascii_digit() })
            .collect();
        if digits.is_empty() {
            return None;
        }
        let radix = if is_hex { 16 } else { 10 };
        if let Ok(code) = u32::from_str_radix(&digits, radix) {
            if let Some(ch) = char::from_u32(code) {
                let mut consumed = 2 + digits_start + digits.len();
                // Optional trailing semicolon.
                if s.as_bytes().get(consumed) == Some(&b';') {
                    consumed += 1;
                }
                return Some((ch, consumed));
            }
        }
    }
    None
}

/// The maximum element nesting depth the tree builder will create. Pathological
/// input such as hundreds of thousands of unclosed tags would otherwise build an
/// arbitrarily deep tree, and every later stage (serialize, style, layout,
/// paint, and even dropping the tree) walks it recursively, so an uncapped depth
/// is a stack overflow waiting to happen. Once the cap is reached, further start
/// tags are attached as childless elements at the current level instead of
/// opening a deeper level. The value matches the order of magnitude real
/// browsers use for their fragment parsing depth limit.
const MAX_DEPTH: usize = 512;

/// Builds a DOM tree from a token stream using a stack of open elements.
struct TreeBuilder {
    open: Vec<Node>,
}

impl TreeBuilder {
    fn new() -> Self {
        // A synthetic root holds the top level nodes.
        TreeBuilder {
            open: vec![dom::elem("__root__".to_string(), AttrMap::new(), Vec::new())],
        }
    }

    fn current_tag(&self) -> Option<&str> {
        self.open.last().and_then(|n| match &n.node_type {
            dom::NodeType::Element(e) => Some(e.tag_name.as_str()),
            dom::NodeType::Text(_) => None,
        })
    }

    fn push_child(&mut self, node: Node) {
        self.open
            .last_mut()
            .expect("open stack never empties below root")
            .children
            .push(node);
    }

    /// Append text, merging into the previous sibling when it is also text. A
    /// stray `<` is tokenized as its own text run, so without this coalescing a
    /// fragment like `<<<` would become several adjacent text nodes that a
    /// serialize then reparse round trip would fold back into one.
    fn push_text(&mut self, data: String) {
        let parent = self
            .open
            .last_mut()
            .expect("open stack never empties below root");
        if let Some(dom::Node {
            node_type: dom::NodeType::Text(existing),
            ..
        }) = parent.children.last_mut()
        {
            existing.push_str(&data);
        } else {
            parent.children.push(dom::text(data));
        }
    }

    fn open_element(&mut self, name: String, attributes: AttrMap) {
        self.open.push(dom::elem(name, attributes, Vec::new()));
    }

    /// Pop the top open element and attach it to its parent.
    fn pop_element(&mut self) {
        if self.open.len() <= 1 {
            return;
        }
        let node = self.open.pop().unwrap();
        self.push_child(node);
    }

    /// Close the nearest open element with the given name, popping intermediate
    /// unclosed elements. If no such element is open the end tag is ignored.
    fn close_named(&mut self, name: &str) {
        let mut idx = None;
        for (i, node) in self.open.iter().enumerate().skip(1) {
            if let dom::NodeType::Element(e) = &node.node_type {
                if e.tag_name == name {
                    idx = Some(i);
                }
            }
        }
        if let Some(target) = idx {
            while self.open.len() > target {
                self.pop_element();
            }
        }
    }

    /// Apply implicit closing before opening `name`.
    fn implicit_close(&mut self, name: &str) {
        loop {
            let Some(cur) = self.current_tag() else {
                return;
            };
            let should_close = match name {
                "li" => cur == "li",
                "option" => cur == "option",
                "tr" => cur == "tr" || cur == "td" || cur == "th",
                "td" | "th" => cur == "td" || cur == "th",
                "p" => cur == "p",
                _ if is_block(name) => cur == "p",
                _ => false,
            };
            if should_close {
                self.pop_element();
            } else {
                return;
            }
        }
    }

    fn finish(mut self) -> Node {
        while self.open.len() > 1 {
            self.pop_element();
        }
        self.open.pop().unwrap()
    }
}

fn is_block(tag: &str) -> bool {
    matches!(
        tag,
        "address"
            | "article"
            | "aside"
            | "blockquote"
            | "div"
            | "dl"
            | "fieldset"
            | "figcaption"
            | "figure"
            | "footer"
            | "form"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "header"
            | "hr"
            | "main"
            | "nav"
            | "ol"
            | "p"
            | "pre"
            | "section"
            | "table"
            | "ul"
    )
}

/// Parse an HTML source string into a DOM tree.
///
/// The returned node is a synthetic `__root__` element whose children are the
/// top level nodes of the document. Wrapping in a single root keeps the tree
/// shape uniform even for fragments with multiple top level elements.
pub fn parse(source: &str) -> Node {
    parse_with_max_depth(source, MAX_DEPTH)
}

/// [`parse`] with a different nesting cap. Every later stage recurses once
/// per level, so a small cap bounds how much stack rendering can use (KOS
/// renders on a 64 KiB stack). Elements nested deeper than the cap are
/// dropped as above; their text is kept.
pub fn parse_with_max_depth(source: &str, max_depth: usize) -> Node {
    let tokens = Tokenizer::new(source).tokenize();
    let mut builder = TreeBuilder::new();
    for token in tokens {
        match token {
            Token::Text(data) => {
                // Drop whitespace only text at the top level to avoid noise.
                if builder.open.len() == 1 && data.trim().is_empty() {
                    continue;
                }
                builder.push_text(data);
            }
            Token::Comment(_) | Token::Doctype => {}
            Token::StartTag {
                name,
                attributes,
                self_closing,
            } => {
                builder.implicit_close(&name);
                let is_void = dom::html_void(&name);
                // `open` always holds the synthetic root plus one entry per open
                // element, so `open.len() > MAX_DEPTH` means the nesting cap is
                // reached. At the cap only void elements are kept: they never open
                // a deeper level and serialize back to the identical token, so the
                // round trip stays stable. Any element that would nest is dropped,
                // which keeps the tree bounded without breaking `parse(serialize)`.
                if builder.open.len() > max_depth {
                    if is_void {
                        builder.push_child(dom::elem(name, attributes, Vec::new()));
                    }
                } else if self_closing || is_void {
                    builder.push_child(dom::elem(name, attributes, Vec::new()));
                } else {
                    builder.open_element(name, attributes);
                }
            }
            Token::EndTag { name } => {
                if dom::html_void(&name) {
                    continue;
                }
                builder.close_named(&name);
            }
        }
    }
    builder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_start_end_and_text() {
        let tokens = Tokenizer::new("<p>hi</p>").tokenize();
        assert_eq!(tokens.len(), 3);
        assert!(matches!(&tokens[0], Token::StartTag { name, .. } if name == "p"));
        assert!(matches!(&tokens[1], Token::Text(t) if t == "hi"));
        assert!(matches!(&tokens[2], Token::EndTag { name } if name == "p"));
    }

    #[test]
    fn parses_unquoted_and_boolean_attributes() {
        let tokens = Tokenizer::new("<input type=text disabled>").tokenize();
        if let Token::StartTag { attributes, .. } = &tokens[0] {
            assert_eq!(attributes.get("type"), Some(&"text".to_string()));
            assert_eq!(attributes.get("disabled"), Some(&String::new()));
        } else {
            panic!("expected start tag");
        }
    }

    #[test]
    fn decodes_named_and_numeric_entities() {
        assert_eq!(decode_entities("a&amp;b"), "a&b");
        assert_eq!(decode_entities("&#65;&#x42;"), "AB");
        assert_eq!(decode_entities("&lt;&gt;"), "<>");
    }

    #[test]
    fn self_closing_tag_is_flagged() {
        let tokens = Tokenizer::new("<br/>").tokenize();
        assert!(matches!(&tokens[0], Token::StartTag { self_closing: true, .. }));
    }
}
