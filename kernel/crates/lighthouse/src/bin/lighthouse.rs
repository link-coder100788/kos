//! The Lighthouse command line tool.
//!
//! Usage:
//!
//! ```text
//! lighthouse <file.html> [--css <file.css>]   Render a document, print every stage.
//! lighthouse demo                              Render a built in sample document.
//! lighthouse --help                            Show this help.
//! ```
//!
//! For an input document the tool prints, in order, the parsed DOM tree, the
//! computed styles, the layout box tree and an ASCII raster of the paint stage.
//! CSS is taken from any `<style>` elements in the document and from a companion
//! stylesheet passed with `--css`.

#![warn(clippy::pedantic)]
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

use lighthouse::css;
use lighthouse::layout::{layout_tree, Dimensions, Rect};
use lighthouse::paint::{build_display_list, Canvas};
use lighthouse::style::style_tree;
use lighthouse::{html, style};
use std::process::ExitCode;

const DEMO_HTML: &str = r#"<html>
  <body>
    <style>
      body { display: block; padding: 10px; background: #f6f7f9; }
      .card { display: block; width: 360px; padding: 16px; margin: 12px;
              background: #ffffff; border-width: 2px; border-color: #d0d7de; }
      h1 { display: block; margin: 4px; font-size: 28px; color: #1f2328; }
      p  { display: block; margin: 6px; color: #57606a; }
      .tag { display: inline; color: #0969da; }
    </style>
    <div class="card">
      <h1>Lighthouse</h1>
      <p>A dependency free rendering engine you can inspect stage by stage.</p>
      <p>Built by a <span class="tag">renderer</span> for humans and agents.</p>
    </div>
  </body>
</html>"#;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        print_usage();
        return ExitCode::SUCCESS;
    }

    let (html_src, css_extra_path) = if args[0] == "demo" {
        (DEMO_HTML.to_string(), None)
    } else {
        let path = &args[0];
        let src = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("lighthouse: cannot read {path}: {e}");
                return ExitCode::FAILURE;
            }
        };
        let mut css_path = None;
        let mut i = 1;
        while i < args.len() {
            if args[i] == "--css" && i + 1 < args.len() {
                css_path = Some(args[i + 1].clone());
                i += 2;
            } else {
                i += 1;
            }
        }
        (src, css_path)
    };

    let dom = html::parse(&html_src);

    // `<style>` contents come in through `style::document_stylesheet`.
    let mut css_src = String::new();
    if let Some(path) = css_extra_path {
        match std::fs::read_to_string(&path) {
            Ok(s) => css_src.push_str(&s),
            Err(e) => {
                eprintln!("lighthouse: cannot read {path}: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    let mut stylesheet = style::document_stylesheet(&dom);
    stylesheet.rules.extend(css::parse(&css_src).rules);

    let styled = style_tree(&dom, &stylesheet);

    let viewport = Dimensions {
        content: Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 0.0,
        },
        ..Default::default()
    };
    let layout = layout_tree(&styled, viewport);

    println!("== DOM ==");
    print!("{}", dom.pretty());

    println!("\n== COMPUTED STYLES ==");
    print!("{}", style::pretty(&styled));

    println!("\n== LAYOUT ==");
    print!("{}", lighthouse::layout::pretty(&layout));

    println!("\n== PAINT (ascii raster) ==");
    let height = (layout.dimensions.content.height.ceil() as usize + 20).min(1200);
    let mut canvas = Canvas::new(800, height.max(40));
    let display_list = build_display_list(&layout);
    canvas.paint(&display_list);
    print!("{}", canvas.to_ascii(72, (height / 18).clamp(6, 40)));

    ExitCode::SUCCESS
}

fn print_usage() {
    println!("Lighthouse: a dependency free browser rendering engine");
    println!();
    println!("USAGE:");
    println!("  lighthouse <file.html> [--css <file.css>]  Render a document, print all stages");
    println!("  lighthouse demo                            Render a built in sample");
    println!("  lighthouse --help                          Show this help");
    println!();
    println!("Stages printed: DOM, computed styles, layout box tree, ASCII paint.");
}
