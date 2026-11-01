//! Max-scale stress and adversarial parser fuzzer.
//!
//! This binary drives three campaigns and reports raw counts:
//!
//! 1. Layout invariant fuzzing across many seeds (the same checks the gate test
//!    runs, but at a much higher operation budget).
//! 2. Adversarial HTML: random and structurally hostile markup is parsed, round
//!    tripped and pushed through the full pipeline. Every input runs inside
//!    `catch_unwind` so a panic is reported with its seed instead of aborting.
//! 3. Adversarial CSS: random and hostile stylesheets are parsed and applied.
//!
//! A watchdog thread aborts with the offending seed if any single input takes
//! longer than a few seconds, which turns an infinite loop into a precise,
//! reproducible failure rather than a silent hang.
//!
#![warn(clippy::pedantic)]
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::cast_lossless,
    clippy::cast_possible_wrap
)]
//!
//! Budgets come from the environment:
//!   `LIGHTHOUSE_FUZZ_OPS`   layout iterations (default 200)
//!   `LIGHTHOUSE_FUZZ_SEED`  base seed (default `0xC0FFEE`)
//!   `LIGHTHOUSE_ADV_OPS`    adversarial inputs per parser (default = `FUZZ_OPS`)

use lighthouse::fuzz::{self, Rng};
use lighthouse::layout::{layout_tree, Dimensions, Rect};
use lighthouse::style::style_tree;
use lighthouse::{css, html};
use std::panic;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn main() {
    let ops = fuzz::ops_from_env();
    let base_seed = fuzz::seed_from_env();
    let adv_ops = env_u64("LIGHTHOUSE_ADV_OPS", u64::from(ops)) as u32;

    // Watchdog: `current` holds `(stage_code << 40) | id`, `heartbeat` is a
    // monotonically increasing tick. If the tick stops advancing for too long we
    // are stuck on a single input, so abort and print where.
    let current = Arc::new(AtomicU64::new(0));
    let heartbeat = Arc::new(AtomicUsize::new(0));
    {
        let current = Arc::clone(&current);
        let heartbeat = Arc::clone(&heartbeat);
        std::thread::spawn(move || {
            let mut last_tick = 0usize;
            let mut stalled = Duration::ZERO;
            loop {
                std::thread::sleep(Duration::from_millis(200));
                let tick = heartbeat.load(Ordering::Relaxed);
                if tick == last_tick {
                    stalled += Duration::from_millis(200);
                    if stalled > Duration::from_secs(5) {
                        let packed = current.load(Ordering::Relaxed);
                        let stage = packed >> 40;
                        let id = packed & 0xff_ffff_ffff;
                        eprintln!(
                            "WATCHDOG: stuck for >5s on stage {stage} input id {id}. Hang detected."
                        );
                        std::process::abort();
                    }
                } else {
                    last_tick = tick;
                    stalled = Duration::ZERO;
                }
            }
        });
    }

    // Panics are expected to never happen; keep the default hook silent so the
    // per-input catch_unwind report is the single source of truth.
    panic::set_hook(Box::new(|_| {}));

    let mark = |stage: u64, id: u64| {
        current.store((stage << 40) | id, Ordering::Relaxed);
        heartbeat.fetch_add(1, Ordering::Relaxed);
    };

    let start = Instant::now();
    let mut panics = 0u64;

    // Campaign 1: layout invariants at scale. Run in chunks so the watchdog
    // keeps getting a heartbeat during the long run.
    println!("== layout invariant fuzz ==");
    let chunk = 5_000u32;
    let mut done = 0u32;
    while done < ops {
        let this = chunk.min(ops - done);
        mark(1, u64::from(done));
        let result = fuzz::run(this, base_seed.wrapping_add(u64::from(done)));
        if !result.ok {
            eprintln!("LAYOUT INVARIANT VIOLATION: {:?}", result.violation);
            std::process::exit(1);
        }
        done += this;
    }
    println!("layout: {ops} docs from seed {base_seed}: ok=true violation=None");

    // Campaign 2: adversarial HTML.
    println!("== adversarial HTML fuzz ==");
    let mut roundtrip_fail = 0u64;
    for i in 0..adv_ops {
        let seed = base_seed.wrapping_add(0x1111_0000).wrapping_add(u64::from(i));
        mark(2, u64::from(i));
        let src = adversarial_html(&mut Rng::new(seed));
        let outcome = panic::catch_unwind(panic::AssertUnwindSafe(|| run_html_pipeline(&src)));
        match outcome {
            Ok(true) => {}
            Ok(false) => {
                roundtrip_fail += 1;
                if roundtrip_fail <= 5 {
                    eprintln!("HTML round trip mismatch at seed {seed}: {:?}", truncate(&src));
                }
            }
            Err(_) => {
                panics += 1;
                eprintln!("HTML PANIC at seed {seed}: {:?}", truncate(&src));
            }
        }
    }
    println!("html: {adv_ops} inputs: panics={panics} roundtrip_mismatches={roundtrip_fail}");

    // Campaign 3: adversarial CSS.
    println!("== adversarial CSS fuzz ==");
    let mut css_panics = 0u64;
    for i in 0..adv_ops {
        let seed = base_seed.wrapping_add(0x2222_0000).wrapping_add(u64::from(i));
        mark(3, u64::from(i));
        let src = adversarial_css(&mut Rng::new(seed));
        let outcome = panic::catch_unwind(panic::AssertUnwindSafe(|| run_css_pipeline(&src)));
        if outcome.is_err() {
            css_panics += 1;
            eprintln!("CSS PANIC at seed {seed}: {:?}", truncate(&src));
        }
    }
    println!("css: {adv_ops} inputs: panics={css_panics}");

    let total_panics = panics + css_panics;
    println!(
        "== summary == total_inputs={} elapsed={:?} panics={total_panics} roundtrip_mismatches={roundtrip_fail}",
        u64::from(ops) + u64::from(adv_ops) * 2,
        start.elapsed()
    );
    if total_panics > 0 || roundtrip_fail > 0 {
        std::process::exit(1);
    }
    println!("ALL CLEAN");
}

