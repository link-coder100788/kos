//! Gate 2: CSS cascade and specificity.

use lighthouse::css::{self, Color, Selector, SimpleSelector, Value};
use lighthouse::html;
use lighthouse::style::style_tree;

fn color(r: u8, g: u8, b: u8) -> Value {
    Value::ColorValue(Color { r, g, b, a: 255 })
}

fn winning_color(dom_src: &str, css_src: &str, tag_path: &[usize]) -> Value {
    let dom = html::parse(dom_src);
    let sheet = css::parse(css_src);
    let styled = style_tree(&dom, &sheet);
    let mut node = &styled;
    for &i in tag_path {
        node = &node.children[i];
    }
    node.value("color").expect("color should be set")
}

#[test]
fn specificity_reference_calculation() {
    // id beats class beats tag, computed as (id, class, tag).
    let id = Selector::Simple(SimpleSelector {
        id: Some("x".into()),
        ..Default::default()
    });
    let class = Selector::Simple(SimpleSelector {
        class: vec!["y".into()],
        ..Default::default()
    });
    let tag = Selector::Simple(SimpleSelector {
        tag_name: Some("div".into()),
        ..Default::default()
    });
    let compound = Selector::Simple(SimpleSelector {
        tag_name: Some("p".into()),
        class: vec!["a".into(), "b".into()],
        id: Some("z".into()),
    });

    assert_eq!(id.specificity(), (1, 0, 0));
    assert_eq!(class.specificity(), (0, 1, 0));
    assert_eq!(tag.specificity(), (0, 0, 1));
    assert_eq!(compound.specificity(), (1, 2, 1));

    assert!(id.specificity() > class.specificity());
    assert!(class.specificity() > tag.specificity());
    // A single id outranks two classes.
    let two_classes = Selector::Simple(SimpleSelector {
        class: vec!["a".into(), "b".into()],
        ..Default::default()
    });
    assert!(id.specificity() > two_classes.specificity());
}

#[test]
fn highest_specificity_wins_over_source_order() {
    // Even though .note comes last in source order, #special has higher
    // specificity and must win.
    let css_src = "p { color: red; }\n\
                   p.note { color: green; }\n\
                   #special { color: blue; }\n\
                   .note { color: black; }\n";
    let dom_src = r#"<p id="special" class="note">hi</p>"#;
    assert_eq!(winning_color(dom_src, css_src, &[0]), color(0, 0, 255));
}

#[test]
fn latest_source_order_wins_at_equal_specificity() {
    let css_src = ".a { color: red; }\n.a { color: green; }\n";
    let dom_src = r#"<span class="a">hi</span>"#;
    assert_eq!(winning_color(dom_src, css_src, &[0]), color(0, 128, 0));
}

#[test]
fn compound_selector_beats_single_class() {
    let css_src = ".note { color: red; }\np.note { color: green; }\n";
    let dom_src = r#"<p class="note">hi</p>"#;
    // p.note is (0,1,1) which beats .note (0,1,0).
    assert_eq!(winning_color(dom_src, css_src, &[0]), color(0, 128, 0));
}

#[test]
fn inheritance_of_color_from_ancestor() {
    let css_src = "#root { color: blue; }\n";
    let dom_src = r#"<div id="root"><p><span>deep</span></p></div>"#;
    // color inherits down to the span even though only #root sets it.
    assert_eq!(winning_color(dom_src, css_src, &[0, 0, 0]), color(0, 0, 255));
}

#[test]
fn malformed_css_terminates_and_does_not_hang() {
    // Each of these once caused the parser to spin forever on a byte it never
    // consumed at selector position. Reaching the assertion at all proves the
    // parser now always makes forward progress.
    let hostile = [
        ":",
        "::::",
        "@media screen { div { color: red; } }",
        "!!! { }",
        "(x) { color: red }",
        "% ^ & { }",
        "div { color: red } : @ ! ( ) p { color: blue }",
        "}}}{{{;;;",
        "/* unterminated",
    ];
    for src in hostile {
        let sheet = css::parse(src);
        // Applying the (possibly empty) sheet must also not panic.
        let dom = html::parse("<div><p>x</p></div>");
        let _ = style_tree(&dom, &sheet);
    }
}

#[test]
fn non_finite_lengths_are_rejected() {
    // "nan", "inf" and f32 overflowing exponents must not become numeric values,
    // or they would poison layout with NaN and infinity.
    for junk in ["nan", "inf", "-inf", "1e40", "1e40px", "NaN"] {
        match css::parse_value(junk) {
            Value::Length(n, _) | Value::Number(n) => {
                panic!("{junk:?} parsed to non-finite numeric {n}");
            }
            Value::Keyword(_) | Value::ColorValue(_) => {}
        }
    }
    // A valid finite length still parses.
    assert_eq!(css::parse_value("12px"), Value::Length(12.0, css::Unit::Px));
}

#[test]
fn non_inherited_property_does_not_leak() {
    let css_src = "#root { width: 100px; }\n";
    let dom_src = r#"<div id="root"><p>child</p></div>"#;
    let dom = html::parse(dom_src);
    let sheet = css::parse(css_src);
    let styled = style_tree(&dom, &sheet);
    let root = &styled.children[0];
    let p = &root.children[0];
    assert!(root.value("width").is_some());
    assert!(p.value("width").is_none(), "width must not inherit");
}
