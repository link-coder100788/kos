//! Painting: turn a layout tree into a display list, then rasterize.
//!
//! The [`DisplayList`] is a flat, ordered list of [`DisplayCommand`]s (solid
//! rectangles, border edges and text). Producing a display list separates
//! "what to draw" from "how to draw it", which is how real engines feed a GPU
//! or a software rasterizer.
//!
//! Lighthouse ships a headless raster [`Canvas`] plus an ASCII renderer so that
//! output can be inspected and tested without a windowing system.

use crate::prelude::*;
use crate::css::{Color, Value};
use crate::layout::{LayoutBox, Rect};
use crate::style::StyledNode;

/// One drawing instruction.
#[derive(Debug, Clone, PartialEq)]
pub enum DisplayCommand {
    /// Fill a rectangle with a solid color.
    SolidColor(Color, Rect),
    /// Draw text at a rectangle in a color.
    Text(String, Color, Rect, TextStyle),
}

/// How text should look, beyond its color.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextStyle {
    /// Font size in px.
    pub font_size: f32,
    /// `font-weight: bold` (or 600 and up).
    pub bold: bool,
    /// `text-decoration: underline`, here or on an ancestor.
    pub underline: bool,
}

/// An ordered list of drawing instructions.
pub type DisplayList = Vec<DisplayCommand>;

/// Build a display list from a laid out box tree.
pub fn build_display_list(root: &LayoutBox) -> DisplayList {
    let mut list = Vec::new();
    render_layout_box(&mut list, root, false);
    list
}

/// `underline` is inherited down the tree here because CSS draws an
/// element's text decoration across all its descendants' text.
fn render_layout_box(list: &mut DisplayList, layout_box: &LayoutBox, underline: bool) {
    let underline = underline
        || styled(layout_box).is_some_and(|s| {
            matches!(s.value("text-decoration").or_else(|| s.value("text-decoration-line")),
                Some(Value::Keyword(k)) if k.contains("underline"))
        });
    render_background(list, layout_box);
    render_borders(list, layout_box);
    render_text(list, layout_box, underline);
    for child in &layout_box.children {
        render_layout_box(list, child, underline);
    }
}

fn get_color(layout_box: &LayoutBox, name: &str) -> Option<Color> {
    styled(layout_box).and_then(|s| match s.value(name) {
        Some(Value::ColorValue(c)) => Some(c),
        _ => None,
    })
}

fn styled<'a>(layout_box: &'a LayoutBox) -> Option<&'a StyledNode<'a>> {
    match layout_box.box_type {
        crate::layout::BoxType::BlockNode(n) | crate::layout::BoxType::InlineNode(n) => Some(n),
        crate::layout::BoxType::AnonymousBlock => None,
    }
}

fn render_background(list: &mut DisplayList, layout_box: &LayoutBox) {
    if let Some(color) = get_color(layout_box, "background")
        .or_else(|| get_color(layout_box, "background-color"))
    {
        if color.a > 0 {
            list.push(DisplayCommand::SolidColor(color, layout_box.dimensions.padding_box()));
        }
    }
}

fn render_borders(list: &mut DisplayList, layout_box: &LayoutBox) {
    let Some(color) =
        get_color(layout_box, "border-color").or_else(|| get_color(layout_box, "border"))
    else {
        return;
    };
    let d = &layout_box.dimensions;
    let border_box = d.border_box();

    // Left.
    list.push(DisplayCommand::SolidColor(
        color,
        Rect {
            x: border_box.x,
            y: border_box.y,
            width: d.border.left,
            height: border_box.height,
        },
    ));
    // Right.
    list.push(DisplayCommand::SolidColor(
        color,
        Rect {
            x: border_box.x + border_box.width - d.border.right,
            y: border_box.y,
            width: d.border.right,
            height: border_box.height,
        },
    ));
    // Top.
    list.push(DisplayCommand::SolidColor(
        color,
        Rect {
            x: border_box.x,
            y: border_box.y,
            width: border_box.width,
            height: d.border.top,
        },
    ));
    // Bottom.
    list.push(DisplayCommand::SolidColor(
        color,
        Rect {
            x: border_box.x,
            y: border_box.y + border_box.height - d.border.bottom,
            width: border_box.width,
            height: d.border.bottom,
        },
    ));
}

fn render_text(list: &mut DisplayList, layout_box: &LayoutBox, underline: bool) {
    let Some(node) = styled(layout_box) else {
        return;
    };
    if let crate::dom::NodeType::Text(data) = &node.node.node_type {
        let trimmed = data.trim();
        if trimmed.is_empty() {
            return;
        }
        let color = get_color(layout_box, "color").unwrap_or(Color {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        });
        let style = TextStyle {
            font_size: match node.value("font-size") {
                Some(v) if v.to_px() > 0.0 => v.to_px(),
                _ => 16.0,
            },
            bold: match node.value("font-weight") {
                Some(Value::Keyword(k)) => k == "bold" || k == "bolder",
                Some(Value::Number(n)) => n >= 600.0,
                _ => false,
            },
            underline,
        };
        if layout_box.fragments.is_empty() {
            list.push(DisplayCommand::Text(trimmed.to_string(), color, layout_box.dimensions.content, style));
        }
        for fragment in &layout_box.fragments {
            list.push(DisplayCommand::Text(fragment.text.clone(), color, fragment.rect, style));
        }
    }
}

/// A software raster surface of RGBA pixels.
pub struct Canvas {
    /// Row major pixel buffer.
    pub pixels: Vec<Color>,
    /// Width in pixels.
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
}

