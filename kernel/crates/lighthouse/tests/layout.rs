//! Gate 3: layout invariants and golden rectangles.

use lighthouse::css;
use lighthouse::fuzz;
use lighthouse::html;
use lighthouse::layout::{layout_tree, Dimensions, LayoutBox, Rect};
use lighthouse::style::style_tree;

fn layout_with(dom_src: &str, css_src: &str, width: f32) -> LayoutBox<'static> {
    // Leak the inputs so the returned tree can borrow them for the test body.
    let dom = Box::leak(Box::new(html::parse(dom_src)));
    let sheet = Box::leak(Box::new(css::parse(css_src)));
    let styled = Box::leak(Box::new(style_tree(dom, sheet)));
    let viewport = Dimensions {
        content: Rect {
            x: 0.0,
            y: 0.0,
            width,
            height: 0.0,
        },
        ..Default::default()
    };
    layout_tree(styled, viewport)
}

fn assert_rect(actual: Rect, x: f32, y: f32, w: f32, h: f32) {
    let eq = |a: f32, b: f32| (a - b).abs() < 0.01;
    assert!(
        eq(actual.x, x) && eq(actual.y, y) && eq(actual.width, w) && eq(actual.height, h),
        "expected (x:{x} y:{y} w:{w} h:{h}) got (x:{} y:{} w:{} h:{})",
        actual.x,
        actual.y,
        actual.width,
        actual.height
    );
}

#[test]
fn golden_single_block_box_model() {
    let css_src = "div { display: block; }\n\
                   .a { width: 200px; padding: 10px; margin: 5px; height: 50px; }\n";
    let layout = layout_with(r#"<div class="a"></div>"#, css_src, 400.0);
    let div = &layout.children[0];
    // content.x = margin(5) + padding(10) = 15; content.y = margin(5) + padding(10) = 15.
    assert_rect(div.dimensions.content, 15.0, 15.0, 200.0, 50.0);
}

#[test]
fn golden_nested_blocks_and_auto_height() {
    let css_src = "div { display: block; }\n\
                   .outer { width: 300px; padding: 20px; }\n\
                   .inner { height: 40px; margin: 10px; }\n";
    let layout = layout_with(
        r#"<div class="outer"><div class="inner"></div></div>"#,
        css_src,
        400.0,
    );
    let outer = &layout.children[0];
    let inner = &outer.children[0];
    // outer content x/y = padding 20; width fixed 300; height grows to inner margin box (60).
    assert_rect(outer.dimensions.content, 20.0, 20.0, 300.0, 60.0);
    // inner auto width fills outer content minus its margins (300 - 20 = 280).
    assert_rect(inner.dimensions.content, 30.0, 30.0, 280.0, 40.0);
}

#[test]
fn golden_siblings_stack_vertically() {
    let css_src = "div { display: block; }\n\
                   .box { height: 30px; margin: 5px; }\n";
    let layout = layout_with(
        r#"<div class="box"></div><div class="box"></div>"#,
        css_src,
        200.0,
    );
    let first = &layout.children[0];
    let second = &layout.children[1];
    // First content.y = margin 5. First margin box bottom = 5+30+5 = 40.
    // Second content.y = 40 + margin 5 = 45.
    assert_rect(first.dimensions.content, 5.0, 5.0, 190.0, 30.0);
    assert_rect(second.dimensions.content, 5.0, 45.0, 190.0, 30.0);
}

#[test]
fn no_negative_dimensions_when_overconstrained() {
    // Width larger than the viewport: auto margins collapse, width clamps at 0
    // rather than going negative.
    let css_src = "div { display: block; }\n\
                   .wide { width: 1000px; margin: 50px; }\n";
    let layout = layout_with(r#"<div class="wide"></div>"#, css_src, 100.0);
    let div = &layout.children[0];
    assert!(div.dimensions.content.width >= 0.0);
    assert!(div.dimensions.content.height >= 0.0);
}

#[test]
fn golden_box_sizing_border_box() {
    // With border-box the specified 200px width and 100px height include the
    // 20px padding and 10px border on each side, so the content box is
    // 200-40-20 = 140 wide and 100-40-20 = 40 tall.
    let css_src = "div { display: block; }\n\
                   .a { width: 200px; height: 100px; padding: 20px; \
                        border-width: 10px; box-sizing: border-box; }\n";
    let layout = layout_with(r#"<div class="a"></div>"#, css_src, 400.0);
    let div = &layout.children[0];
    assert_rect(div.dimensions.content, 30.0, 30.0, 140.0, 40.0);
}

#[test]
fn overflow_child_is_clipped_and_contained() {
    // The inner box asks for 500px inside a 100px parent. It must be clipped so
    // its border box never escapes the parent content box.
    let css_src = "div { display: block; }\n\
                   .outer { width: 100px; }\n\
                   .inner { width: 500px; height: 10px; }\n";
    let layout = layout_with(
        r#"<div class="outer"><div class="inner"></div></div>"#,
        css_src,
        800.0,
    );
    let outer = &layout.children[0].dimensions.content;
    let inner = layout.children[0].children[0].dimensions.border_box();
    assert!(
        inner.x >= outer.x - 0.01 && inner.x + inner.width <= outer.x + outer.width + 0.01,
        "inner border box (x:{} w:{}) escaped outer content (x:{} w:{})",
        inner.x,
        inner.width,
        outer.x,
        outer.width
    );
}

#[test]
fn overflow_wide_box_clips_width_to_viewport() {
    let css_src = "div { display: block; }\n.wide { width: 1000px; }\n";
    let layout = layout_with(r#"<div class="wide"></div>"#, css_src, 100.0);
    let w = layout.children[0].dimensions.content.width;
    assert!((w - 100.0).abs() < 0.01, "expected clipped width 100, got {w}");
}

#[test]
fn fuzz_layout_invariants_hold() {
    // Bounded run: seed and op count come from the environment in CI.
    let ops = fuzz::ops_from_env();
    let seed = fuzz::seed_from_env();
    let result = fuzz::run(ops, seed);
    assert!(result.ok, "layout invariant violated: {:?}", result.violation);
}