fn truncate(s: &str) -> String {
    if s.len() <= 120 {
        s.to_string()
    } else {
        format!("{}... ({} bytes)", &s[..120.min(s.len())], s.len())
    }
}

fn run_html_pipeline(src: &str) -> bool {
    let dom = html::parse(src);
    // Round trip: reparsing the serialization must reproduce the tree.
    let serialized = dom.to_html();
    let reparsed = html::parse(&serialized);
    let round_trips = dom == reparsed;
    // Push through the rest of the pipeline to shake out downstream panics.
    let sheet = css::parse("div{display:block;} *{color:red;}");
    let styled = style_tree(&dom, &sheet);
    let _ = layout_tree(&styled, viewport());
    let _ = dom.pretty();
    round_trips
}

fn run_css_pipeline(src: &str) {
    let sheet = css::parse(src);
    let dom = html::parse("<div id=a class=x><p>hi</p><span>y</span></div>");
    let styled = style_tree(&dom, &sheet);
    let layout = layout_tree(&styled, viewport());
    let _ = lighthouse::paint::build_display_list(&layout);
}

fn viewport() -> Dimensions {
    Dimensions {
        content: Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 0.0,
        },
        ..Default::default()
    }
}

const TAGS: &[&str] = &[
    "div", "p", "span", "ul", "li", "b", "img", "br", "table", "tr", "td", "a", "h1", "style",
    "script", "input",
];
const ATTRS: &[&str] = &["id", "class", "src", "href", "style", "width", "disabled"];

