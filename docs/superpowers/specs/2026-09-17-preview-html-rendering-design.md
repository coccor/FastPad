# FastPad Preview HTML Rendering Design

Status: Approved design (pending written-spec review)  
Date: 17 September 2026  
Amends: `2026-09-16-markdown-preview-design.md` §3 "Out of scope (v1)" (HTML rendering) and §7.6 (images)

## 1. Purpose

GitHub renders a sanitized subset of HTML inside Markdown, and most project READMEs depend on it:
centred headers, sized and linked images, badge rows, collapsible `<details>` sections, `<br>` and
`<kbd>` inside tables. FastPad's preview shows all of that as literal grey text today, and cannot
draw SVG images at all. Opening FastPad's own README in FastPad shows the problem on the first line.

Goal: render everything GitHub's HTML allowlist supports, locally and offline, with the preview's
existing guarantees intact:

> Nothing not required for the first editable frame may block the first editable frame.

and typing never waits on preview work.

## 2. Decisions

| Topic | Decision |
|---|---|
| Fidelity target | GitHub's HTML sanitizer allowlist (tags and attributes), delivered in three phases (§3) |
| Architecture | HTML tag stack inside the existing `model.rs` builder; one block tree and one layout pipeline for Markdown and HTML |
| HTML parsing | Hand-written, error-tolerant tokenizer in `src/preview/html.rs`; no new crate |
| Remote images | Never fetched. Placeholders sized from attributes or showing the alt text |
| SVG | Rasterized off the UI thread with Direct2D's SVG support (`ID2D1DeviceContext5`), producing the same BGRA pixels as WIC decodes |
| Inline images | DirectWrite inline objects via a hand-written COM vtable, like `window/accessibility.rs` |
| `<details>` | Interactive, collapsed unless `open`, keyboard and MSAA accessible |
| Named entities | HTML4 set (~250 names) plus numeric references |

Rejected alternatives:

- **Separate HTML renderer** (own tree and layout for HTML regions): duplicates text layout, links,
  accessibility, and scroll mapping, and would drift from the Markdown renderer.
- **Rewrite HTML to Markdown before parsing**: cannot express alignment, image sizes, `<details>`,
  or sub/superscript.
- **HTML parser crate** (`html5ever` and similar): large dependency closure, rejected by the
  dependency audit, and full HTML5 tree construction is far beyond the sanitized subset.
- **Loading remote images, opt-in or always**: breaks the no-network promise; declined by the owner.

## 3. Phases

One spec, three implementation plans. Each phase ships on its own.

| Phase | Scope |
|---|---|
| **1. README essentials** (this spec, detailed) | Tokenizer and sanitizer; `div`, `p`, `h1`–`h6` with `align`; inline `b strong i em code tt kbd samp var s strike del ins sub sup mark small span abbr bdo cite dfn time q a br wbr`; `img` inline and block with `width`/`height`; `picture`/`source` by theme; `hr`; `details`/`summary`; SVG images |
| **2. HTML block structure** (sketched, §11) | `ul ol li`, `blockquote`, `pre`, `dl dt dd`, `figure figcaption`, `ruby rt rp` |
| **3. HTML tables** (sketched, §11) | `table caption thead tbody tfoot tr th td` with `colspan`, `rowspan`, `align` |

Until phases 2 and 3 land, their tags follow the sanitizer's unknown-tag rule (§4.2): the tag is
dropped and its text content kept, so content is readable, never shown as raw markup.

## 4. Model (pure Rust, no Win32)

### 4.1 Tokenizer: `src/preview/html.rs` (new)

`tokenize(html: &str) -> Vec<Token>` never fails.

```text
Token::Start { name, attrs: Vec<(name, value)>, self_closing }
Token::End { name }
Token::Text(String)      // entities decoded
Token::Comment           // content discarded
```

- Tag and attribute names are ASCII-lowercased. Attribute values may be double-quoted,
  single-quoted, or unquoted; a name without a value has an empty value.
- A `<` that does not begin a valid tag (`<` followed by a letter, `/` and a letter, or `!--`) is
  text. An unterminated tag at the end of input is text.
- An unterminated comment runs to the end of input.
- Entities in text and attribute values: named HTML4 entities (the 252 names of the HTML 4.01
  entity sets, including `&nbsp;`, `&copy;`, `&mdash;`), decimal `&#169;`, hexadecimal `&#xA9;`.
  Invalid or unknown references stay literal. Code points 0, surrogates, and values above
  `U+10FFFF` decode to `U+FFFD`.

