# lighthouse (KOS port)

A small HTML/CSS rendering engine: HTML → DOM → styled tree → layout boxes →
display list. It's the planned engine for the KOS Browser's rendered view.

Ported from [pavanchow/lighthouse](https://github.com/pavanchow/lighthouse)
at commit `d3bcdc1` (MIT, © 2026 Pavan Nallamothu; see `LICENSE`).
`DESIGN.md` is upstream's description of the pipeline and supported subset.

## Changes from upstream

- `no_std` + `alloc` unless the `std` feature (on by default) is enabled.
  The kernel depends on it with `default-features = false`.
- `HashMap` → `BTreeMap` for computed styles (no hasher needed; iteration
  order is deterministic).
- `f32::round` → `libm::roundf` (via the crate-private `RoundF32` trait).
- `fuzz::ops_from_env` / `seed_from_env`, the `lighthouse` CLI and the
  `stress` binary need the `std` feature.
- `.cargo/config.toml` builds for the host, overriding the kernel's
  bare-metal target, so tests and the CLI run on your machine.

### Rendering fixes for real pages

Tested in `tests/kos.rs`.

- **Word wrapping.** Inline text is laid out word by word and wraps at the
  block's width. Each text box keeps one `TextFragment` per line, and paint
  emits one `Text` command per fragment. `<br>` and `white-space: pre` break
  lines. `layout_tree_with` takes `TextMetrics`, so a renderer with a real
  fixed-width font can make layout match what it draws.
- **Raw text elements.** `<script>` and `<style>` contents (and `<title>`,
  `<textarea>`) are text up to the end tag, so `a<b` in JavaScript no longer
  creates elements.
- **Default styles.** `style::USER_AGENT_CSS` gives headings sizes, lists
  indents, links color and underline, and so on. `style::document_stylesheet`
  combines it with the page's `<style>` blocks.
- **CSS outside the subset is skipped, not misapplied.** At-rules are dropped
  whole, and selectors with combinators, pseudo-classes or attribute tests
  are dropped. `!important` is ignored. `em`, `rem` and `pt` become px, and the
  `margin`/`padding`/`border-width` shorthands expand into their four sides.
- **More of the cascade.** `style="..."` attributes apply, the `hidden`
  attribute hides, `flex`/`grid`/`table`/... lay out as blocks, and
  `font-weight` and `white-space` inherit.
- **Replaced elements.** `LayoutOptions::replaced` gives the size of
  elements the renderer draws itself (images, form fields). Each becomes one
  box of that size on its line, or a block scaled down to fit, and its
  children are dropped. Horizontal margin, border and padding on inline
  elements take up space.
- **Nesting cap.** `html::parse_with_max_depth` sets how deep elements may
  nest. KOS uses 48 to bound how much stack rendering needs.
- **Paint** passes a `TextStyle` (font size, bold, underline) with text.
- More named entities (`&copy;`, `&mdash;`, `&raquo;`, ...).

## Use

From this directory:

```sh
cargo test                        # upstream's unit tests and gates
cargo run -- demo                 # print every stage for a sample page
cargo run -- page.html            # ... or for your own file
LIGHTHOUSE_FUZZ_OPS=3000 LIGHTHOUSE_ADV_OPS=3000 cargo run --release --bin stress
```