/// Build a hostile HTML string: unbalanced tags, deep nesting, giant attributes,
/// unterminated quotes, stray delimiters, entity edge cases and raw noise.
fn adversarial_html(rng: &mut Rng) -> String {
    let mut s = String::new();
    let steps = 3 + rng.below(60);
    for _ in 0..steps {
        match rng.below(12) {
            0 => {
                let t = TAGS[rng.below(TAGS.len() as u32) as usize];
                s.push('<');
                s.push_str(t);
                let na = rng.below(3);
                for _ in 0..na {
                    let a = ATTRS[rng.below(ATTRS.len() as u32) as usize];
                    s.push(' ');
                    s.push_str(a);
                    match rng.below(4) {
                        0 => {}
                        1 => {
                            s.push_str("=\"");
                            s.push_str(&"x".repeat(rng.below(2000) as usize));
                        }
                        2 => {
                            s.push_str("='unterminated");
                        }
                        _ => {
                            s.push_str("=val");
                        }
                    }
                }
                if rng.below(3) == 0 {
                    s.push('/');
                }
                s.push('>');
            }
            1 => {
                let t = TAGS[rng.below(TAGS.len() as u32) as usize];
                s.push_str("</");
                s.push_str(t);
                s.push('>');
            }
            2 => s.push_str(&"<div>".repeat(rng.below(2000) as usize)),
            3 => s.push_str(&"</div>".repeat(rng.below(2000) as usize)),
            4 => s.push_str("<!-- unterminated comment"),
            5 => s.push_str("<!doctype"),
            6 => s.push_str("&amp;&lt;&#65;&#x1F600;&#;&#xZZ;&nosemicolon"),
            7 => s.push('<'),
            8 => s.push('>'),
            9 => s.push_str("<<<>>><a<<b>"),
            10 => {
                // Random raw bytes as UTF-8 text.
                for _ in 0..rng.below(40) {
                    let c = char::from_u32(0x20 + rng.below(0x4000)).unwrap_or('?');
                    s.push(c);
                }
            }
            _ => s.push_str("plain text 你好 \u{00a0}"),
        }
    }
    s
}

const PROPS: &[&str] = &[
    "width",
    "height",
    "margin",
    "padding",
    "color",
    "background",
    "display",
    "border-width",
    "box-sizing",
];
const VALS: &[&str] = &[
    "10px", "auto", "nan", "inf", "-inf", "1e40px", "1e40", "#f", "#zzz", "#12345",
    "rgb(1,2)", "rgb(999,-5,300)", "rgba(0,0,0,9)", "block", "border-box", "-99999px", "0",
    "  ", "/*x*/", "100%",
];
const SELS: &[&str] = &[
    "div", ".c", "#i", "*", "p.a.b#c", ":hover", "::before", "div > p", "a b c", "@media", "!!!",
    "(x)", "%", ",", "", "#", ".",
];

/// Build a hostile stylesheet: junk selectors, malformed values, unbalanced
/// braces and stray delimiters that used to be able to hang the parser.
fn adversarial_css(rng: &mut Rng) -> String {
    let mut s = String::new();
    let steps = 3 + rng.below(60);
    for _ in 0..steps {
        match rng.below(9) {
            0 => {
                let sel = SELS[rng.below(SELS.len() as u32) as usize];
                s.push_str(sel);
                s.push_str(" { ");
                let nd = rng.below(4);
                for _ in 0..nd {
                    let p = PROPS[rng.below(PROPS.len() as u32) as usize];
                    let v = VALS[rng.below(VALS.len() as u32) as usize];
                    s.push_str(p);
                    if rng.below(5) != 0 {
                        s.push(':');
                    }
                    s.push_str(v);
                    s.push(';');
                }
                s.push_str(" }");
            }
            1 => s.push('{'),
            2 => s.push('}'),
            3 => s.push_str("::::;;;;"),
            4 => s.push_str("/* unterminated comment"),
            5 => s.push_str(&":".repeat(rng.below(500) as usize)),
            6 => s.push_str(&"@media screen ".repeat(rng.below(200) as usize)),
            7 => {
                for _ in 0..rng.below(40) {
                    let c = char::from_u32(0x20 + rng.below(0x4000)).unwrap_or('?');
                    s.push(c);
                }
            }
            _ => s.push_str("color:red"),
        }
    }
    s
}
