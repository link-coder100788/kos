//! Tests for the changes made in the KOS port (see README.md).

use lighthouse::css::{self, Unit, Value};
use lighthouse::dom::{Node, NodeType};
use lighthouse::html;
use lighthouse::layout::{layout_tree, layout_tree_with, BoxType, Dimensions, LayoutBox, LayoutOptions, Rect};
use lighthouse::paint::{build_display_list, DisplayCommand};
use lighthouse::style::{document_stylesheet, style_tree, Display, StyledNode};

fn viewport(width: f32) -> Dimensions {
    Dimensions { content: Rect { x: 0.0, y: 0.0, width, height: 0.0 }, ..Default::default() }
}

fn tag(node: &Node) -> Option<&str> {
    match &node.node_type {
        NodeType::Element(e) => Some(&e.tag_name),
        NodeType::Text(_) => None,
    }
}

/// The first styled node (depth first) for an element with this tag.
fn find<'a>(node: &'a StyledNode<'a>, name: &str) -> Option<&'a StyledNode<'a>> {
    if tag(node.node) == Some(name) {
        return Some(node);
    }
    node.children.iter().find_map(|c| find(c, name))
}

fn text_boxes<'a, 'b>(b: &'b LayoutBox<'a>, out: &mut Vec<&'b LayoutBox<'a>>) {
    if !b.fragments.is_empty() {
        out.push(b);
    }
    for c in &b.children {
        text_boxes(c, out);
    }
}

// ---- HTML -------------------------------------------------------------------