### 4.2 Sanitizer

Applied to the token stream, matching GitHub's behaviour:

- **Allowlisted tags** (phase 1 set in §3, plus phase 2 and 3 names once those phases land) are kept.
- **Removed with their content**: `script`, `style`, `iframe`, `object`, `embed`, `template`,
  `noscript`, `textarea`, `select`, `button`, `form`, `input`, `svg`, `math` (inline SVG markup in
  HTML is not rendered by GitHub; SVG *files* are, §7).
- **Any other tag** is dropped; its text content is kept.
- **Attributes** kept only from: `align`, `alt`, `width`, `height`, `src`, `srcset`, `media`,
  `href`, `title`, `open`, `name`, `id`. Everything else (including `style`, `class`, event
  handlers) is removed.
- `href` and `src` values are classified by the existing `links.rs` rules; `javascript:` and other
  unrecognised schemes become inert.
- **Comments** produce nothing.

### 4.3 Builder integration

`Event::Html` (HTML block chunks) and `Event::InlineHtml` both feed sanitized tokens to the existing
`Builder` instead of being stored as text. `BlockKind::Html` and `Frame::Html` are removed.

- **Block tags** (`div`, `p`, `h1`–`h6`, `details`, `summary`, `picture`, `hr`) push
  frames like `Tag::BlockQuote` does. Markdown blocks that arrive before the matching end tag
  become the frame's children, so this works across pulldown-cmark's separate HTML and Markdown
  blocks:

  ```html
  <div align="center">

  # FastPad

  </div>
  ```

- **Loose text** (text tokens, inline tags, or images) directly inside a container frame opens an
  implicit paragraph, flushed when a block child starts or the container ends, the same mechanism
  as tight list items (`flush_loose_item_text`).
- **Inline tags** open and close styles on the current inline target (§4.4).
- **Unclosed tags** close when their enclosing Markdown container (list item, quote, table cell)
  ends, or at the end of the document. **Stray end tags** with no matching open frame or style are
  ignored. An end tag closes the nearest matching open element and everything opened after it.