impl Canvas {
    /// Create a white canvas of the given size.
    pub fn new(width: usize, height: usize) -> Canvas {
        let white = Color {
            r: 255,
            g: 255,
            b: 255,
            a: 255,
        };
        Canvas {
            pixels: vec![white; width * height],
            width,
            height,
        }
    }

    /// Paint a display list onto the canvas.
    pub fn paint(&mut self, list: &DisplayList) {
        for cmd in list {
            self.paint_command(cmd);
        }
    }

    fn paint_command(&mut self, cmd: &DisplayCommand) {
        match cmd {
            DisplayCommand::SolidColor(color, rect) => self.fill(*color, *rect),
            DisplayCommand::Text(_, color, rect, _) => {
                // Text is approximated as a faint filled band so it shows up in
                // the raster without a real glyph rasterizer.
                let faint = Color {
                    r: color.r,
                    g: color.g,
                    b: color.b,
                    a: 160,
                };
                self.fill(faint, *rect);
            }
        }
    }

    fn fill(&mut self, color: Color, rect: Rect) {
        let x0 = rect.x.max(0.0) as usize;
        let y0 = rect.y.max(0.0) as usize;
        let x1 = ((rect.x + rect.width).max(0.0) as usize).min(self.width);
        let y1 = ((rect.y + rect.height).max(0.0) as usize).min(self.height);
        for y in y0..y1 {
            for x in x0..x1 {
                let dst = self.pixels[y * self.width + x];
                self.pixels[y * self.width + x] = blend(dst, color);
            }
        }
    }

    /// Render the canvas as ASCII art by sampling into a character grid.
    ///
    /// Each output cell samples the average luminance of the pixels it covers
    /// and maps it to a character ramp, so darker painted regions read as denser
    /// characters. This is the headless verification surface.
    pub fn to_ascii(&self, cols: usize, rows: usize) -> String {
        let ramp = [' ', '.', ':', '-', '=', '+', '*', '#', '%', '@'];
        let mut out = String::with_capacity((cols + 1) * rows);
        for row in 0..rows {
            for col in 0..cols {
                let px0 = col * self.width / cols;
                let px1 = ((col + 1) * self.width / cols).max(px0 + 1).min(self.width);
                let py0 = row * self.height / rows;
                let py1 = ((row + 1) * self.height / rows).max(py0 + 1).min(self.height);
                let mut sum = 0.0f32;
                let mut count = 0.0f32;
                for y in py0..py1 {
                    for x in px0..px1 {
                        let c = self.pixels[y * self.width + x];
                        sum += luminance(c);
                        count += 1.0;
                    }
                }
                let lum = if count > 0.0 { sum / count } else { 1.0 };
                let idx = ((1.0 - lum) * (ramp.len() - 1) as f32).round_f() as usize;
                out.push(ramp[idx.min(ramp.len() - 1)]);
            }
            out.push('\n');
        }
        out
    }
}

fn luminance(c: Color) -> f32 {
    let a = c.a as f32 / 255.0;
    let base = (0.299 * c.r as f32 + 0.587 * c.g as f32 + 0.114 * c.b as f32) / 255.0;
    // Composite against white so transparent stays bright.
    base * a + (1.0 - a)
}

fn blend(dst: Color, src: Color) -> Color {
    let sa = src.a as f32 / 255.0;
    let mix = |d: u8, s: u8| ((s as f32 * sa) + (d as f32 * (1.0 - sa))).round_f() as u8;
    Color {
        r: mix(dst.r, src.r),
        g: mix(dst.g, src.g),
        b: mix(dst.b, src.b),
        a: 255,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_sets_pixels_in_rect() {
        let mut canvas = Canvas::new(10, 10);
        let red = Color { r: 255, g: 0, b: 0, a: 255 };
        canvas.fill(
            red,
            Rect {
                x: 2.0,
                y: 2.0,
                width: 3.0,
                height: 3.0,
            },
        );
        assert_eq!(canvas.pixels[2 * 10 + 2], red);
        // Outside the rect stays white.
        assert_eq!(canvas.pixels[0], Color { r: 255, g: 255, b: 255, a: 255 });
    }

    #[test]
    fn ascii_output_has_expected_shape() {
        let canvas = Canvas::new(20, 20);
        let ascii = canvas.to_ascii(8, 4);
        let lines: Vec<&str> = ascii.lines().collect();
        assert_eq!(lines.len(), 4);
        assert!(lines.iter().all(|l| l.chars().count() == 8));
    }

    #[test]
    fn display_list_includes_background_and_border() {
        use crate::layout::{layout_tree, Dimensions, Rect as LRect};
        use crate::style::style_tree;
        use crate::{css, html};
        let dom = html::parse("<div class=box>hi</div>");
        let sheet = css::parse(
            "div { display: block; } .box { background: #ff0000; border-width: 2px; border-color: #000000; height: 20px; }",
        );
        let styled = style_tree(&dom, &sheet);
        let layout = layout_tree(
            &styled,
            Dimensions {
                content: LRect {
                    x: 0.0,
                    y: 0.0,
                    width: 100.0,
                    height: 0.0,
                },
                ..Default::default()
            },
        );
        let list = build_display_list(&layout);
        let solids = list
            .iter()
            .filter(|c| matches!(c, DisplayCommand::SolidColor(_, _)))
            .count();
        // One background plus four border edges.
        assert!(solids >= 5, "expected background and borders, got {solids} solids");
    }
}