#[test]
fn script_contents_are_text_not_markup() {
    let dom = html::parse(r#"<script>if (a<b.length) { s = "<div>"; }</script><p>after</p>"#);
    let script = &dom.children[0];
    assert_eq!(tag(script), Some("script"));
    assert_eq!(script.children.len(), 1);
    assert!(matches!(&script.children[0].node_type, NodeType::Text(t) if t == r#"if (a<b.length) { s = "<div>"; }"#));
    assert_eq!(tag(&dom.children[1]), Some("p"));
}

#[test]
fn script_round_trips_unescaped() {
    let dom = html::parse("<style>a > b { color: red }</style><script>x < y && z</script>");
    assert_eq!(html::parse(&dom.to_html()), dom);
}

#[test]
fn title_decodes_entities_but_keeps_tags_as_text() {
    let dom = html::parse("<title>A &amp; <b>B</b></title>");
    assert!(matches!(&dom.children[0].children[0].node_type, NodeType::Text(t) if t == "A & <b>B</b>"));
}

#[test]
fn nesting_is_capped_at_the_given_depth() {
    fn depth(n: &Node) -> usize {
        1 + n.children.iter().map(depth).max().unwrap_or(0)
    }
    let src = "<div>".repeat(100) + "deep";
    let dom = html::parse_with_max_depth(&src, 48);
    // The synthetic root, 48 divs, and the text.
    assert_eq!(depth(&dom), 50);
}

// ---- CSS --------------------------------------------------------------------

fn decl(sheet: &css::Stylesheet, rule: usize, name: &str) -> Option<Value> {
    sheet.rules[rule].declarations.iter().find(|d| d.name == name).map(|d| d.value.clone())
}

#[test]
fn complex_selectors_are_dropped_not_misapplied() {
    let sheet = css::parse("div p { color: red } a:hover { color: red } [x] { color: red } p, ul li { width: 5px }");
    // Only the `p` from the last rule survives.
    assert_eq!(sheet.rules.len(), 1);
    assert_eq!(sheet.rules[0].selectors.len(), 1);
    assert_eq!(decl(&sheet, 0, "width"), Some(Value::Length(5.0, Unit::Px)));
}

#[test]
fn at_rules_are_skipped_whole() {
    let sheet = css::parse("@charset \"utf-8\"; @media (max-width: 10px) { p { color: red } .x { width: 1px } } p { width: 2px }");
    assert_eq!(sheet.rules.len(), 1);
    assert_eq!(decl(&sheet, 0, "width"), Some(Value::Length(2.0, Unit::Px)));
}

#[test]
fn shorthands_expand_to_sides() {
    let sheet = css::parse("p { margin: 1px 2px 3px 4px; padding: 5px 6px }");
    let px = |n| Some(Value::Length(n, Unit::Px));
    assert_eq!(decl(&sheet, 0, "margin-top"), px(1.0));
    assert_eq!(decl(&sheet, 0, "margin-right"), px(2.0));
    assert_eq!(decl(&sheet, 0, "margin-bottom"), px(3.0));
    assert_eq!(decl(&sheet, 0, "margin-left"), px(4.0));
    assert_eq!(decl(&sheet, 0, "padding-top"), px(5.0));
    assert_eq!(decl(&sheet, 0, "padding-left"), px(6.0));
}

#[test]
fn important_and_relative_units() {
    let sheet = css::parse("p { width: 2em !important; height: 1.5rem; font-size: 12pt }");
    assert_eq!(decl(&sheet, 0, "width"), Some(Value::Length(32.0, Unit::Px)));
    assert_eq!(decl(&sheet, 0, "height"), Some(Value::Length(24.0, Unit::Px)));
    assert_eq!(decl(&sheet, 0, "font-size"), Some(Value::Length(16.0, Unit::Px)));
}

// ---- style ------------------------------------------------------------------

#[test]
fn style_attribute_and_hidden_attribute() {
    let dom = html::parse(r#"<div style="display: none">a</div><p hidden>b</p><span style="color: #ff0000">c</span>"#);
    let sheet = css::parse("");
    let styled = style_tree(&dom, &sheet);
    assert_eq!(find(&styled, "div").unwrap().display(), Display::None);
    assert_eq!(find(&styled, "p").unwrap().display(), Display::None);
    assert!(matches!(find(&styled, "span").unwrap().value("color"), Some(Value::ColorValue(c)) if c.r == 255));
}

#[test]
fn modern_display_values_lay_out_as_blocks() {
    let dom = html::parse("<div>x</div><span>y</span><b>z</b>");
    let sheet = css::parse("div { display: flex } span { display: grid } b { display: inline-block }");
    let styled = style_tree(&dom, &sheet);
    assert_eq!(find(&styled, "div").unwrap().display(), Display::Block);
    assert_eq!(find(&styled, "span").unwrap().display(), Display::Block);
    assert_eq!(find(&styled, "b").unwrap().display(), Display::Inline);
}

#[test]
fn document_stylesheet_has_defaults_then_page_styles() {
    let dom = html::parse("<style>h2 { font-size: 40px }</style><h1>a</h1><h2>b</h2>");
    let sheet = document_stylesheet(&dom);
    let styled = style_tree(&dom, &sheet);
    assert_eq!(find(&styled, "h1").unwrap().value("font-size"), Some(Value::Length(32.0, Unit::Px)));
    // The page's rule beats the default for h2.
    assert_eq!(find(&styled, "h2").unwrap().value("font-size"), Some(Value::Length(40.0, Unit::Px)));
}

// ---- layout -----------------------------------------------------------------

#[test]
fn long_text_wraps_within_its_block() {
    // 16px text is 8px per character: 100px fits 12 characters a line.
    let dom = html::parse("<p>one two three four five six seven eight nine ten</p>");
    let sheet = css::parse("");
    let styled = style_tree(&dom, &sheet);
    let layout = layout_tree(&styled, viewport(100.0));
    let mut boxes = Vec::new();
    text_boxes(&layout, &mut boxes);
    let lines: Vec<&str> = boxes[0].fragments.iter().map(|f| f.text.as_str()).collect();
    assert_eq!(lines, ["one two", "three four", "five six", "seven eight", "nine ten"]);
    for f in &boxes[0].fragments {
        assert!(f.rect.x >= 0.0 && f.rect.x + f.rect.width <= 100.0, "{f:?} overflows");
    }
    // Lines stack, and the block grows to hold them.
    assert!(boxes[0].fragments.windows(2).all(|w| w[1].rect.y > w[0].rect.y));
    assert!(layout.dimensions.content.height >= 5.0 * 19.2 - 0.01);
}

#[test]
fn spaces_join_words_across_elements_only_where_the_source_has_them() {
    let dom = html::parse("<p>a <b>b</b>c</p>");
    let sheet = css::parse("");
    let styled = style_tree(&dom, &sheet);
    let layout = layout_tree(&styled, viewport(400.0));
    let mut boxes = Vec::new();
    text_boxes(&layout, &mut boxes);
    let xs: Vec<f32> = boxes.iter().map(|b| b.fragments[0].rect.x).collect();
    // "a", space, "b", then "c" straight after: 0, 16, 24.
    assert_eq!(xs, [0.0, 16.0, 24.0]);
}

#[test]
fn inline_margins_and_padding_space_out_elements() {
    let dom = html::parse("<p><a>one</a><a>two</a></p>");
    let sheet = css::parse("a { margin: 0 4px; padding-left: 2px }");
    let styled = style_tree(&dom, &sheet);
    let layout = layout_tree(&styled, viewport(400.0));
    let mut boxes = Vec::new();
    text_boxes(&layout, &mut boxes);
    // "one" after 4 + 2, then 4 after it, then 4 + 2 before "two".
    assert_eq!(boxes[0].fragments[0].rect.x, 6.0);
    assert_eq!(boxes[1].fragments[0].rect.x, 6.0 + 24.0 + 4.0 + 6.0);
}

/// Lays `html` out at `width`, with every `<img>` a 30x40 replaced element.
fn with_images(html: &str, css: &str, width: f32, check: impl FnOnce(&LayoutBox)) {
    let dom = html::parse(html);
    let sheet = css::parse(css);
    let styled = style_tree(&dom, &sheet);
    let replaced = |n: &StyledNode| (tag(n.node) == Some("img")).then_some((30.0, 40.0));
    let options = LayoutOptions { replaced: &replaced, ..Default::default() };
    check(&layout_tree_with(&styled, viewport(width), &options));
}

fn find_box<'a, 'b>(b: &'b LayoutBox<'a>, name: &str) -> Option<&'b LayoutBox<'a>> {
    match b.box_type {
        BoxType::BlockNode(n) | BoxType::InlineNode(n) if tag(n.node) == Some(name) => Some(b),
        _ => b.children.iter().find_map(|c| find_box(c, name)),
    }
}

#[test]
fn replaced_elements_take_their_size_on_the_line() {
    with_images("<p>a <img>b</p>", "", 400.0, |layout| {
        let img = find_box(layout, "img").unwrap();
        // After "a" and a space; the line grows to the image's height.
        assert_eq!(img.dimensions.content, Rect { x: 16.0, y: 0.0, width: 30.0, height: 40.0 });
        let mut boxes = Vec::new();
        text_boxes(layout, &mut boxes);
        assert_eq!(boxes[1].fragments[0].rect.x, 46.0);
        assert_eq!(layout.dimensions.content.height, 40.0);
    });
}

#[test]
fn replaced_elements_wrap_and_drop_their_children() {
    // 8 + 30 doesn't leave room for a second image in 60px.
    with_images("<p>a<img><img>x</p>", "", 60.0, |layout| {
        let p = find_box(layout, "p").unwrap();
        let imgs: Vec<_> = p.children[0].children.iter().filter(|b| matches!(b.box_type, BoxType::InlineNode(n) if tag(n.node) == Some("img"))).collect();
        assert_eq!(imgs.len(), 2);
        assert_eq!((imgs[1].dimensions.content.x, imgs[1].dimensions.content.y), (0.0, 40.0));
        assert!(imgs.iter().all(|b| b.children.is_empty()));
    });
}

#[test]
fn block_replaced_elements_scale_down_to_fit() {
    with_images("<img>", "img { display: block }", 15.0, |layout| {
        let img = find_box(layout, "img").unwrap();
        assert_eq!((img.dimensions.content.width, img.dimensions.content.height), (15.0, 20.0));
    });
}

#[test]
fn br_and_pre_break_lines() {
    let dom = html::parse("<p>a<br>b</p><pre>x  y\nz</pre>");
    let sheet = css::parse("pre { white-space: pre }");
    let styled = style_tree(&dom, &sheet);
    let layout = layout_tree(&styled, viewport(400.0));
    let mut boxes = Vec::new();
    text_boxes(&layout, &mut boxes);
    let a = &boxes[0].fragments[0];
    let b = &boxes[1].fragments[0];
    assert_eq!(a.rect.x, b.rect.x);
    assert!(b.rect.y > a.rect.y);
    let pre: Vec<&str> = boxes[2].fragments.iter().map(|f| f.text.as_str()).collect();
    assert_eq!(pre, ["x  y", "z"]);
}

// ---- paint ------------------------------------------------------------------

#[test]
fn links_are_underlined_and_bold_is_bold() {
    let dom = html::parse(r#"<p><a href="/">link</a> <b>bold</b></p>"#);
    let sheet = document_stylesheet(&dom);
    let styled = style_tree(&dom, &sheet);
    let layout = layout_tree(&styled, viewport(400.0));
    let texts: Vec<_> = build_display_list(&layout)
        .into_iter()
        .filter_map(|c| match c {
            DisplayCommand::Text(t, _, _, style) => Some((t, style)),
            DisplayCommand::SolidColor(..) => None,
        })
        .collect();
    assert_eq!(texts.len(), 2);
    assert!(texts[0].0 == "link" && texts[0].1.underline && !texts[0].1.bold);
    assert!(texts[1].0 == "bold" && texts[1].1.bold && !texts[1].1.underline);
}
