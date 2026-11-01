//! Style resolution: match rules to DOM nodes and compute values.
//!
//! This stage walks the DOM and, for each element, finds every rule whose
//! selector matches. Matched declarations are applied in cascade order:
//! ascending specificity, then source order, so the last strongest declaration
//! wins. The result is a [`StyledNode`] tree that mirrors the DOM but carries a
//! map of computed property values.
//!
//! A small set of properties (`color` and `font-size`) inherit from the parent
//! when a node does not specify them, matching CSS inheritance for those
//! properties.

use crate::prelude::*;
use crate::css::{Rule, Selector, SimpleSelector, Specificity, Stylesheet, Value};
use crate::dom::{ElementData, Node, NodeType};
use alloc::collections::BTreeMap;

/// Map of computed property name to value for a single node.
pub type PropertyMap = BTreeMap<String, Value>;

/// A node paired with its computed style and styled children.
#[derive(Debug, Clone)]
pub struct StyledNode<'a> {
    /// The DOM node this style applies to.
    pub node: &'a Node,
    /// Computed property values after the cascade and inheritance.
    pub specified_values: PropertyMap,
    /// Styled children.
    pub children: Vec<StyledNode<'a>>,
}

/// The CSS `display` outer type Lighthouse understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Display {
    /// A block level box.
    Block,
    /// An inline level box.
    Inline,
    /// The element and its subtree are not rendered.
    None,
}

/// Properties that inherit from parent to child when unset.
const INHERITED: &[&str] = &[
    "color",
    "font-size",
    "font-family",
    "font-weight",
    "line-height",
    "text-align",
    "white-space",
];

/// Default styles for HTML elements, like a browser's built-in stylesheet.
/// It's applied before the page's own CSS, and uses only tag selectors, so
/// any author rule for the same element wins. Vertical margins are half
/// the usual values because Lighthouse doesn't collapse adjacent margins.
pub const USER_AGENT_CSS: &str = "
body { margin: 8px; }
h1 { font-size: 32px; margin-top: 11px; margin-bottom: 11px; font-weight: bold; }
h2 { font-size: 24px; margin-top: 10px; margin-bottom: 10px; font-weight: bold; }
h3 { font-size: 19px; margin-top: 9px; margin-bottom: 9px; font-weight: bold; }
h4 { margin-top: 10px; margin-bottom: 10px; font-weight: bold; }
h5 { font-size: 13px; margin-top: 11px; margin-bottom: 11px; font-weight: bold; }
h6 { font-size: 11px; margin-top: 12px; margin-bottom: 12px; font-weight: bold; }
p, dl, figure { margin-top: 8px; margin-bottom: 8px; }
ul, ol, menu { margin-top: 8px; margin-bottom: 8px; padding-left: 40px; }
dd { margin-left: 40px; }
blockquote { margin: 8px 40px; }
pre { margin-top: 8px; margin-bottom: 8px; white-space: pre; font-family: monospace; }
hr { margin-top: 8px; margin-bottom: 8px; border-top-width: 1px; border-color: #888888; }
a { color: #0000ee; text-decoration: underline; }
b, strong, th { font-weight: bold; }
u, ins { text-decoration: underline; }
";

impl StyledNode<'_> {
    /// Look up a computed value by property name.
    pub fn value(&self, name: &str) -> Option<Value> {
        self.specified_values.get(name).cloned()
    }

    /// Look up a value, falling back to the first name that is set.
    pub fn lookup(&self, name: &str, fallback: &str, default: &Value) -> Value {
        self.value(name)
            .or_else(|| self.value(fallback))
            .unwrap_or_else(|| default.clone())
    }

    /// The `display` outer type of this node.
    ///
    /// An explicit `display` declaration always wins. Otherwise a small user
    /// agent default is applied: structural elements (`html`, `body`, `div`,
    /// `p`, headings, list items, ...) default to block, metadata elements
    /// (`head`, `style`, `script`, `title`, ...) default to none, and everything
    /// else defaults to inline, matching how a real browser lays out a page with
    /// no author stylesheet.
    pub fn display(&self) -> Display {
        if let Some(Value::Keyword(s)) = self.value("display") {
            return match s.as_str() {
                "none" => Display::None,
                "inline" | "inline-block" | "inline-flex" | "inline-grid" | "inline-table" | "contents" => {
                    Display::Inline
                }
                // flex, grid, table, list-item, flow-root, table-row, ...:
                // laying these out as plain blocks keeps their contents in
                // reading order, which beats squashing them into a line.
                _ => Display::Block,
            };
        }
        if let NodeType::Element(e) = &self.node.node_type {
            if e.attributes.contains_key("hidden") {
                return Display::None;
            }
        }
        match &self.node.node_type {
            NodeType::Text(_) => Display::Inline,
            NodeType::Element(e) => ua_default_display(&e.tag_name),
        }
    }
}

/// The user agent default `display` for an element tag.
fn ua_default_display(tag: &str) -> Display {
    match tag {
        "head" | "style" | "script" | "title" | "meta" | "link" | "base" | "template" => Display::None,
        "__root__" | "html" | "body" | "address" | "article" | "aside" | "blockquote" | "div"
        | "dl" | "dd" | "dt" | "fieldset" | "figcaption" | "figure" | "footer" | "form" | "h1"
        | "h2" | "h3" | "h4" | "h5" | "h6" | "header" | "hgroup" | "hr" | "li" | "main" | "nav"
        | "ol" | "p" | "pre" | "section" | "table" | "ul" | "center" | "details" | "summary"
        | "menu" | "legend" | "caption" | "thead" | "tbody" | "tfoot" | "tr" | "td" | "th" => {
            Display::Block
        }
        _ => Display::Inline,
    }
}

/// A single matched rule together with the specificity that matched.
type MatchedRule<'a> = (Specificity, &'a Rule);

