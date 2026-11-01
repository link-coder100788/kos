//! The Document Object Model.
//!
//! A DOM tree is a tree of [`Node`]s. A node is either an [`NodeType::Element`]
//! (a tag with a name and attributes) or an [`NodeType::Text`] run. Comments and
//! the doctype are dropped by the parser, matching how a rendering engine treats
//! them for layout purposes.

use crate::prelude::*;
use alloc::collections::BTreeMap;

/// A map of attribute name to value. A `BTreeMap` is used so that iteration and
/// serialization are deterministic, which makes round trip tests stable.
pub type AttrMap = BTreeMap<String, String>;

/// A single node in the DOM tree.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// Child nodes in document order.
    pub children: Vec<Node>,
    /// The kind of node (element or text).
    pub node_type: NodeType,
}

/// The two node kinds Lighthouse models.
#[derive(Debug, Clone, PartialEq)]
pub enum NodeType {
    /// A text run. The string is the already entity decoded text content.
    Text(String),
    /// An element with a tag name and attributes.
    Element(ElementData),
}

/// The data carried by an element node.
#[derive(Debug, Clone, PartialEq)]
pub struct ElementData {
    /// Lowercased tag name, for example `div`.
    pub tag_name: String,
    /// Attributes keyed by lowercased name.
    pub attributes: AttrMap,
}

/// Construct a text node.
pub fn text(data: String) -> Node {
    Node {
        children: Vec::new(),
        node_type: NodeType::Text(data),
    }
}

/// Construct an element node with the given tag, attributes and children.
pub fn elem(name: String, attrs: AttrMap, children: Vec<Node>) -> Node {
    Node {
        children,
        node_type: NodeType::Element(ElementData {
            tag_name: name,
            attributes: attrs,
        }),
    }
}

impl ElementData {
    /// The value of the `id` attribute, if present.
    pub fn id(&self) -> Option<&String> {
        self.attributes.get("id")
    }

    /// The set of class names from the `class` attribute, split on whitespace.
    pub fn classes(&self) -> Vec<&str> {
        match self.attributes.get("class") {
            Some(classlist) => classlist.split_whitespace().collect(),
            None => Vec::new(),
        }
    }

    /// Look up an arbitrary attribute value.
    pub fn get_attribute(&self, name: &str) -> Option<&String> {
        self.attributes.get(name)
    }
}

impl Node {
    /// Serialize this node (and its subtree) back into HTML text.
    ///
    /// This is the inverse of parsing for the documented subset. Void elements
    /// are emitted without a closing tag and text is entity encoded, so that
    /// `parse(serialize(parse(x)))` equals `parse(x)`.
    pub fn to_html(&self) -> String {
        let mut out = String::new();
        self.serialize_into(&mut out);
        out
    }

    fn serialize_into(&self, out: &mut String) {
        match &self.node_type {
            NodeType::Text(data) => out.push_str(&encode_text(data)),
            NodeType::Element(elem) => {
                // The synthetic document root is not real markup: serialize only
                // its children so a round trip reparses to the same tree.
                if elem.tag_name == "__root__" {
                    for child in &self.children {
                        child.serialize_into(out);
                    }
                    return;
                }
                out.push('<');
                out.push_str(&elem.tag_name);
                for (name, value) in &elem.attributes {
                    out.push(' ');
                    out.push_str(name);
                    out.push_str("=\"");
                    out.push_str(&encode_attr(value));
                    out.push('"');
                }
                out.push('>');
                if html_void(&elem.tag_name) {
                    return;
                }
                // Script and style contents were parsed literally, so write
                // them back literally too; encoding them would change them.
                if crate::html::raw_text_kind(&elem.tag_name) == Some(crate::html::RawText::Literal) {
                    for child in &self.children {
                        match &child.node_type {
                            NodeType::Text(data) => out.push_str(data),
                            NodeType::Element(_) => child.serialize_into(out),
                        }
                    }
                } else {
                    for child in &self.children {
                        child.serialize_into(out);
                    }
                }
                out.push_str("</");
                out.push_str(&elem.tag_name);
                out.push('>');
            }
        }
    }

    /// Pretty print the DOM tree as an indented outline.
    pub fn pretty(&self) -> String {
        let mut out = String::new();
        self.pretty_into(&mut out, 0);
        out
    }

    fn pretty_into(&self, out: &mut String, depth: usize) {
        for _ in 0..depth {
            out.push_str("  ");
        }
        match &self.node_type {
            NodeType::Text(data) => {
                let trimmed = data.trim();
                out.push_str("#text \"");
                out.push_str(trimmed);
                out.push_str("\"\n");
            }
            NodeType::Element(elem) => {
                out.push('<');
                out.push_str(&elem.tag_name);
                for (name, value) in &elem.attributes {
                    out.push(' ');
                    out.push_str(name);
                    out.push('=');
                    out.push('"');
                    out.push_str(value);
                    out.push('"');
                }
                out.push_str(">\n");
                for child in &self.children {
                    child.pretty_into(out, depth + 1);
                }
            }
        }
    }
}

/// The HTML void elements. They never have children or an end tag.
pub fn html_void(tag: &str) -> bool {
    matches!(
        tag,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

fn encode_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

fn encode_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_split_on_whitespace() {
        let mut attrs = AttrMap::new();
        attrs.insert("class".to_string(), "  a  b c ".to_string());
        let node = elem("div".to_string(), attrs, Vec::new());
        if let NodeType::Element(e) = &node.node_type {
            assert_eq!(e.classes(), vec!["a", "b", "c"]);
        } else {
            panic!("expected element");
        }
    }

    #[test]
    fn void_tag_serializes_without_end_tag() {
        let node = elem("br".to_string(), AttrMap::new(), Vec::new());
        assert_eq!(node.to_html(), "<br>");
    }

    #[test]
    fn text_is_entity_encoded_on_serialize() {
        let node = text("a < b & c".to_string());
        assert_eq!(node.to_html(), "a &lt; b &amp; c");
    }

    #[test]
    fn html_void_recognizes_known_tags() {
        assert!(html_void("img"));
        assert!(html_void("input"));
        assert!(!html_void("div"));
    }
}
