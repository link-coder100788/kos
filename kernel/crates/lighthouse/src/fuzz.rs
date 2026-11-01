//! Property based invariant checking for the layout engine.
//!
//! Real layout bugs often show up only on odd tree shapes, so this module
//! generates random simple block documents and checks structural invariants
//! that must hold for any correct block layout:
//!
//! 1. Every in flow block child's border box lies inside its parent's content
//!    box (no horizontal or vertical escape).
//! 2. Block siblings stack without vertical overlap (each starts at or below the
//!    previous sibling's margin box bottom).
//! 3. No box has a negative width or height.
//!
//! The generator is a tiny deterministic PRNG so failures are reproducible. The
//! number of operations and the seed are controllable through the environment
//! variables `LIGHTHOUSE_FUZZ_OPS` and `LIGHTHOUSE_FUZZ_SEED`, which keeps the
//! run bounded in CI while allowing longer local runs.

use crate::prelude::*;
use crate::css::{self, Stylesheet};
use crate::dom::{self, AttrMap, Node};
use core::fmt::Write as _;
use crate::layout::{layout_tree, BoxType, Dimensions, LayoutBox, Rect};
use crate::style::style_tree;

/// A small linear congruential generator. Deterministic and dependency free.
pub struct Rng {
    state: u64,
}

impl Rng {
    /// Seed the generator.
    pub fn new(seed: u64) -> Self {
        Rng {
            state: seed
                .wrapping_mul(2_862_933_555_777_941_757)
                .wrapping_add(3_037_000_493),
        }
    }

    fn next_u64(&mut self) -> u64 {
        // Numerical Recipes LCG constants.
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.state
    }

    /// A value in `0..bound`.
    pub fn below(&mut self, bound: u32) -> u32 {
        if bound == 0 {
            return 0;
        }
        (self.next_u64() >> 33) as u32 % bound
    }
}