fn matches(elem: &ElementData, selector: &Selector) -> bool {
    let Selector::Simple(simple) = selector;
    matches_simple(elem, simple)
}

fn matches_simple(elem: &ElementData, selector: &SimpleSelector) -> bool {
    if let Some(name) = &selector.tag_name {
        if *name != elem.tag_name {
            return false;
        }
    }
    if let Some(id) = &selector.id {
        if elem.id() != Some(id) {
            return false;
        }
    }
    let elem_classes = elem.classes();
    if selector
        .class
        .iter()
        .any(|c| !elem_classes.iter().any(|ec| ec == c))
    {
        return false;
    }
    true
}

fn match_rule<'a>(elem: &ElementData, rule: &'a Rule) -> Option<MatchedRule<'a>> {
    rule.selectors
        .iter()
        .filter(|s| matches(elem, s))
        .map(Selector::specificity)
        .max()
        .map(|spec| (spec, rule))
}

fn matching_rules<'a>(elem: &ElementData, stylesheet: &'a Stylesheet) -> Vec<MatchedRule<'a>> {
    stylesheet
        .rules
        .iter()
        .filter_map(|rule| match_rule(elem, rule))
        .collect()
}

/// Compute the specified values for one element by running the cascade.
///
/// Rules are sorted by specificity ascending. Because [`Vec::sort_by`] is
/// stable, rules with equal specificity keep their source order, so applying
/// them front to back leaves the highest specificity then latest declaration in
/// place. That is exactly the CSS cascade for the supported subset.
fn specified_values(
    elem: &ElementData,
    stylesheet: &Stylesheet,
    inherited: &PropertyMap,
) -> PropertyMap {
    let mut values = PropertyMap::new();

    for (name, value) in inherited {
        if INHERITED.contains(&name.as_str()) {
            values.insert(name.clone(), value.clone());
        }
    }

    let mut rules = matching_rules(elem, stylesheet);
    rules.sort_by_key(|a| a.0);
    for (_, rule) in rules {
        for declaration in &rule.declarations {
            values.insert(declaration.name.clone(), declaration.value.clone());
        }
    }
    // A `style` attribute is more specific than any selector, so it goes last.
    if let Some(inline) = elem.get_attribute("style") {
        for declaration in crate::css::parse_inline(inline) {
            values.insert(declaration.name, declaration.value);
        }
    }
    values
}