- **Nesting rules**: `p` cannot contain block tags; a block start tag inside an open `p` closes it
  first (HTML's implied `</p>`). Headings inside headings close the outer heading.
- A block frame closed inside an inline context (for example `<div>` inside a table cell) lays out
  its content inline; table cells only hold rich text.

### 4.4 Model types

```rust
pub enum BlockKind {
    Heading { level: u8, align: TextAlign, text: RichText },
    Paragraph { align: TextAlign, text: RichText },
    List { start: Option<u64>, items: Vec<ListItem> },
    Quote(Vec<BlockKind>),
    Code { language: String, text: String },
    Table { alignments: Vec<CellAlign>, head: Vec<RichText>, rows: Vec<Vec<RichText>> },
    Rule,
    Container { align: TextAlign, children: Vec<BlockKind> },
    Details { open: bool, summary: RichText, children: Vec<BlockKind> },
}

pub enum TextAlign { Inherit, Left, Center, Right, Justify }

pub struct RichText {
    pub text: String,
    pub utf16_len: u32,
    pub spans: Vec<Span>,
    pub images: Vec<InlineImage>,
}

pub struct InlineImage {
    /// UTF-16 offset of the U+FFFC placeholder the image occupies.
    pub position: u32,
    pub image: ImageRef,
}

pub struct ImageRef {
    pub dest: String,
    pub alt: String,
    pub title: String,
    pub width: Option<Length>,
    pub height: Option<Length>,
    /// `<picture>` candidates in document order; empty for a plain image.
    pub sources: Vec<ImageSource>,
}

pub struct ImageSource { pub srcset: String, pub scheme: Option<ColorScheme> }
pub enum ColorScheme { Light, Dark }
pub enum Length { Pixels(f32), Percent(f32) }

pub enum InlineStyle {
    Strong, Emphasis, Strikethrough, Code, Link(String),
    Underline, Subscript, Superscript, Mark, Small, Keyboard,
}
```

- `BlockKind::Images` and `InlineStyle::ImageAlt` are removed: Markdown images (`![alt](src)`) and
  HTML `<img>` both become `InlineImage`s in rich text. A paragraph of only images is an ordinary
  paragraph whose images flow inline, as on GitHub.
- `Paragraph` gains `align`. Markdown headings and paragraphs get `TextAlign::Inherit`.
- `<p align>` becomes a `Paragraph` with that alignment; `<div align>` becomes `Container`;
  `<h1 align>` becomes `Heading` with alignment. `<center>` is not on GitHub's allowlist, so it
  follows the unknown-tag rule (text kept, no centring), matching GitHub.
- `<img>` without `src` is dropped. `align` on `<img>` (a float on GitHub) is ignored: floats are
  out of scope, and the image stays inline. `<source>` outside `<picture>` is dropped.
- `h7` and `h8`, which GitHub allows, render as `h6`.
- `<hr>` becomes `Rule`.
- Attribute parsing: `width="96"`, `width="96px"` → `Pixels(96.0)`; `width="50%"` → `Percent(50.0)`;
  anything else → `None`. `align` accepts `left`, `center`, `middle` (as center), `right`,
  `justify`, case-insensitively.
- `<picture>`: each `<source>` with `media` equal (ignoring whitespace and case) to
  `(prefers-color-scheme: dark)` or `(prefers-color-scheme: light)` records that scheme; other
  `media` values record `None`. The first URL of `srcset` (before any whitespace or comma) is the
  candidate. The inner `<img>` provides `dest`, `alt`, size, and title.
- `<details>`: `open` records the attribute. The first `<summary>` child's rich text is the
  summary; without one the summary text is "Details" (GitHub's browser default). Blocks after the
  summary are children.
- `<a name>` and `id` attributes become heading-slug anchors in phase 1 only when on a heading;
  otherwise they are ignored.
- `<q>` wraps its text in curly quotes. `<abbr>`, `<bdo>`, `<time>`, `<cite>`, `<dfn>`, `<var>`,
  `<span>` keep their text; `<cite>`, `<dfn>`, `<var>` apply `Emphasis`. `<bdo dir>` direction is
  ignored.
- `<br>` inserts `\n` (hard break); `<wbr>` inserts `U+200B`.

### 4.5 Incremental reparse

A container, `Details`, or HTML heading is one top-level `Block`, so the existing slice rule (reparse
the dirty blocks, accept when the sentinel blocks parse back identically) needs no new mechanism.
An edit anywhere inside a `<div>` reparses the whole `<div>`.

Known cost: an unclosed block tag near the top makes the rest of the document one block, and every
edit then reparses to the end. This matches GitHub's semantics and is accepted; it is recorded by a
benchmark (§9.3), not prevented.

The seeded property test (full parse equals incremental parse after every edit) is the authority for
any additional fallback rule; a divergence adds a fallback, never a rendering special case.

## 5. Layout and rendering

### 5.1 Inline images

Each `InlineImage` is attached to its `U+FFFC` placeholder with `IDWriteTextLayout::SetInlineObject`.
The inline object is a hand-written COM object (`src/preview/inline_object.rs`, new) implementing
`IDWriteInlineObject`:

- `GetMetrics` reports the image's layout size (below) with the baseline at the bottom edge.
- `GetOverhangMetrics` reports zero; `GetBreakConditions` reports `DWRITE_BREAK_CONDITION_NEUTRAL`
  on both sides so images wrap like words.
- `Draw` does nothing. After layout, `HitTestTextPosition` on each placeholder gives the image's
  rectangle, which becomes an `ImageSlot` drawn by the existing `DrawOp::Image` path.

Layout size, in DIPs, for available width `W`:

1. Explicit `width` and `height`: use them (`Percent` resolves against `W`).
2. One of them: the other follows the natural aspect ratio once known; until then the placeholder
   height is the line height.
3. Neither: the natural size once decoded.
4. Always scaled down uniformly to at most `W`, never up.

Placeholders (pending, missing, remote, or failed images):

- With an explicit size: a bordered box of that size with the alt text inside, as today.
- Without: a compact chip, one line tall, sized to the alt text plus padding (`"image"` when the
  alt text is empty). A row of remote badges reads as a row of labelled chips.

When a pending image decodes and its size changes, the containing block's layout is discarded and
rebuilt, as for today's image blocks.

### 5.2 Alignment

`Style` (passed down through `layout_kind`) gains `align: TextAlign`. A `Container`'s alignment
overrides `Inherit` for all its descendants; a paragraph's or heading's own alignment overrides the
inherited one. Paragraph and heading layouts call `SetTextAlignment` (`Justify` uses
`DWRITE_TEXT_ALIGNMENT_JUSTIFIED`). As in GitHub's CSS, alignment moves inline content only; code
blocks, tables, and list markers are not moved.

### 5.3 Inline styles

| Style | DirectWrite |
|---|---|
| `Underline` | `SetUnderline` |
| `Small` | `SetFontSize(size × 0.875)` |
| `Subscript`, `Superscript` | `SetFontSize(size × 0.75)` plus an `IDWriteTypography` with the `subs` or `sups` feature. Glyphs the font lacks render smaller on the baseline (DirectWrite has no baseline shift); this is a documented approximation |
| `Mark` | Rounded fill behind the range's hit-test rectangles, role `Mark` |
| `Keyboard` | Code font at `CODE_SCALE`, rounded fill `CodeBackground`, plus a 1 DIP rounded stroke, role `KbdBorder` |

### 5.4 Blocks

- `Container`: lays out children with the normal block margins; draws nothing itself.
- `Details`: §6.
- `Heading` and `Paragraph`: unchanged apart from alignment and inline images.

### 5.5 `<picture>` and themes

When resolving an `ImageRef` for layout, the first `ImageSource` whose `scheme` matches the preview
theme's darkness (`PreviewColors::is_dark`, new) wins; otherwise the first source with no scheme;
otherwise `dest`. The view already relays out every block on a theme change, which re-resolves
pictures.

### 5.6 Colours

`PreviewColors` gains `mark` and `kbd_border` for all eight themes. Light and dark follow GitHub's
Primer values; the Catppuccin flavours use their palette (`yellow` at reduced alpha for `mark`,
`surface1` for `kbd_border`).

## 6. `<details>` interaction and accessibility

### 6.1 Rendering

A disclosure row: a triangle glyph (`▸` collapsed, `▾` open) in the body font, then the summary
rich text. When open, children are laid out below, indented 16 DIPs (scaled by `unit`).

### 6.2 State

`ViewState` holds `details_overrides: HashMap<DetailsKey, bool>` recording sections the user
toggled away from their `open` default.

`DetailsKey { summary: String, occurrence: u32 }`: the summary's plain text and its index among
details with the same summary text, counted in document order over the whole block tree. The key
survives edits elsewhere and edits to the section's body, resets when the summary text changes, and
the map is cleared by `replace_document`.

Toggling discards that top-level block's layout; the existing anchor-preserving height correction
keeps the content under the viewport stable.

### 6.3 Interactive targets

`LinkHit` becomes `Target { kind: TargetKind, text, rects, layout, range, scrolls, clip }` with
`TargetKind::Link(dest)` or `TargetKind::Disclosure(DetailsKey)`.

- Mouse: clicking a disclosure row toggles; hovering shows the hand cursor. A link inside a
  summary follows the link and does not toggle.
- Keyboard: Tab and Shift+Tab move through links and disclosures in document order; Enter and
  Space activate a focused disclosure; Enter follows a focused link as today.
- `block_has_link` becomes `block_has_target` and also answers for `Details`.

### 6.4 Navigation and scroll sync

- `#anchor` navigation to a heading inside a collapsed section opens every enclosing section first,
  then scrolls to the heading (Chrome and Edge behaviour).
- Heading slugs count headings inside collapsed sections, so duplicate `-1` suffixes match GitHub.
- Editor to preview: source lines inside a collapsed section map to the disclosure row.
- Preview to editor: with the disclosure row at the top, the editor scrolls to the `<details>` line.

### 6.5 MSAA

- Each disclosure is a child with `ROLE_SYSTEM_OUTLINEBUTTON`, name = summary plain text, state
  `STATE_SYSTEM_EXPANDED` or `STATE_SYSTEM_COLLAPSED` (plus focusable/focused), and default action
  "Expand" or "Collapse". Toggling raises `EVENT_OBJECT_STATECHANGE` for that child.
- A link whose text is only inline images is named by the images' alt text, then `title`, then the
  link destination.
- Non-link images add no children, as today.

## 7. SVG and image decoding

### 7.1 Dispatch

`decode_image(path, max_width)` dispatches on the extension (case-insensitive): `.svg` goes to
`decode_svg`; everything else to the existing WIC path. Both return `DecodedImage`, so the cache,
bitmap creation, and device-loss recovery are unchanged.

### 7.2 `decode_svg`

1. Read the file; above 8 MB, fail.
2. Natural size from the root `<svg>` element (read with the §4.1 tokenizer): `width`/`height` as
   plain numbers or `px`; otherwise the `viewBox` width and height; otherwise 300×150. Sizes above
   the existing 64-megapixel limit fail.
3. Compatibility rewrite: on `<use>` elements, `href=` becomes `xlink:href=`, and the root gains
   `xmlns:xlink="http://www.w3.org/1999/xlink"` when absent (Direct2D implements SVG 1.1 linking).
4. Target pixel size: the natural size scaled to at most `max_width` (or natural when 0), preserving
   aspect ratio.
5. Rasterize on the worker thread:
   - `D2D1CreateFactory(D2D1_FACTORY_TYPE_MULTI_THREADED)` through the existing lazy
     `LoadLibraryExW(LOAD_LIBRARY_SEARCH_SYSTEM32)` + `GetProcAddress` loading in `dwrite.rs`.
   - A WIC bitmap (`GUID_WICPixelFormat32bppPBGRA`) and `CreateWicBitmapRenderTarget`, cast to
     `ID2D1DeviceContext5`.
   - `SHCreateMemStream` (loaded lazily from `shlwapi.dll`) over the rewritten bytes, then
     `CreateSvgDocument(stream, viewport)`, `DrawSvgDocument`, and a copy of the WIC bitmap's pixels.
6. Any failure (no `ID2D1DeviceContext5` before Windows 10 1703, malformed SVG, size limits) returns
   an error, which the cache turns into the placeholder.

Direct2D renders shapes, paths, gradients, `<use>`, transforms, opacity, and clipping. It does not
render `<text>`, filters, masks, CSS `<style>` blocks, or animation; such SVGs render partially or
fail to the placeholder. Direct2D never fetches external resources.

### 7.3 Fallback if the WIC render target lacks SVG support

The first implementation task verifies step 5 with `assets/fastpad-icon.svg`. If
`ID2D1DeviceContext5` is not available from a WIC bitmap render target, SVGs rasterize on the UI
thread instead, into a compatible bitmap on the preview's own device context, the first time each
is drawn at a given size. This fallback is only used if the spike fails, and the spec is updated
when it is.

### 7.4 Other formats

Unchanged: PNG, JPEG, GIF (first frame), BMP, TIFF, ICO, and WebP or HEIC where the codec is
installed. Animated GIF playback is out of scope.

## 8. Error handling

- The tokenizer, sanitizer, and builder accept all input; there is no parse failure path.
- Image and SVG failures render the placeholder with alt text and raise no notice, as today.
- A failure creating an inline object for an image lays the image out as its alt-text chip without
  an inline object (plain text span), so text layout never fails because of an image.
- Details toggling cannot fail; a stale `DetailsKey` (summary edited) is ignored.

## 9. Testing and benchmarks

### 9.1 Unit tests without Win32

- `html.rs`: malformed markup (unclosed quotes, stray `<`, `<` inside text, uppercase names),
  attribute forms, void and self-closing tags, comments including unterminated, entities (named,
  decimal, hex, unknown, out of range).
- Sanitizer: removed-with-content tags, dropped unknown tags keep text, attribute allowlist,
  `javascript:` links inert.
- `model.rs`: one fixture per phase 1 tag; cross-block nesting (§4.3 example); unclosed and stray
  tags; implied `</p>`; `<details open>` and missing `<summary>`; `<picture>` scheme detection;
  `width` and `height` parsing; image-only paragraphs as inline images; Markdown images as inline
  images.
- `incremental.rs`: the seeded property test gains HTML fixture documents and edits inserting or
  deleting `<`, `>`, `</div>`, `<details>`, `<summary>`, `-->`, and blank lines inside containers.
- `links.rs`: slugs for headings inside collapsed details; accessible names of image-only links.

### 9.2 Win32 unit and integration tests

- `layout.rs`: centred paragraph text starts near the middle; three inline images lay out on one
  line when they fit and wrap when they do not; `width` attributes size images; percentage widths
  resolve against the content width; placeholders without a size are one line tall; a collapsed
  `Details` is shorter than the same block open; `Mark` and `Keyboard` produce their fills and
  strokes.
- `images.rs`: `assets/fastpad-icon.svg` decodes at 96 pixels wide with the header colour
  `#FFB020` at the header's position; a malformed SVG and an oversize SVG return errors; a later
  larger request decodes a larger image.
- New fixture `tests/fixtures/html-readme.md` mirroring the README's HTML: a centred `<div>` with
  an SVG `<img width height>`, a heading, a badge row of remote `<a><img></a>`, a
  `<details><summary>` wrapping a Markdown table with `<br>` and `<b>`, `<kbd>`, a comment, and a
  `<picture>` with light and dark sources.
- `tests/windows/markdown_preview.rs`:
  - The fixture renders with no literal HTML: a test hook reports zero text runs containing `<`
    followed by a tag name from the fixture.
  - The SVG image slot reaches the loaded state.
  - The details block starts collapsed; Tab focuses its disclosure, Enter expands it, and the
    document height grows; Enter again collapses it.
  - MSAA reports the disclosure as `ROLE_SYSTEM_OUTLINEBUTTON` with its state flipping between
    collapsed and expanded.
  - Import guard: the release `FastPad.exe` import table contains none of `d2d1.dll`, `dwrite.dll`,
    `windowscodecs.dll`, `shlwapi.dll`, and none are loaded at input readiness.

### 9.3 Benchmarks

| Measure | Target |
|---|---|
| Warm TTI and first paint launching with a `.md` file | No statistically material regression (existing `fastpad-bench compare`) |
| Preview open, 100 KB HTML-heavy fixture | < 50 ms p95 |
| One-paragraph update inside a `<div>` spanning a 1 MB document | Recorded, no target (§4.5) |
| SVG decode of `assets/fastpad-icon.svg` at 256 px | Recorded (worker thread) |
| Release `FastPad.exe` size | Recorded before and after; expected growth under 100 KB |

### 9.4 Run discipline

During implementation: compile through Clippy and run targeted tests only. The full
`cargo test -- --test-threads=1` suite and the benchmarks run at final review.
`%LocalAppData%\FastPad\fastpad.ini` is backed up and restored around every live-app run.

## 10. Documentation

- `2026-09-16-markdown-preview-design.md`: a note under §3 pointing to this spec for HTML rendering.
- `README.md`: the Markdown preview section states that GitHub's HTML subset and local SVG images
  render, and that remote images show placeholders.
- `benchmarks/README.md`: the §9.3 measures and results.

## 11. Later phases (sketch)

Each gets its own plan, amending this spec where the detail changes.

### Phase 2: HTML block structure

- `ul`, `ol` (`start`, `type` ignored), `li`: map to `BlockKind::List` with items built from HTML
  children; loose text in `li` forms implicit paragraphs.
- `blockquote` → `Quote`; `pre` → `Code` with entity-decoded text and `<code>` unwrapped.
- `dl`, `dt`, `dd` → new `DefinitionList { items: Vec<(RichText, Vec<BlockKind>)> }`, `dt` bold,
  `dd` indented.
- `figure` → `Container`; `figcaption` → a muted paragraph.
- `ruby`, `rt`, `rp`: base text followed by the annotation in parentheses at `Small` size (no
  above-text placement).

### Phase 3: HTML tables

- `table`, `caption`, `thead`, `tbody`, `tfoot`, `tr`, `th`, `td` → a new `HtmlTable` block with a
  cell grid resolved from `colspan` and `rowspan`, cells holding block children (not only rich text),
  `align` per cell and row, and `th` bold.
- Layout extends `push_table` column measurement to spanning cells (distribute the excess width of
  a spanning cell evenly across its columns) and row heights to spanning rows.
- Horizontal scrolling for wide tables reuses `DrawOp::Scrollable`.

## 12. Risks

| Risk | Mitigation |
|---|---|
| WIC bitmap render target cannot provide `ID2D1DeviceContext5` | Spike first (§7.3); UI-thread fallback defined |
| Unclosed `<div>` makes large documents reparse fully on every edit | Accepted, matches GitHub; benchmark records the cost |
| Hand-written `IDWriteInlineObject` lifetime bugs | Objects owned by the laid block and released with its layouts; unit tests create and drop many layouts |
| Incremental model diverges with HTML edits | Property test with HTML edits (§9.1) |
| Sub/superscript approximation looks off for letters | Documented; revisit with a baseline-shifting inline object if it matters |
| Binary size growth (entity table, new code) | HTML4 entity set only; size recorded (§9.3) |
| Direct2D SVG feature gaps (text, filters, CSS) | Documented; placeholder on failure |