/// Read the fuzz operation budget from the environment, defaulting to a small
/// CI friendly value.
#[cfg(feature = "std")]
pub fn ops_from_env() -> u32 {
    std::env::var("LIGHTHOUSE_FUZZ_OPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(200)
}

/// Read the base seed from the environment, defaulting to a fixed value.
#[cfg(feature = "std")]
pub fn seed_from_env() -> u64 {
    std::env::var("LIGHTHOUSE_FUZZ_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0x00C0_FFEE)
}

/// Generate a random document: nested `div`s with per element ids and a
/// stylesheet giving each a random height, padding and margin. Widths are
/// deliberately allowed to overflow the parent (large fixed widths, wide
/// padding, and `box-sizing: border-box`), which is exactly the domain the
/// containment invariant must survive: the engine clamps the border box to the
/// containing block, so containment has to hold even under intentional overflow.
pub fn generate_document(rng: &mut Rng, max_nodes: u32) -> (Node, Stylesheet) {
    let mut css_src = String::from("div { display: block; }\n");
    let mut counter = 0u32;
    let root = generate_node(rng, &mut counter, max_nodes.max(1), 0, &mut css_src);
    let stylesheet = css::parse(&css_src);
    (
        dom::elem("__root__".to_string(), AttrMap::new(), vec![root]),
        stylesheet,
    )
}

fn generate_node(
    rng: &mut Rng,
    counter: &mut u32,
    budget: u32,
    depth: u32,
    css_src: &mut String,
) -> Node {
    let id = format!("n{}", *counter);
    *counter += 1;

    let height = rng.below(80);
    let padding = rng.below(40);
    let margin = rng.below(40);
    let mut decl = format!("height: {height}px; padding: {padding}px; margin: {margin}px;");

    // Width: leave auto a third of the time, otherwise pick a fixed width that
    // can be anywhere from tiny to far wider than the 800px viewport, so
    // children routinely try to overflow their parent.
    if rng.below(3) != 0 {
        let width = rng.below(1200);
        let _ = write!(decl, " width: {width}px;");
    }
    // Occasionally use border-box so padding and border eat into the width.
    if rng.below(4) == 0 {
        decl.push_str(" box-sizing: border-box;");
    }
    let _ = writeln!(css_src, "#{id} {{ {decl} }}");

    let mut attrs = AttrMap::new();
    attrs.insert("id".to_string(), id);

    let mut children = Vec::new();
    if depth < 6 && *counter < budget {
        let n = rng.below(4);
        for _ in 0..n {
            if *counter >= budget {
                break;
            }
            children.push(generate_node(rng, counter, budget, depth + 1, css_src));
        }
    }
    dom::elem("div".to_string(), attrs, children)
}

/// The result of checking one generated document.
#[derive(Debug)]
pub struct CheckResult {
    /// Whether all invariants held.
    pub ok: bool,
    /// A human readable description of the first violation, if any.
    pub violation: Option<String>,
}

/// Run the invariant checks over `iterations` random documents starting from
/// `base_seed`. Returns the first failure, or a success result.
pub fn run(iterations: u32, base_seed: u64) -> CheckResult {
    let viewport = Dimensions {
        content: Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 0.0,
        },
        ..Default::default()
    };

    for i in 0..iterations {
        let seed = base_seed.wrapping_add(i as u64);
        let mut rng = Rng::new(seed);
        let max_nodes = 3 + rng.below(15);
        let (dom_root, stylesheet) = generate_document(&mut rng, max_nodes);
        let styled = style_tree(&dom_root, &stylesheet);
        let layout = layout_tree(&styled, viewport);

        if let Some(msg) = check_invariants(&layout) {
            return CheckResult {
                ok: false,
                violation: Some(format!("seed {seed}: {msg}")),
            };
        }
    }
    CheckResult {
        ok: true,
        violation: None,
    }
}

/// Check the layout invariants over a single tree. Returns the first violation.
pub fn check_invariants(root: &LayoutBox) -> Option<String> {
    // Non negative dimensions everywhere.
    if let Some(msg) = check_non_negative(root) {
        return Some(msg);
    }
    // Containment and sibling stacking on block boxes.
    check_block_relations(root)
}

fn check_non_negative(b: &LayoutBox) -> Option<String> {
    let c = b.dimensions.content;
    // Non-finite values (NaN, infinity) compare false against everything, so
    // they would silently pass a plain `< -EPS` test. Reject them explicitly.
    if !c.width.is_finite() || !c.height.is_finite() {
        return Some(format!(
            "non-finite dimension: width {} height {}",
            c.width, c.height
        ));
    }
    if c.width < -EPS || c.height < -EPS {
        return Some(format!(
            "negative dimension: width {:.3} height {:.3}",
            c.width, c.height
        ));
    }
    for child in &b.children {
        if let Some(m) = check_non_negative(child) {
            return Some(m);
        }
    }
    None
}

const EPS: f32 = 0.01;

fn is_block(b: &LayoutBox) -> bool {
    matches!(b.box_type, BoxType::BlockNode(_) | BoxType::AnonymousBlock)
}

fn check_block_relations(parent: &LayoutBox) -> Option<String> {
    let block_children: Vec<&LayoutBox> = parent.children.iter().filter(|c| is_block(c)).collect();

    let parent_content = parent.dimensions.content;
    let mut prev_bottom: Option<f32> = None;

    for child in &block_children {
        let border = child.dimensions.border_box();

        // Invariant 1: containment within the parent content box.
        if border.x < parent_content.x - EPS {
            return Some(format!(
                "child border box escapes left: child.x {:.3} < parent.x {:.3}",
                border.x, parent_content.x
            ));
        }
        if border.x + border.width > parent_content.x + parent_content.width + EPS {
            return Some(format!(
                "child border box escapes right: {:.3} > {:.3}",
                border.x + border.width,
                parent_content.x + parent_content.width
            ));
        }
        if border.y < parent_content.y - EPS {
            return Some(format!(
                "child border box escapes top: child.y {:.3} < parent.y {:.3}",
                border.y, parent_content.y
            ));
        }

        // Invariant 2: siblings stack without vertical overlap.
        if let Some(pb) = prev_bottom {
            if border.y < pb - EPS {
                return Some(format!(
                    "sibling overlap: child.y {:.3} < prev sibling bottom {:.3}",
                    border.y, pb
                ));
            }
        }
        prev_bottom = Some(child.dimensions.margin_box().y + child.dimensions.margin_box().height);
    }

    for child in &parent.children {
        if let Some(m) = check_block_relations(child) {
            return Some(m);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_is_deterministic() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..100 {
            assert_eq!(a.below(1000), b.below(1000));
        }
    }

    #[test]
    fn rng_stays_in_bounds() {
        let mut rng = Rng::new(7);
        for _ in 0..1000 {
            assert!(rng.below(10) < 10);
        }
    }

    #[test]
    fn small_run_holds_invariants() {
        let result = run(50, 1);
        assert!(result.ok, "{:?}", result.violation);
    }
}