/// The stylesheet a page should be rendered with: `USER_AGENT_CSS` followed
/// by the contents of every `<style>` element in the document.
pub fn document_stylesheet(root: &Node) -> Stylesheet {
    let mut source = String::from(USER_AGENT_CSS);
    collect_styles(root, &mut source);
    crate::css::parse(&source)
}

fn collect_styles(node: &Node, out: &mut String) {
    if let NodeType::Element(elem) = &node.node_type {
        if elem.tag_name == "style" {
            for child in &node.children {
                if let NodeType::Text(t) = &child.node_type {
                    out.push_str(t);
                    out.push('\n');
                }
            }
        }
    }
    for child in &node.children {
        collect_styles(child, out);
    }
}

/// Build a styled tree from a DOM root and a stylesheet.
pub fn style_tree<'a>(root: &'a Node, stylesheet: &'a Stylesheet) -> StyledNode<'a> {
    style_node(root, stylesheet, &PropertyMap::new())
}

fn style_node<'a>(
    node: &'a Node,
    stylesheet: &'a Stylesheet,
    inherited: &PropertyMap,
) -> StyledNode<'a> {
    let specified = match &node.node_type {
        NodeType::Element(elem) => specified_values(elem, stylesheet, inherited),
        NodeType::Text(_) => {
            // Text nodes inherit but never match rules of their own.
            let mut values = PropertyMap::new();
            for (name, value) in inherited {
                if INHERITED.contains(&name.as_str()) {
                    values.insert(name.clone(), value.clone());
                }
            }
            values
        }
    };

    let children = node
        .children
        .iter()
        .map(|child| style_node(child, stylesheet, &specified))
        .collect();

    StyledNode {
        node,
        specified_values: specified,
        children,
    }
}

/// Pretty print the styled tree, one node per line with its computed values.
pub fn pretty(styled: &StyledNode) -> String {
    let mut out = String::new();
    pretty_into(styled, &mut out, 0);
    out
}

fn pretty_into(styled: &StyledNode, out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
    match &styled.node.node_type {
        NodeType::Element(elem) => {
            out.push('<');
            out.push_str(&elem.tag_name);
            out.push('>');
        }
        NodeType::Text(data) => {
            out.push_str("#text \"");
            out.push_str(data.trim());
            out.push('"');
        }
    }
    if !styled.specified_values.is_empty() {
        let mut keys: Vec<&String> = styled.specified_values.keys().collect();
        keys.sort();
        out.push_str("  {");
        for (i, key) in keys.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push_str(key);
            out.push_str(": ");
            out.push_str(&format_value(&styled.specified_values[*key]));
        }
        out.push('}');
    }
    out.push('\n');
    for child in &styled.children {
        pretty_into(child, out, depth + 1);
    }
}

fn format_value(value: &Value) -> String {
    match value {
        Value::Keyword(k) => k.clone(),
        Value::Length(n, _) => format!("{n}px"),
        Value::Number(n) => format!("{n}"),
        Value::ColorValue(c) => format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css;
    use crate::html;

    #[test]
    fn display_reads_keyword() {
        let dom = html::parse("<div>x</div>");
        let sheet = css::parse("div { display: block; }");
        let styled = style_tree(&dom, &sheet);
        assert_eq!(styled.children[0].display(), Display::Block);
    }

    #[test]
    fn display_none_and_default_inline() {
        let dom = html::parse("<a>x</a><b>y</b>");
        let sheet = css::parse("a { display: none; }");
        let styled = style_tree(&dom, &sheet);
        assert_eq!(styled.children[0].display(), Display::None);
        assert_eq!(styled.children[1].display(), Display::Inline);
    }

    #[test]
    fn lookup_falls_back() {
        let dom = html::parse("<div>x</div>");
        let sheet = css::parse("div { margin: 7px; }");
        let styled = style_tree(&dom, &sheet);
        let v = styled.children[0].lookup("margin-left", "margin", &Value::Length(0.0, css::Unit::Px));
        assert_eq!(v.to_px(), 7.0);
    }
}
