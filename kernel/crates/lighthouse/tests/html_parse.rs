//! Gate 1: HTML parse correctness and DOM round trip.

use lighthouse::dom::{Node, NodeType};
use lighthouse::html;

fn tag_of(node: &Node) -> &str {
    match &node.node_type {
        NodeType::Element(e) => &e.tag_name,
        NodeType::Text(_) => "#text",
    }
}

fn text_of(node: &Node) -> &str {
    match &node.node_type {
        NodeType::Text(t) => t,
        _ => panic!("not a text node"),
    }
}

#[test]
fn parses_nesting_attributes_and_text() {
    let dom = html::parse(r#"<div id="a" class="x y"><p>Hello <b>world</b></p></div>"#);
    assert_eq!(tag_of(&dom), "__root__");
    assert_eq!(dom.children.len(), 1);

    let div = &dom.children[0];
    assert_eq!(tag_of(div), "div");
    if let NodeType::Element(e) = &div.node_type {
        assert_eq!(e.get_attribute("id"), Some(&"a".to_string()));
        assert_eq!(e.classes(), vec!["x", "y"]);
    } else {
        panic!("expected element");
    }

    let p = &div.children[0];
    assert_eq!(tag_of(p), "p");
    assert_eq!(p.children.len(), 2);
    assert_eq!(text_of(&p.children[0]), "Hello ");
    let b = &p.children[1];
    assert_eq!(tag_of(b), "b");
    assert_eq!(text_of(&b.children[0]), "world");
}

#[test]
fn void_and_self_closing_elements_have_no_children() {
    let dom = html::parse(r#"<div><img src="p.png"><br><input type="text"/></div>"#);
    let div = &dom.children[0];
    assert_eq!(div.children.len(), 3);
    for child in &div.children {
        assert!(child.children.is_empty(), "{} should have no children", tag_of(child));
    }
    assert_eq!(tag_of(&div.children[0]), "img");
    assert_eq!(tag_of(&div.children[1]), "br");
    assert_eq!(tag_of(&div.children[2]), "input");
}

#[test]
fn implicit_close_of_list_items() {
    let dom = html::parse("<ul><li>a<li>b<li>c</ul>");
    let ul = &dom.children[0];
    assert_eq!(tag_of(ul), "ul");
    assert_eq!(ul.children.len(), 3);
    for (i, expected) in ["a", "b", "c"].iter().enumerate() {
        let li = &ul.children[i];
        assert_eq!(tag_of(li), "li");
        assert_eq!(text_of(&li.children[0]), *expected);
    }
}

#[test]
fn implicit_close_of_paragraphs() {
    let dom = html::parse("<p>one<p>two");
    assert_eq!(dom.children.len(), 2);
    assert_eq!(tag_of(&dom.children[0]), "p");
    assert_eq!(tag_of(&dom.children[1]), "p");
    assert_eq!(text_of(&dom.children[0].children[0]), "one");
    assert_eq!(text_of(&dom.children[1].children[0]), "two");
}

#[test]
fn block_element_closes_open_paragraph() {
    let dom = html::parse("<p>text<div>block</div>");
    assert_eq!(dom.children.len(), 2);
    assert_eq!(tag_of(&dom.children[0]), "p");
    assert_eq!(tag_of(&dom.children[1]), "div");
}

#[test]
fn decodes_entities_in_text_and_attributes() {
    let dom = html::parse(r#"<a title="a &amp; b">1 &lt; 2 &#65; &#x42;</a>"#);
    let a = &dom.children[0];
    if let NodeType::Element(e) = &a.node_type {
        assert_eq!(e.get_attribute("title"), Some(&"a & b".to_string()));
    } else {
        panic!("expected element");
    }
    assert_eq!(text_of(&a.children[0]), "1 < 2 A B");
}

#[test]
fn comments_and_doctype_are_dropped() {
    let dom = html::parse("<!doctype html><!-- hi --><div>x</div>");
    assert_eq!(dom.children.len(), 1);
    assert_eq!(tag_of(&dom.children[0]), "div");
}

fn depth_of(node: &Node) -> usize {
    1 + node.children.iter().map(depth_of).max().unwrap_or(0)
}

#[test]
fn deeply_nested_input_is_depth_capped_and_does_not_overflow() {
    // 200k unclosed <div> tags used to build a 200k deep tree and overflow the
    // stack in the recursive serialize and layout stages. The tree builder now
    // caps nesting depth, so parsing, serializing and pretty printing all stay
    // bounded and never panic.
    let n = 200_000;
    let mut src = String::new();
    for _ in 0..n {
        src.push_str("<div>");
    }
    let dom = html::parse(&src);
    assert!(
        depth_of(&dom) <= 520,
        "tree depth {} not capped",
        depth_of(&dom)
    );
    // These recursive walks must not overflow the stack.
    let _ = dom.to_html();
    let _ = dom.pretty();
}

#[test]
fn adversarial_markup_never_panics() {
    // A corpus of hostile inputs: unclosed tags, stray delimiters, giant
    // attributes, unterminated quotes and comments, bad entities, raw bytes.
    let corpus = [
        "<",
        ">",
        "<<<>>>",
        "<a<<b>",
        "<div class=\"unterminated",
        "<div ============>",
        "<!-- unterminated comment",
        "<!doctype",
        "</></></>",
        "<div></span></div>",
        "&amp;&#;&#xZZ;&nosemi&#x1F600;",
        "<img src=\"a\" src=\"b\" src=\"c\">",
        "<p><p><p><p><li><li><td><tr>",
        "text 你好 \u{00a0} more",
        "<style>body{color:red}</style>plain",
    ];
    for src in corpus {
        let dom = html::parse(src);
        // Round trip must be stable and must not panic.
        let reparsed = html::parse(&dom.to_html());
        assert_eq!(dom, reparsed, "round trip changed the tree for {src:?}");
    }
}

#[test]
fn round_trip_serialize_reparse_is_stable() {
    let sources = [
        r#"<div id="a" class="x y"><p>Hello <b>world</b></p></div>"#,
        r#"<ul><li>one</li><li>two</li></ul>"#,
        r#"<section><img src="p.png"><br><span>1 &lt; 2</span></section>"#,
        r#"<article><h1>Title</h1><p>Body text here.</p></article>"#,
    ];
    for src in sources {
        let first = html::parse(src);
        let serialized = first.to_html();
        let second = html::parse(&serialized);
        assert_eq!(first, second, "round trip failed for {src}\nserialized: {serialized}");
    }
}
