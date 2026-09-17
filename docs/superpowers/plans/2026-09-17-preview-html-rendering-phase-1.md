# Preview HTML Rendering (Phase 1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan in **one run** (the owner asked for a single grouped run: no per-task review cycles). Steps use checkbox (`- [ ]`) syntax for tracking. Work through Parts 1–8 in order, compile and run only the targeted tests each part names, commit at the end of each part, and do the one review and the full suite in Part 8.

**Goal:** The Markdown preview renders GitHub's sanitized HTML subset (phase 1: `div`/`p`/headings with `align`, inline formatting tags, sized inline images, `<picture>` by theme, `<details>`/`<summary>`) and local SVG images, offline, without costing startup or typing latency.

**Architecture:** A hand-written tokenizer and sanitizer (`html.rs`) feed HTML tokens into the existing `model.rs` builder, which grows HTML frames (`Container`, `Details`, HTML paragraphs and headings) inside the same block tree Markdown uses. Images become inline U+FFFC placeholders carried by `RichText` and are laid out as DirectWrite inline objects (`inline_object.rs`). SVG files rasterize on the image worker through Direct2D's SVG renderer into a WIC bitmap (`svg.rs`). `<details>` state lives in the view, keyed by `outline.rs`, with mouse, keyboard, anchor, and MSAA support.

**Tech Stack:** Rust 2024, `pulldown-cmark 0.13.4`, `windows 0.62.2` (Direct2D incl. `ID2D1DeviceContext5` SVG, DirectWrite inline objects and typography, WIC), `windows-sys 0.61.2`, `windows-numerics 0.3.1`. No new crates and no new `windows` features.

**Spec:** `docs/superpowers/specs/2026-09-17-preview-html-rendering-design.md` (amends `docs/superpowers/specs/2026-09-16-markdown-preview-design.md`)

## Global Constraints

- Nothing not required for the first editable frame may block the first editable frame; typing never waits on preview work.
- No new crates. `Cargo.toml` dependency pins and `windows` features stay exactly as they are (`tools/audit-dependencies.ps1` must still pass unchanged).
- `d2d1.dll`, `dwrite.dll`, and `windowscodecs.dll` load only through `LoadLibraryExW(LOAD_LIBRARY_SEARCH_SYSTEM32)` + `GetProcAddress` (or COM activation for WIC) after a preview opens; none may enter `FastPad.exe`'s import table. Never call the `windows` crate's `D2D1CreateFactory` / `DWriteCreateFactory` free functions.
- Nothing is fetched from the network. Remote images render as placeholders.
- Images: never upscaled, rejected above `MAX_IMAGE_PIXELS = 64_000_000`; SVG files above `MAX_SVG_BYTES = 8 * 1024 * 1024` fail to a placeholder.
- HTML allowlist (phase 1): `a abbr b bdo br cite code del details dfn div em h1–h8 hr i img ins kbd mark p picture q s samp small source span strike strong sub summary sup time tt var wbr`. Removed with content: `button form iframe math noscript object script select style svg template textarea`. Attributes kept: `align alt height href id media name open src srcset title width`.
- Parsing never fails; image, SVG, and details problems degrade to placeholders or plain text and raise no notice.
- The seeded incremental property test (full parse equals incremental parse) stays the authority: a divergence adds a fallback rule in `incremental.rs`, never a rendering special case.
- Run discipline: compile gate is `cargo clippy --all-targets --all-features -- -D warnings`; run only the tests each part names; the full `cargo test -- --test-threads=1` suite and the ignored benchmarks run once, in Part 8.
- Worktree test setup: before the first `cargo test`, copy the native DLLs into the worktree (`native\out\x64\Scintilla.dll` and `Lexilla.dll` from the main checkout if `native\out` is missing here). Window tests need `--test-threads=1`.
- Back up `%LocalAppData%\FastPad\fastpad.ini` before, and restore it after, every live-app run.
- Commit messages end with:
  ```
  Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01B2tLBsVChr2g7EffFRGXcn
  ```

## Clarifications to the spec made while planning

Implementers follow these; Part 8 records them in the spec.

1. **SVG memory stream.** `decode_svg` wraps the bytes in an `IWICStream` (`IWICImagingFactory::CreateStream` + `InitializeFromMemory`), which already lives in `windowscodecs.dll`, instead of `SHCreateMemStream`. No `shlwapi.dll` load, so the import guards keep their current three DLLs.
2. **`Length` holds whole numbers** (`Pixels(u32)`, `Percent(u32)`) so `BlockKind` keeps `Eq`. Fractions truncate, zero and unparsable values are ignored, and a percentage `height` is ignored (it has no containing height).
3. **`ImageSource { url, scheme }`** stores the first candidate URL of `srcset`, not the raw attribute.
4. **`BlockKind::Heading` gains `anchor: Option<String>`**, set from the heading's `id` or an `<a name>` / `<a id>` inside it.
5. **`<picture>` is inline state**, not a block frame: CommonMark never starts an HTML block for `<picture>` on a line with other content, so it usually arrives as inline HTML.
6. **HTML text whitespace** collapses to single spaces, a space never starts a line, and trailing spaces of an HTML-built text are trimmed. Markdown text is untouched.
7. **One preview block per CommonMark top-level block.** When several blocks finish inside one HTML block (`<p>a</p><p>b</p>`), they are wrapped in `Container { align: Inherit }`. HTML blocks are tokenized when the HTML block ends, so tags split across lines stay whole. An HTML block that produces nothing (a comment, a stray end tag) produces no preview block (previously a grey `Html` block).
8. **Sanitizer and `<picture>` state reset** whenever the builder emits a top-level block or an HTML block ends with no open frame, so no state crosses a top-level block boundary.
9. **`href`, `src`, and `srcset` values with any scheme other than `http`, `https`, or `mailto`** are removed by the sanitizer (`links::is_safe_url`); `<a>` without `href` is plain text and `<img>` without `src` is dropped.
10. **`outline.rs`** owns `DetailsKey`, per-block details keys, and heading anchors (including headings inside collapsed sections). `ViewState::anchors` becomes `ViewState::outline`.
11. **`TargetKind::Disclosure { key, expanded }`** carries the expanded state from layout, so snapshots need no lookup. MSAA activation of a disclosure reuses `WM_FASTPAD_PREVIEW_ACTIVATE` with `wparam = ACTIVATE_DISCLOSURE` (1) and a boxed `DetailsKey`.
12. **Scroll sync needs no new mapping.** A collapsed section is a short block, so the existing proportional line mapping already puts its source lines on the disclosure row, and the block top maps to the `<details>` line. A view test pins this.
13. **Summary rows are not aligned** by an enclosing `align`; the section's children are.
14. **Adjacent images** wrap at spaces between them (neutral break conditions); images with no space between them stay on one line.
15. **Colours.** Catppuccin `Flavor` gains `yellow`; `mark` is `blend(yellow, base, 64)`. High contrast uses the editor background for `mark` (no fill) and the editor foreground for `kbd_border`. GitHub dark `mark` is `rgb(69, 56, 25)`.
16. **Layout sizes.** Details children are indented 16 DIPs and start 8 DIPs below the summary row. Image chips pad the alt text by 6 DIPs; sized placeholder boxes pad it by 8 DIPs.
17. **Empty results are dropped:** an HTML `<p>` with only whitespace, a `<div>` with no children, and `<summary>` outside `<details>` (its text is kept).
18. **`LaidBlock::heading`** becomes `headings: Vec<(u32, f32)>` (document-order heading index within the block and its y), used by anchor scrolling.
19. **README.** This branch's README preview section is updated; draft PR #4 rewrites the README, so reconcile the sentence when both merge.
20. **No inline-object fallback path.** `inline_box` is a Rust allocation that cannot fail, and `SetInlineObject` fails only for a range the model never produces, so spec §8's "lay the image out as its alt-text chip without an inline object" has no trigger; an error there propagates like any other layout error.
21. **Test hooks.** "No literal HTML" is asserted on the parsed model of the fixture (every text in the block tree) rather than on DirectWrite runs, and the integration test reads disclosure state from the accessibility snapshot; the MSAA role and state mapping itself is covered by `accessible.rs` unit tests.

## File Structure

| Path | Status | Responsibility |
|---|---|---|
| `src/preview/html.rs` | Create | `Token`, `tokenize`, `decode_entities`, `attr`, `Sanitizer` |
| `src/preview/html_entities.rs` | Create | The 252 HTML 4.01 named character references, `lookup` |
| `src/preview/svg.rs` | Create | `decode_svg`, `natural_size`, `rewrite_for_direct2d` |
| `src/preview/inline_object.rs` | Create | Hand-written `IDWriteInlineObject` (`inline_box`) |
| `src/preview/outline.rs` | Create | `DetailsKey`, `Anchor`, `Outline`, `build`, `count_nested`, `details_open_attribute`, `has_details` |
| `src/preview/mod.rs` | Modify | Register the new modules |
| `src/preview/links.rs` | Modify | `is_safe_url` |
| `src/preview/dwrite.rs` | Modify | `load_system_library` and `create_d2d_factory` shared with the SVG worker |
| `src/preview/images.rs` | Modify | Dispatch `.svg` to `decode_svg` |
| `src/preview/model.rs` | Modify | Inline images, `TextAlign`, `Length`, `ImageSource`, `Container`, `Details`, new inline styles, HTML builder |
| `src/preview/incremental.rs` | Modify | HTML fixtures and edits in the property test |
| `src/preview/colors.rs` | Modify | `Mark`, `KbdBorder` roles; `is_dark` |
| `src/catppuccin.rs` | Modify | `Flavor::yellow` |
| `src/preview/layout.rs` | Modify | Inline image layout, alignment, new styles, `<picture>` choice, details rows, `Target` |
| `src/preview/render.rs` | Modify | Placeholder alt origin and clip, `RoundedStroke` |
| `src/preview/view.rs` | Modify | Outline, details overrides, target activation, anchors into sections, state-change events, stats |
| `src/preview/accessible.rs` | Modify | Disclosures as outline buttons |
| `tests/fixtures/html-readme.md` | Create | README-like HTML fixture |
| `tests/windows/markdown_preview.rs` | Modify | Fixture integration test and HTML benchmarks |
| `README.md`, `benchmarks/README.md`, spec | Modify | Documentation and recorded measurements |

---
## Part 1: HTML tokenizer, entities, and sanitizer

**Files:**
- Create: `src/preview/html_entities.rs`, `src/preview/html.rs`
- Modify: `src/preview/mod.rs`, `src/preview/links.rs`

**Interfaces:**
- Produces: `html::Token { Start { name, attrs, self_closing }, End { name }, Text(String), Comment }`; `html::tokenize(&str) -> Vec<Token>`; `html::decode_entities(&str) -> String`; `html::attr(&[(String, String)], &str) -> Option<&str>`; `html::Sanitizer` with `filter(&mut self, Token) -> Option<Token>`, `is_removing(&self) -> bool`, `reset(&mut self)`; `html_entities::lookup(&str) -> Option<char>`; `links::is_safe_url(&str) -> bool`.

- [ ] **Step 1: Register the modules**

In `src/preview/mod.rs`, add to the module list (keep it alphabetical):

```rust
pub mod html;
mod html_entities;
```

- [ ] **Step 2: Write the entity table**

Create `src/preview/html_entities.rs`:

```rust
//! The 252 named character references of HTML 4.01 (Latin-1, symbols, and special characters),
//! sorted by name in byte order for binary search.

pub const ENTITIES: [(&str, u32); 252] = [
    ("AElig", 198), ("Aacute", 193), ("Acirc", 194), ("Agrave", 192), ("Alpha", 913),
    ("Aring", 197), ("Atilde", 195), ("Auml", 196), ("Beta", 914), ("Ccedil", 199), ("Chi", 935),
    ("Dagger", 8225), ("Delta", 916), ("ETH", 208), ("Eacute", 201), ("Ecirc", 202),
    ("Egrave", 200), ("Epsilon", 917), ("Eta", 919), ("Euml", 203), ("Gamma", 915),
    ("Iacute", 205), ("Icirc", 206), ("Igrave", 204), ("Iota", 921), ("Iuml", 207), ("Kappa", 922),
    ("Lambda", 923), ("Mu", 924), ("Ntilde", 209), ("Nu", 925), ("OElig", 338), ("Oacute", 211),
    ("Ocirc", 212), ("Ograve", 210), ("Omega", 937), ("Omicron", 927), ("Oslash", 216),
    ("Otilde", 213), ("Ouml", 214), ("Phi", 934), ("Pi", 928), ("Prime", 8243), ("Psi", 936),
    ("Rho", 929), ("Scaron", 352), ("Sigma", 931), ("THORN", 222), ("Tau", 932), ("Theta", 920),
    ("Uacute", 218), ("Ucirc", 219), ("Ugrave", 217), ("Upsilon", 933), ("Uuml", 220), ("Xi", 926),
    ("Yacute", 221), ("Yuml", 376), ("Zeta", 918), ("aacute", 225), ("acirc", 226), ("acute", 180),
    ("aelig", 230), ("agrave", 224), ("alefsym", 8501), ("alpha", 945), ("amp", 38), ("and", 8743),
    ("ang", 8736), ("aring", 229), ("asymp", 8776), ("atilde", 227), ("auml", 228),
    ("bdquo", 8222), ("beta", 946), ("brvbar", 166), ("bull", 8226), ("cap", 8745),
    ("ccedil", 231), ("cedil", 184), ("cent", 162), ("chi", 967), ("circ", 710), ("clubs", 9827),
    ("cong", 8773), ("copy", 169), ("crarr", 8629), ("cup", 8746), ("curren", 164), ("dArr", 8659),
    ("dagger", 8224), ("darr", 8595), ("deg", 176), ("delta", 948), ("diams", 9830),
    ("divide", 247), ("eacute", 233), ("ecirc", 234), ("egrave", 232), ("empty", 8709),
    ("emsp", 8195), ("ensp", 8194), ("epsilon", 949), ("equiv", 8801), ("eta", 951), ("eth", 240),
    ("euml", 235), ("euro", 8364), ("exist", 8707), ("fnof", 402), ("forall", 8704),
    ("frac12", 189), ("frac14", 188), ("frac34", 190), ("frasl", 8260), ("gamma", 947),
    ("ge", 8805), ("gt", 62), ("hArr", 8660), ("harr", 8596), ("hearts", 9829), ("hellip", 8230),
    ("iacute", 237), ("icirc", 238), ("iexcl", 161), ("igrave", 236), ("image", 8465),
    ("infin", 8734), ("int", 8747), ("iota", 953), ("iquest", 191), ("isin", 8712), ("iuml", 239),
    ("kappa", 954), ("lArr", 8656), ("lambda", 955), ("lang", 9001), ("laquo", 171),
    ("larr", 8592), ("lceil", 8968), ("ldquo", 8220), ("le", 8804), ("lfloor", 8970),
    ("lowast", 8727), ("loz", 9674), ("lrm", 8206), ("lsaquo", 8249), ("lsquo", 8216), ("lt", 60),
    ("macr", 175), ("mdash", 8212), ("micro", 181), ("middot", 183), ("minus", 8722), ("mu", 956),
    ("nabla", 8711), ("nbsp", 160), ("ndash", 8211), ("ne", 8800), ("ni", 8715), ("not", 172),
    ("notin", 8713), ("nsub", 8836), ("ntilde", 241), ("nu", 957), ("oacute", 243), ("ocirc", 244),
    ("oelig", 339), ("ograve", 242), ("oline", 8254), ("omega", 969), ("omicron", 959),
    ("oplus", 8853), ("or", 8744), ("ordf", 170), ("ordm", 186), ("oslash", 248), ("otilde", 245),
    ("otimes", 8855), ("ouml", 246), ("para", 182), ("part", 8706), ("permil", 8240),
    ("perp", 8869), ("phi", 966), ("pi", 960), ("piv", 982), ("plusmn", 177), ("pound", 163),
    ("prime", 8242), ("prod", 8719), ("prop", 8733), ("psi", 968), ("quot", 34), ("rArr", 8658),
    ("radic", 8730), ("rang", 9002), ("raquo", 187), ("rarr", 8594), ("rceil", 8969),
    ("rdquo", 8221), ("real", 8476), ("reg", 174), ("rfloor", 8971), ("rho", 961), ("rlm", 8207),
    ("rsaquo", 8250), ("rsquo", 8217), ("sbquo", 8218), ("scaron", 353), ("sdot", 8901),
    ("sect", 167), ("shy", 173), ("sigma", 963), ("sigmaf", 962), ("sim", 8764), ("spades", 9824),
    ("sub", 8834), ("sube", 8838), ("sum", 8721), ("sup", 8835), ("sup1", 185), ("sup2", 178),
    ("sup3", 179), ("supe", 8839), ("szlig", 223), ("tau", 964), ("there4", 8756), ("theta", 952),
    ("thetasym", 977), ("thinsp", 8201), ("thorn", 254), ("tilde", 732), ("times", 215),
    ("trade", 8482), ("uArr", 8657), ("uacute", 250), ("uarr", 8593), ("ucirc", 251),
    ("ugrave", 249), ("uml", 168), ("upsih", 978), ("upsilon", 965), ("uuml", 252),
    ("weierp", 8472), ("xi", 958), ("yacute", 253), ("yen", 165), ("yuml", 255), ("zeta", 950),
    ("zwj", 8205), ("zwnj", 8204),
];

pub fn lookup(name: &str) -> Option<char> {
    ENTITIES
        .binary_search_by(|(entry, _)| (*entry).cmp(name))
        .ok()
        .and_then(|index| char::from_u32(ENTITIES[index].1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_sorted_and_complete() {
        assert!(ENTITIES.windows(2).all(|pair| pair[0].0 < pair[1].0));
        assert_eq!(lookup("nbsp"), Some('\u{A0}'));
        assert_eq!(lookup("copy"), Some('©'));
        assert_eq!(lookup("mdash"), Some('—'));
        assert_eq!(lookup("AElig"), Some('Æ'));
        assert_eq!(lookup("Omega"), Some('Ω'));
        assert_eq!(lookup("euro"), Some('€'));
        assert_eq!(lookup("apos"), None, "apos is XHTML, not HTML 4.01");
    }
}
```

- [ ] **Step 3: Add `is_safe_url` with its test**

In `src/preview/links.rs`, add after `resolve_image_path`:

```rust
/// Whether the HTML sanitizer keeps a link or image reference: web and mail links, fragments, and
/// paths without a scheme. `javascript:`, `data:`, and every other scheme are removed.
pub fn is_safe_url(dest: &str) -> bool {
    let dest = dest.trim();
    let lower = dest.to_ascii_lowercase();
    lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("mailto:")
        || !has_scheme(dest)
}
```

and to its `tests` module:

```rust
    #[test]
    fn safe_urls_are_web_mail_fragments_and_paths() {
        for dest in [
            "https://x.dev",
            "HTTP://X",
            "mailto:a@b",
            "#part",
            "img/a.png",
            r"C:\a.png",
            "",
        ] {
            assert!(is_safe_url(dest), "{dest}");
        }
        for dest in [
            "javascript:alert(1)",
            " JavaScript:x",
            "data:text/html,x",
            "file:///C:/x",
            "vbscript:x",
        ] {
            assert!(!is_safe_url(dest), "{dest}");
        }
    }
```

- [ ] **Step 4: Write the tokenizer and sanitizer tests first**

Create `src/preview/html.rs` containing only this test module; Step 5 adds the code above it:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn start(name: &str, attrs: &[(&str, &str)]) -> Token {
        Token::Start {
            name: name.into(),
            attrs: attrs
                .iter()
                .map(|(key, value)| ((*key).into(), (*value).into()))
                .collect(),
            self_closing: false,
        }
    }

    fn end(name: &str) -> Token {
        Token::End { name: name.into() }
    }

    fn text(value: &str) -> Token {
        Token::Text(value.into())
    }

    #[test]
    fn tags_text_and_nesting() {
        assert_eq!(
            tokenize(r#"<div align="center">Hi <b>there</b></div>"#),
            vec![
                start("div", &[("align", "center")]),
                text("Hi "),
                start("b", &[]),
                text("there"),
                end("b"),
                end("div"),
            ]
        );
    }

    #[test]
    fn attribute_forms_and_case() {
        assert_eq!(
            tokenize("<IMG SRC='a.png' width=96 Alt=\"x y\" hidden>"),
            vec![start(
                "img",
                &[("src", "a.png"), ("width", "96"), ("alt", "x y"), ("hidden", "")]
            )]
        );
        let br = Token::Start {
            name: "br".into(),
            attrs: Vec::new(),
            self_closing: true,
        };
        assert_eq!(tokenize("<br/><br />"), vec![br.clone(), br]);
        assert_eq!(
            tokenize(r#"<a href="x" href="y">"#),
            vec![start("a", &[("href", "x")])]
        );
    }

    #[test]
    fn invalid_markup_stays_text() {
        for source in [
            "a < b",
            "<3",
            "x <div",
            r#"<a href="unterminated>t"#,
            "</ div>",
            "<!DOCTYPE html>",
        ] {
            assert_eq!(tokenize(source), vec![text(source)], "{source}");
        }
    }

    #[test]
    fn comments_are_discarded_even_when_unterminated() {
        assert_eq!(
            tokenize("a<!-- x > y -->b"),
            vec![text("a"), Token::Comment, text("b")]
        );
        assert_eq!(tokenize("a<!-- open"), vec![text("a"), Token::Comment]);
    }

    #[test]
    fn character_references_decode_in_text_and_attributes() {
        assert_eq!(
            tokenize("&copy; &#169; &#xA9; &#XA9; &nbsp;"),
            vec![text("© © © © \u{A0}")]
        );
        assert_eq!(
            tokenize("&unknown; &amp &;"),
            vec![text("&unknown; &amp &;")]
        );
        assert_eq!(
            tokenize("&#0;&#xD800;&#x110000;&#99999999999;"),
            vec![text("\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}")]
        );
        assert_eq!(
            tokenize(r#"<p title="a &amp; b">"#),
            vec![start("p", &[("title", "a & b")])]
        );
        assert_eq!(decode_entities("&#x;&#;&#12a;"), "&#x;&#;&#12a;");
    }

    fn sanitized(source: &str) -> Vec<Token> {
        let mut sanitizer = Sanitizer::default();
        tokenize(source)
            .into_iter()
            .filter_map(|token| sanitizer.filter(token))
            .collect()
    }

    #[test]
    fn dangerous_elements_are_removed_with_their_content() {
        assert_eq!(
            sanitized("a<script>alert(1)</script>b"),
            vec![text("a"), text("b")]
        );
        assert_eq!(sanitized("<svg><svg></svg>x</svg>y"), vec![text("y")]);
        assert_eq!(sanitized("<style/>kept"), vec![text("kept")]);
    }

    #[test]
    fn unknown_tags_are_dropped_and_their_text_kept() {
        assert_eq!(
            sanitized("<center>hi</center><u>x</u>"),
            vec![text("hi"), text("x")]
        );
    }

    #[test]
    fn only_allowlisted_attributes_and_safe_urls_survive() {
        assert_eq!(
            sanitized(r#"<a href="https://x.dev" style="color:red" onclick="f()">"#),
            vec![start("a", &[("href", "https://x.dev")])]
        );
        assert_eq!(
            sanitized(r#"<a href="javascript:alert(1)" title="t">"#),
            vec![start("a", &[("title", "t")])]
        );
        assert_eq!(
            sanitized(r#"<img src="data:image/png;base64,AA" alt="a">"#),
            vec![start("img", &[("alt", "a")])]
        );
        assert_eq!(sanitized("<!-- note -->"), vec![]);
    }
}
```

- [ ] **Step 5: Implement the tokenizer and sanitizer**

Put this above the test module in `src/preview/html.rs`:

```rust
//! HTML inside Markdown: a small error-tolerant tokenizer and GitHub's sanitizer rules. Nothing
//! here fails; markup that is not a valid tag stays text, as GitHub shows it.

use crate::preview::html_entities;
use crate::preview::links::is_safe_url;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Token {
    Start {
        name: String,
        attrs: Vec<(String, String)>,
        self_closing: bool,
    },
    End {
        name: String,
    },
    /// Text with character references decoded.
    Text(String),
    /// A comment; its content is discarded.
    Comment,
}

/// Tags the preview renders. Every other tag is dropped and its text kept, unless it is in
/// `REMOVED_WITH_CONTENT`.
const ALLOWED_TAGS: &[&str] = &[
    "a", "abbr", "b", "bdo", "br", "cite", "code", "del", "details", "dfn", "div", "em", "h1",
    "h2", "h3", "h4", "h5", "h6", "h7", "h8", "hr", "i", "img", "ins", "kbd", "mark", "p",
    "picture", "q", "s", "samp", "small", "source", "span", "strike", "strong", "sub", "summary",
    "sup", "time", "tt", "var", "wbr",
];

/// Elements GitHub removes together with everything inside them.
const REMOVED_WITH_CONTENT: &[&str] = &[
    "button", "form", "iframe", "math", "noscript", "object", "script", "select", "style", "svg",
    "template", "textarea",
];

const ALLOWED_ATTRIBUTES: &[&str] = &[
    "align", "alt", "height", "href", "id", "media", "name", "open", "src", "srcset", "title",
    "width",
];

pub fn attr<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

pub fn tokenize(html: &str) -> Vec<Token> {
    let bytes = html.as_bytes();
    let mut tokens = Vec::new();
    let mut text_start = 0;
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'<' {
            index += 1;
            continue;
        }
        let parsed = if html[index..].starts_with("<!--") {
            let end = html[index + 4..]
                .find("-->")
                .map_or(bytes.len(), |found| index + 4 + found + 3);
            Some((Token::Comment, end))
        } else {
            parse_tag(html, index)
        };
        match parsed {
            Some((token, end)) => {
                push_text(&mut tokens, &html[text_start..index]);
                tokens.push(token);
                index = end;
                text_start = end;
            }
            None => index += 1,
        }
    }
    push_text(&mut tokens, &html[text_start..]);
    tokens
}

fn push_text(tokens: &mut Vec<Token>, raw: &str) {
    if !raw.is_empty() {
        tokens.push(Token::Text(decode_entities(raw)));
    }
}

fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-' || byte == b':'
}

/// Parses the tag starting at `html[start] == '<'`. `None` when it is not a complete tag: the `<`
/// is then text.
fn parse_tag(html: &str, start: usize) -> Option<(Token, usize)> {
    let bytes = html.as_bytes();
    let mut index = start + 1;
    let closing = bytes.get(index) == Some(&b'/');
    if closing {
        index += 1;
    }
    if !bytes.get(index)?.is_ascii_alphabetic() {
        return None;
    }
    let name_start = index;
    while index < bytes.len() && is_name_byte(bytes[index]) {
        index += 1;
    }
    let name = html[name_start..index].to_ascii_lowercase();
    let mut attrs: Vec<(String, String)> = Vec::new();
    let mut self_closing = false;
    loop {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        match *bytes.get(index)? {
            b'>' => {
                index += 1;
                break;
            }
            b'/' => {
                index += 1;
                if bytes.get(index) == Some(&b'>') {
                    self_closing = true;
                    index += 1;
                    break;
                }
            }
            _ => {
                let attr_start = index;
                while index < bytes.len()
                    && !bytes[index].is_ascii_whitespace()
                    && !matches!(bytes[index], b'=' | b'>' | b'/')
                {
                    index += 1;
                }
                if index == attr_start {
                    // A stray `=` where a name belongs.
                    index += 1;
                    continue;
                }
                let attr_name = html[attr_start..index].to_ascii_lowercase();
                while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                    index += 1;
                }
                let mut value = String::new();
                if bytes.get(index) == Some(&b'=') {
                    index += 1;
                    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                        index += 1;
                    }
                    match *bytes.get(index)? {
                        quote @ (b'"' | b'\'') => {
                            let value_start = index + 1;
                            let end = value_start + html[value_start..].find(quote as char)?;
                            value = decode_entities(&html[value_start..end]);
                            index = end + 1;
                        }
                        _ => {
                            let value_start = index;
                            while index < bytes.len()
                                && !bytes[index].is_ascii_whitespace()
                                && bytes[index] != b'>'
                            {
                                index += 1;
                            }
                            value = decode_entities(&html[value_start..index]);
                        }
                    }
                }
                if !attrs.iter().any(|(existing, _)| *existing == attr_name) {
                    attrs.push((attr_name, value));
                }
            }
        }
    }
    let token = if closing {
        Token::End { name }
    } else {
        Token::Start {
            name,
            attrs,
            self_closing,
        }
    };
    Some((token, index))
}

pub fn decode_entities(raw: &str) -> String {
    if !raw.contains('&') {
        return raw.to_owned();
    }
    let mut output = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(amp) = rest.find('&') {
        output.push_str(&rest[..amp]);
        rest = &rest[amp..];
        match decode_reference(rest) {
            Some((character, used)) => {
                output.push(character);
                rest = &rest[used..];
            }
            None => {
                output.push('&');
                rest = &rest[1..];
            }
        }
    }
    output.push_str(rest);
    output
}

/// Decodes the reference at the start of `reference` (which begins with `&`), returning the
/// character and the bytes it used including the `;`. Unknown or malformed references are `None`
/// and stay literal; numeric references outside Unicode decode to U+FFFD.
fn decode_reference(reference: &str) -> Option<(char, usize)> {
    // The longest HTML 4.01 name is 8 bytes; bounding the search keeps text full of `&` linear.
    let end = reference.bytes().take(34).position(|byte| byte == b';')?;
    let body = &reference[1..end];
    let character = if let Some(number) = body.strip_prefix('#') {
        let (digits, radix) = match number.strip_prefix(['x', 'X']) {
            Some(hex) => (hex, 16),
            None => (number, 10),
        };
        if digits.is_empty() || !digits.chars().all(|digit| digit.is_digit(radix)) {
            return None;
        }
        u32::from_str_radix(digits, radix)
            .ok()
            .filter(|code| *code != 0)
            .and_then(char::from_u32)
            .unwrap_or('\u{FFFD}')
    } else {
        html_entities::lookup(body)?
    };
    Some((character, end + 1))
}

/// GitHub's HTML sanitizer as a token filter. It is stateful because removed elements
/// (`<script>…</script>`) can span many tokens, and in inline HTML many Markdown events.
#[derive(Debug, Default)]
pub struct Sanitizer {
    /// The element being removed and how deeply it is nested inside itself.
    removing: Option<(String, u32)>,
}

impl Sanitizer {
    pub fn filter(&mut self, token: Token) -> Option<Token> {
        if let Some((removed, depth)) = &mut self.removing {
            match &token {
                Token::Start {
                    name,
                    self_closing: false,
                    ..
                } if name == removed => *depth += 1,
                Token::End { name } if name == removed => {
                    *depth -= 1;
                    if *depth == 0 {
                        self.removing = None;
                    }
                }
                _ => {}
            }
            return None;
        }
        match token {
            Token::Start {
                name,
                attrs,
                self_closing,
            } => {
                if REMOVED_WITH_CONTENT.contains(&name.as_str()) {
                    if !self_closing {
                        self.removing = Some((name, 1));
                    }
                    None
                } else if ALLOWED_TAGS.contains(&name.as_str()) {
                    let attrs = attrs
                        .into_iter()
                        .filter(|(attr_name, value)| {
                            ALLOWED_ATTRIBUTES.contains(&attr_name.as_str())
                                && (!matches!(attr_name.as_str(), "href" | "src" | "srcset")
                                    || is_safe_url(value))
                        })
                        .collect();
                    Some(Token::Start {
                        name,
                        attrs,
                        self_closing,
                    })
                } else {
                    None
                }
            }
            Token::End { name } => ALLOWED_TAGS
                .contains(&name.as_str())
                .then_some(Token::End { name }),
            Token::Text(text) => Some(Token::Text(text)),
            Token::Comment => None,
        }
    }

    /// True inside a removed element: Markdown text and code arriving between inline HTML events
    /// belong to it and are dropped too.
    pub fn is_removing(&self) -> bool {
        self.removing.is_some()
    }

    pub fn reset(&mut self) {
        self.removing = None;
    }
}
```

- [ ] **Step 6: Compile and run the part's tests**

Run: `cargo clippy --all-targets --all-features -- -D warnings`
Expected: no warnings (rustfmt may reflow the constant arrays; run `cargo fmt` first).

Run: `cargo test --lib preview::html preview::links`
Expected: all `html`, `html_entities`, and `links` tests PASS. (`cargo test` takes one filter; run `cargo test --lib preview::html` and `cargo test --lib preview::links` if the combined form is rejected.)

- [ ] **Step 7: Commit**

```bash
git add src/preview/mod.rs src/preview/html.rs src/preview/html_entities.rs src/preview/links.rs
git commit -m "feat(preview): tokenize and sanitize HTML the way GitHub does"
```

---
## Part 2: SVG decoding (spike first)

**Files:**
- Create: `src/preview/svg.rs`
- Modify: `src/preview/mod.rs`, `src/preview/dwrite.rs`, `src/preview/images.rs`

**Interfaces:**
- Consumes: `html::{tokenize, attr, Token}` (Part 1); `images::{DecodedImage, MAX_IMAGE_PIXELS}`.
- Produces: `svg::decode_svg(&IWICImagingFactory, &Path, max_width: u32) -> Result<DecodedImage>`; `svg::natural_size(&str) -> (f32, f32)`; `svg::rewrite_for_direct2d(&str) -> String`; `svg::MAX_SVG_BYTES`; `dwrite::load_system_library(&str) -> Result<OwnedModule>` and `dwrite::create_d2d_factory(&OwnedModule, D2D1_FACTORY_TYPE) -> Result<ID2D1Factory>`, both `pub(crate)`. `images::decode_image` now decodes `.svg` files too.

- [ ] **Step 1: Share Direct2D loading with the worker**

In `src/preview/dwrite.rs`, make `load_system_library` `pub(crate)` and add below `resolve` (`D2D1_FACTORY_TYPE` is already imported):

```rust
/// Creates a Direct2D factory from a loaded `d2d1.dll`. The factory must be dropped before `module`.
pub(crate) fn create_d2d_factory(
    module: &OwnedModule,
    factory_type: D2D1_FACTORY_TYPE,
) -> Result<ID2D1Factory> {
    unsafe {
        let create: D2D1CreateFactoryFn = resolve(module, c"D2D1CreateFactory")?;
        let mut raw = std::ptr::null_mut();
        create(factory_type, &ID2D1Factory::IID, std::ptr::null(), &mut raw)
            .ok()
            .map_err(hresult_error)?;
        Ok(ID2D1Factory::from_raw(raw))
    }
}
```

and replace the `let d2d = unsafe { … };` block in `Graphics::load` with:

```rust
        let d2d = create_d2d_factory(&d2d_module, D2D1_FACTORY_TYPE_SINGLE_THREADED)?;
```

- [ ] **Step 2: Dispatch SVG files in `decode_image`**

In `src/preview/images.rs`, change the start of `decode_image` so the WIC factory is created once and `.svg` files go to Direct2D:

```rust
pub fn decode_image(path: &Path, max_width: u32) -> Result<DecodedImage> {
    let _com = ComScope::enter();
    let factory: IWICImagingFactory =
        unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }
            .map_err(hresult_error)?;
    if path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
    {
        return crate::preview::svg::decode_svg(&factory, path, max_width);
    }
    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<u16>>();
    unsafe {
        let decoder = factory
```

(delete the old `let factory: IWICImagingFactory = …` line inside the `unsafe` block; the rest of the function is unchanged). Register the module in `src/preview/mod.rs`: `pub mod svg;`.

- [ ] **Step 3: Write `svg.rs` with its tests**

Create `src/preview/svg.rs`:

```rust
//! SVG images, rasterized on the image worker thread with Direct2D's SVG renderer into the same
//! premultiplied BGRA pixels the WIC decoders produce. Direct2D implements SVG 1.1 shapes, paths,
//! gradients, `<use>`, transforms, opacity, and clipping, and never fetches external resources; text,
//! filters, masks, and CSS stylesheets do not render.

use crate::preview::dwrite::{create_d2d_factory, hresult_error, load_system_library};
use crate::preview::html::{Token, attr, tokenize};
use crate::preview::images::{DecodedImage, MAX_IMAGE_PIXELS};
use crate::{FastPadError, Result};
use std::path::Path;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_SIZE_F, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_FACTORY_TYPE_MULTI_THREADED, D2D1_RENDER_TARGET_PROPERTIES, ID2D1DeviceContext5,
    ID2D1Factory,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Imaging::{
    GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory, WICBitmapCacheOnLoad,
};
use windows::core::Interface;
use windows_numerics::Matrix3x2;

pub const MAX_SVG_BYTES: u64 = 8 * 1024 * 1024;
/// A browser's size for an SVG with neither dimensions nor a viewBox.
const DEFAULT_SIZE: (f32, f32) = (300.0, 150.0);

pub fn decode_svg(
    wic: &IWICImagingFactory,
    path: &Path,
    max_width: u32,
) -> Result<DecodedImage> {
    if std::fs::metadata(path).map_err(FastPadError::Io)?.len() > MAX_SVG_BYTES {
        return Err(FastPadError::Invariant("SVG file is larger than 8 MB"));
    }
    let bytes = std::fs::read(path).map_err(FastPadError::Io)?;
    let source = String::from_utf8_lossy(&bytes);
    let natural = natural_size(&source);
    let (natural_width, natural_height) = (natural.0.ceil() as u32, natural.1.ceil() as u32);
    if natural_width == 0
        || natural_height == 0
        || u64::from(natural_width) * u64::from(natural_height) > MAX_IMAGE_PIXELS
    {
        return Err(FastPadError::Invariant(
            "image is empty or larger than 64 megapixels",
        ));
    }
    let (width, height) = if max_width > 0 && natural_width > max_width {
        let scaled = (u64::from(natural_height) * u64::from(max_width)) / u64::from(natural_width);
        (max_width, scaled.max(1) as u32)
    } else {
        (natural_width, natural_height)
    };
    let document = rewrite_for_direct2d(&source);
    let module = load_system_library("d2d1.dll")?;
    let pixels = {
        let factory = create_d2d_factory(&module, D2D1_FACTORY_TYPE_MULTI_THREADED)?;
        rasterize(wic, &factory, document.as_bytes(), natural, (width, height))?
    };
    drop(module);
    Ok(DecodedImage {
        width,
        height,
        natural_width,
        natural_height,
        pixels,
    })
}

fn rasterize(
    wic: &IWICImagingFactory,
    factory: &ID2D1Factory,
    document: &[u8],
    natural: (f32, f32),
    (width, height): (u32, u32),
) -> Result<Vec<u8>> {
    unsafe {
        let bitmap = wic
            .CreateBitmap(
                width,
                height,
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapCacheOnLoad,
            )
            .map_err(hresult_error)?;
        let properties = D2D1_RENDER_TARGET_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            ..Default::default()
        };
        let target = factory
            .CreateWicBitmapRenderTarget(&bitmap, &properties)
            .map_err(hresult_error)?;
        // SVG support arrived with ID2D1DeviceContext5 in Windows 10 1703; older systems fail here
        // and the image shows its placeholder.
        let context: ID2D1DeviceContext5 = target.cast().map_err(hresult_error)?;
        let stream = wic.CreateStream().map_err(hresult_error)?;
        stream.InitializeFromMemory(document).map_err(hresult_error)?;
        let svg = context
            .CreateSvgDocument(
                &stream,
                D2D_SIZE_F {
                    width: natural.0,
                    height: natural.1,
                },
            )
            .map_err(hresult_error)?;
        context.BeginDraw();
        context.Clear(Some(&D2D1_COLOR_F {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        }));
        context.SetTransform(&Matrix3x2::scale(
            width as f32 / natural.0,
            height as f32 / natural.1,
        ));
        context.DrawSvgDocument(&svg);
        context.EndDraw(None, None).map_err(hresult_error)?;
        drop(svg);
        drop(context);
        drop(target);
        let mut pixels = vec![0_u8; width as usize * height as usize * 4];
        bitmap
            .CopyPixels(std::ptr::null(), width * 4, &mut pixels)
            .map_err(hresult_error)?;
        Ok(pixels)
    }
}

/// The root `<svg>` element's size: `width`/`height` in plain numbers or `px`, completed from the
/// `viewBox` aspect ratio when only one is given; otherwise the `viewBox` size; otherwise 300×150.
pub fn natural_size(svg: &str) -> (f32, f32) {
    let Some(start) = svg
        .match_indices("<svg")
        .map(|(index, _)| index)
        .find(|index| element_is(&svg[*index..], "svg"))
    else {
        return DEFAULT_SIZE;
    };
    let end = tag_end(&svg[start..]).map_or(svg.len(), |end| start + end);
    let Some(Token::Start { attrs, .. }) = tokenize(&svg[start..end]).into_iter().next() else {
        return DEFAULT_SIZE;
    };
    let width = attr(&attrs, "width").and_then(parse_length);
    let height = attr(&attrs, "height").and_then(parse_length);
    let view_box = attr(&attrs, "viewbox").and_then(parse_view_box);
    match (width, height, view_box) {
        (Some(width), Some(height), _) => (width, height),
        (Some(width), None, Some((box_width, box_height))) => {
            (width, width * box_height / box_width)
        }
        (None, Some(height), Some((box_width, box_height))) => {
            (height * box_width / box_height, height)
        }
        (None, None, Some(size)) => size,
        (Some(width), None, None) => (width, DEFAULT_SIZE.1),
        (None, Some(height), None) => (DEFAULT_SIZE.0, height),
        (None, None, None) => DEFAULT_SIZE,
    }
}

fn parse_length(value: &str) -> Option<f32> {
    let value = value.trim();
    let number = value.strip_suffix("px").unwrap_or(value).trim();
    number
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite() && *value > 0.0)
}

fn parse_view_box(value: &str) -> Option<(f32, f32)> {
    let numbers = value
        .split(|character: char| character.is_ascii_whitespace() || character == ',')
        .filter(|part| !part.is_empty())
        .map(|part| part.parse::<f32>().ok())
        .collect::<Option<Vec<_>>>()?;
    match numbers.as_slice() {
        [_, _, width, height]
            if width.is_finite() && height.is_finite() && *width > 0.0 && *height > 0.0 =>
        {
            Some((*width, *height))
        }
        _ => None,
    }
}

/// Direct2D implements SVG 1.1 linking: `<use>` needs `xlink:href`, and the root must declare the
/// `xlink` namespace. SVG 2 files written with plain `href` are rewritten to that form.
pub fn rewrite_for_direct2d(svg: &str) -> String {
    let mut output = String::with_capacity(svg.len() + 64);
    let mut rest = svg;
    let mut root_seen = false;
    while let Some(open) = rest.find('<') {
        output.push_str(&rest[..open]);
        rest = &rest[open..];
        let Some(end) = tag_end(rest) else {
            break;
        };
        let tag = &rest[..end];
        if !root_seen && element_is(tag, "svg") {
            root_seen = true;
            output.push_str("<svg");
            if !tag.contains("xmlns:xlink") {
                output.push_str(r#" xmlns:xlink="http://www.w3.org/1999/xlink""#);
            }
            output.push_str(&tag[4..]);
        } else if element_is(tag, "use") {
            output.push_str(&xlink_hrefs(tag));
        } else {
            output.push_str(tag);
        }
        rest = &rest[end..];
    }
    output.push_str(rest);
    output
}

/// Whether `tag` (starting at `<`) opens element `name`.
fn element_is(tag: &str, name: &str) -> bool {
    tag.strip_prefix('<')
        .and_then(|tag| tag.strip_prefix(name))
        .is_some_and(|rest| {
            rest.starts_with(|character: char| {
                character.is_ascii_whitespace() || character == '>' || character == '/'
            })
        })
}

/// The byte length of the tag at the start of `tag`, through its `>`, skipping quoted values.
fn tag_end(tag: &str) -> Option<usize> {
    let mut quote = None;
    for (index, byte) in tag.bytes().enumerate().skip(1) {
        match quote {
            Some(open) if byte == open => quote = None,
            Some(_) => {}
            None if byte == b'"' || byte == b'\'' => quote = Some(byte),
            None if byte == b'>' => return Some(index + 1),
            None => {}
        }
    }
    None
}

fn xlink_hrefs(tag: &str) -> String {
    let bytes = tag.as_bytes();
    let mut output = String::with_capacity(tag.len() + 6);
    let mut copied = 0;
    let mut search = 1;
    while let Some(found) = tag[search..].find("href=") {
        let at = search + found;
        if bytes[at - 1].is_ascii_whitespace() {
            output.push_str(&tag[copied..at]);
            output.push_str("xlink:");
            copied = at;
        }
        search = at + 5;
    }
    output.push_str(&tag[copied..]);
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::images::decode_image;
    use std::path::PathBuf;

    fn icon() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("fastpad-icon.svg")
    }

    fn scratch(name: &str, contents: &[u8]) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("fastpad-svg-{name}-{}.svg", std::process::id()));
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn pixel(image: &DecodedImage, x: u32, y: u32) -> [u8; 4] {
        let offset = ((y * image.width + x) * 4) as usize;
        image.pixels[offset..offset + 4].try_into().unwrap()
    }

    /// BGRA within a small tolerance for antialiasing.
    fn near(actual: [u8; 4], expected: [u8; 4]) -> bool {
        actual
            .iter()
            .zip(expected)
            .all(|(actual, expected)| actual.abs_diff(expected) <= 8)
    }

    const HEADER_ORANGE: [u8; 4] = [0x20, 0xB0, 0xFF, 0xFF];

    /// The spec §7.3 spike: Direct2D must rasterize SVG into a WIC bitmap off the UI thread.
    #[test]
    fn the_fastpad_icon_rasterizes_on_a_worker_thread() {
        let image = std::thread::spawn(|| decode_image(&icon(), 96))
            .join()
            .unwrap()
            .unwrap();
        assert_eq!((image.natural_width, image.natural_height), (256, 256));
        assert_eq!((image.width, image.height), (96, 96));
        // The header band (#FFB020) at (150, 72) of 256 px, below the binder rings.
        let header = pixel(&image, 56, 27);
        assert!(near(header, HEADER_ORANGE), "{header:?}");
        // Outside the notepad the icon is transparent.
        assert_eq!(pixel(&image, 2, 2), [0, 0, 0, 0]);
    }

    #[test]
    fn use_elements_render_after_the_xlink_rewrite() {
        let image = decode_image(&icon(), 0).unwrap();
        // A binder ring drawn through `<use href="#rings">` covers the header at (95, 50).
        let ring = pixel(&image, 95, 50);
        assert!(!near(ring, HEADER_ORANGE), "{ring:?}");
    }

    #[test]
    fn malformed_huge_and_oversized_svgs_fail() {
        let malformed = scratch("malformed", b"<svg width='10' height='10'><path d=");
        assert!(decode_image(&malformed, 0).is_err());
        let huge = scratch(
            "huge",
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="100000" height="100000"></svg>"#,
        );
        assert!(decode_image(&huge, 0).is_err());
        let mut padded = b"<svg xmlns='http://www.w3.org/2000/svg' width='1' height='1'>".to_vec();
        padded.resize(MAX_SVG_BYTES as usize + 1, b' ');
        let oversized = scratch("oversized", &padded);
        assert!(decode_image(&oversized, 0).is_err());
        for path in [malformed, huge, oversized] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn natural_size_follows_dimensions_then_view_box_then_the_browser_default() {
        assert_eq!(natural_size(r#"<svg width="64" height="32px">"#), (64.0, 32.0));
        assert_eq!(
            natural_size(r#"<?xml version="1.0"?><!-- c --><svg viewBox="0 0 256 128">"#),
            (256.0, 128.0)
        );
        assert_eq!(
            natural_size(r#"<svg width="100%" viewBox="0,0,10,20">"#),
            (10.0, 20.0)
        );
        assert_eq!(
            natural_size(r#"<svg width="50" viewBox="0 0 10 20">"#),
            (50.0, 100.0)
        );
        assert_eq!(natural_size("<svg>"), (300.0, 150.0));
        assert_eq!(
            natural_size("<svgx width='9'><svg height='40'>"),
            (300.0, 40.0)
        );
        assert_eq!(natural_size("not svg"), (300.0, 150.0));
    }

    #[test]
    fn use_elements_get_xlink_hrefs_and_the_root_declares_the_namespace() {
        assert_eq!(
            rewrite_for_direct2d(
                r##"<svg viewBox="0 0 1 1"><use href="#a"/><use xlink:href="#b"/><a href="x"/></svg>"##
            ),
            r##"<svg xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 1 1"><use xlink:href="#a"/><use xlink:href="#b"/><a href="x"/></svg>"##
        );
        let declared = r##"<svg xmlns:xlink="http://www.w3.org/1999/xlink"><use href="#a"/></svg>"##;
        assert_eq!(
            rewrite_for_direct2d(declared),
            declared.replace(" href=", " xlink:href=")
        );
    }

    #[test]
    #[ignore = "performance measurement: cargo test --release --lib preview::svg -- --ignored --nocapture"]
    fn decoding_the_icon_at_256_px_is_recorded() {
        let mut samples = (0..21)
            .map(|_| {
                let started = std::time::Instant::now();
                decode_image(&icon(), 256).unwrap();
                started.elapsed().as_micros() as u64
            })
            .collect::<Vec<_>>();
        samples.sort_unstable();
        println!(
            "svg decode at 256 px: median {} us, max {} us",
            samples[10], samples[20]
        );
    }
}
```

In `src/preview/images.rs` tests, add:

```rust
    #[test]
    fn the_cache_decodes_svg_files_and_decodes_them_larger_on_request() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("fastpad-icon.svg");
        let mut cache = ImageCache::new(std::ptr::null_mut(), 0);
        cache.request(&path, 16);
        wait_for(&mut cache, &path, 16);
        assert_eq!(cache.size(&path), Some((256, 256)));
        cache.request(&path, 64);
        wait_for(&mut cache, &path, 64);
    }
```

- [ ] **Step 4: Run the spike and the part's tests — stop here if the spike fails**

Run: `cargo clippy --all-targets --all-features -- -D warnings`
Expected: no warnings.

Run: `cargo test --lib preview::svg`
Expected: PASS, including `the_fastpad_icon_rasterizes_on_a_worker_thread`.

**Gate:** if `the_fastpad_icon_rasterizes_on_a_worker_thread` fails at the `cast::<ID2D1DeviceContext5>()` or `CreateSvgDocument` call (an `E_NOINTERFACE` / `Win32(0x80004002)` error), stop the run and report it to the owner: the spec §7.3 UI-thread fallback then needs its own plan amendment before any later part. Any other failure (a colour off by more than the tolerance, a wrong size) is a normal bug to fix here.

Run: `cargo test --lib preview::images`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/preview/mod.rs src/preview/svg.rs src/preview/dwrite.rs src/preview/images.rs
git commit -m "feat(preview): rasterize SVG images with Direct2D on the image worker"
```

---
## Part 3: DirectWrite inline object

**Files:**
- Create: `src/preview/inline_object.rs`
- Modify: `src/preview/mod.rs`

**Interfaces:**
- Produces: `inline_object::inline_box(width: f32, height: f32) -> IDWriteInlineObject` — an object whose bottom edge sits on the baseline, breaks like a word, and draws nothing.

- [ ] **Step 1: Write the inline object and its tests**

Register `pub mod inline_object;` in `src/preview/mod.rs`, then create `src/preview/inline_object.rs`:

```rust
//! A fixed-size DirectWrite inline object that reserves an image's space inside a text layout. The
//! preview draws the image itself (`DrawOp::Image`) where DirectWrite placed the object, so `Draw`
//! does nothing. Hand-written COM, like the MSAA providers, so no proc-macro dependency is needed.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use windows::Win32::Foundation::{E_NOINTERFACE, E_POINTER, S_OK};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_BREAK_CONDITION, DWRITE_BREAK_CONDITION_NEUTRAL, DWRITE_INLINE_OBJECT_METRICS,
    DWRITE_OVERHANG_METRICS, IDWriteInlineObject,
};
use windows::core::{BOOL, GUID, HRESULT, IUnknown, Interface};

#[repr(C)]
struct InlineBox {
    vtable: &'static Vtable,
    references: AtomicU32,
    width: f32,
    height: f32,
}

/// `IDWriteInlineObject`'s vtable: `IUnknown`, then `Draw`, `GetMetrics`, `GetOverhangMetrics`,
/// and `GetBreakConditions`.
#[repr(C)]
struct Vtable {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    draw: unsafe extern "system" fn(
        *mut c_void,
        *const c_void,
        *mut c_void,
        f32,
        f32,
        BOOL,
        BOOL,
        *mut c_void,
    ) -> HRESULT,
    get_metrics: unsafe extern "system" fn(*mut c_void, *mut DWRITE_INLINE_OBJECT_METRICS) -> HRESULT,
    get_overhang_metrics:
        unsafe extern "system" fn(*mut c_void, *mut DWRITE_OVERHANG_METRICS) -> HRESULT,
    get_break_conditions: unsafe extern "system" fn(
        *mut c_void,
        *mut DWRITE_BREAK_CONDITION,
        *mut DWRITE_BREAK_CONDITION,
    ) -> HRESULT,
}

static VTABLE: Vtable = Vtable {
    query_interface,
    add_ref,
    release,
    draw,
    get_metrics,
    get_overhang_metrics,
    get_break_conditions,
};

#[cfg(test)]
thread_local! {
    static LIVE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// An inline object `width` by `height` DIPs whose bottom edge sits on the text baseline.
pub fn inline_box(width: f32, height: f32) -> IDWriteInlineObject {
    #[cfg(test)]
    LIVE.with(|live| live.set(live.get() + 1));
    let object = Box::into_raw(Box::new(InlineBox {
        vtable: &VTABLE,
        references: AtomicU32::new(1),
        width,
        height,
    }));
    // SAFETY: `InlineBox` starts with a pointer to a vtable laid out as `IDWriteInlineObject`'s, and
    // the reference it was created with moves into the returned interface.
    unsafe { IDWriteInlineObject::from_raw(object.cast()) }
}

unsafe fn object<'a>(this: *mut c_void) -> &'a InlineBox {
    unsafe { &*this.cast::<InlineBox>() }
}

unsafe extern "system" fn query_interface(
    this: *mut c_void,
    iid: *const GUID,
    output: *mut *mut c_void,
) -> HRESULT {
    if iid.is_null() || output.is_null() {
        return E_POINTER;
    }
    let requested = unsafe { *iid };
    if requested == IUnknown::IID || requested == IDWriteInlineObject::IID {
        unsafe {
            add_ref(this);
            *output = this;
        }
        S_OK
    } else {
        unsafe { *output = std::ptr::null_mut() };
        E_NOINTERFACE
    }
}

unsafe extern "system" fn add_ref(this: *mut c_void) -> u32 {
    unsafe { object(this) }
        .references
        .fetch_add(1, Ordering::Relaxed)
        + 1
}

unsafe extern "system" fn release(this: *mut c_void) -> u32 {
    let remaining = unsafe { object(this) }
        .references
        .fetch_sub(1, Ordering::Release)
        - 1;
    if remaining == 0 {
        drop(unsafe { Box::from_raw(this.cast::<InlineBox>()) });
        #[cfg(test)]
        LIVE.with(|live| live.set(live.get() - 1));
    }
    remaining
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn draw(
    _this: *mut c_void,
    _context: *const c_void,
    _renderer: *mut c_void,
    _origin_x: f32,
    _origin_y: f32,
    _sideways: BOOL,
    _right_to_left: BOOL,
    _effect: *mut c_void,
) -> HRESULT {
    S_OK
}

unsafe extern "system" fn get_metrics(
    this: *mut c_void,
    metrics: *mut DWRITE_INLINE_OBJECT_METRICS,
) -> HRESULT {
    if metrics.is_null() {
        return E_POINTER;
    }
    let object = unsafe { object(this) };
    unsafe {
        *metrics = DWRITE_INLINE_OBJECT_METRICS {
            width: object.width,
            height: object.height,
            baseline: object.height,
            supportsSideways: false.into(),
        };
    }
    S_OK
}

unsafe extern "system" fn get_overhang_metrics(
    _this: *mut c_void,
    overhangs: *mut DWRITE_OVERHANG_METRICS,
) -> HRESULT {
    if overhangs.is_null() {
        return E_POINTER;
    }
    unsafe { *overhangs = DWRITE_OVERHANG_METRICS::default() };
    S_OK
}

unsafe extern "system" fn get_break_conditions(
    _this: *mut c_void,
    before: *mut DWRITE_BREAK_CONDITION,
    after: *mut DWRITE_BREAK_CONDITION,
) -> HRESULT {
    if before.is_null() || after.is_null() {
        return E_POINTER;
    }
    unsafe {
        *before = DWRITE_BREAK_CONDITION_NEUTRAL;
        *after = DWRITE_BREAK_CONDITION_NEUTRAL;
    }
    S_OK
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::dwrite::Graphics;
    use windows::Win32::Graphics::DirectWrite::{
        DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_HIT_TEST_METRICS,
        DWRITE_TEXT_METRICS, DWRITE_TEXT_RANGE, IDWriteTextLayout,
    };

    fn layout(graphics: &Graphics, text: &str, width: f32) -> IDWriteTextLayout {
        let format = graphics
            .text_format(
                "Segoe UI",
                16.0,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
            )
            .unwrap();
        let wide = text.encode_utf16().collect::<Vec<_>>();
        unsafe {
            graphics
                .dwrite
                .CreateTextLayout(&wide, &format, width, 1000.0)
        }
        .unwrap()
    }

    fn place(layout: &IDWriteTextLayout, position: u32, width: f32, height: f32) {
        let range = DWRITE_TEXT_RANGE {
            startPosition: position,
            length: 1,
        };
        unsafe { layout.SetInlineObject(&inline_box(width, height), range) }.unwrap();
    }

    fn text_metrics(layout: &IDWriteTextLayout) -> DWRITE_TEXT_METRICS {
        let mut metrics = DWRITE_TEXT_METRICS::default();
        unsafe { layout.GetMetrics(&mut metrics) }.unwrap();
        metrics
    }

    #[test]
    fn an_inline_box_reserves_its_size_on_the_line() {
        let graphics = Graphics::load().unwrap();
        let layout = layout(&graphics, "a\u{FFFC}b", 500.0);
        place(&layout, 1, 40.0, 30.0);
        assert!(text_metrics(&layout).height >= 30.0);
        let (mut x, mut y) = (0.0, 0.0);
        let mut hit = DWRITE_HIT_TEST_METRICS::default();
        unsafe { layout.HitTestTextPosition(1, false, &mut x, &mut y, &mut hit) }.unwrap();
        assert_eq!(hit.width, 40.0);
        assert!(hit.left > 0.0);
    }

    #[test]
    fn inline_boxes_wrap_like_words() {
        let graphics = Graphics::load().unwrap();
        let layout = layout(&graphics, "\u{FFFC} \u{FFFC} \u{FFFC}", 110.0);
        for position in [0, 2, 4] {
            place(&layout, position, 40.0, 20.0);
        }
        assert_eq!(text_metrics(&layout).lineCount, 2);
    }

    #[test]
    fn inline_boxes_are_freed_with_their_layouts() {
        let graphics = Graphics::load().unwrap();
        let before = LIVE.with(std::cell::Cell::get);
        for _ in 0..100 {
            let layout = layout(&graphics, "\u{FFFC}", 100.0);
            place(&layout, 0, 10.0, 10.0);
            text_metrics(&layout);
        }
        assert_eq!(LIVE.with(std::cell::Cell::get), before);
    }
}
```

- [ ] **Step 2: Compile and run the part's tests**

Run: `cargo clippy --all-targets --all-features -- -D warnings`, then `cargo test --lib preview::inline_object`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src/preview/mod.rs src/preview/inline_object.rs
git commit -m "feat(preview): add a DirectWrite inline object for images in text"
```

---
## Part 4: Block model with inline images and HTML

**Files:**
- Modify: `src/preview/model.rs` (replace the whole file), `src/preview/layout.rs` (compile fixes only)

**Interfaces:**
- Consumes: `html::{tokenize, attr, Token, Sanitizer}` (Part 1).
- Produces (all `pub` in `model.rs`):
  - `OBJECT_REPLACEMENT: char = '\u{FFFC}'`
  - `BlockKind::{Heading { level, align, anchor, text }, Paragraph { align, text }, List { start, items }, Quote(Vec<BlockKind>), Code { language, text }, Table { alignments, head, rows }, Rule, Container { align, children }, Details { open, summary, children }}`
  - `TextAlign::{Inherit, Left, Center, Right, Justify}` with `TextAlign::parse(&str) -> TextAlign`
  - `RichText { text, utf16_len, spans, images: Vec<InlineImage> }` with `RichText::plain(&str) -> RichText`, `plain_text(&self) -> &str`
  - `InlineImage { position: u32, image: ImageRef }`
  - `ImageRef { dest, alt, title, width: Option<Length>, height: Option<Length>, sources: Vec<ImageSource> }` (`Default`)
  - `Length::{Pixels(u32), Percent(u32)}` with `Length::parse(&str) -> Option<Length>`
  - `ImageSource { url: String, scheme: Option<ColorScheme> }`, `ColorScheme::{Light, Dark}` with `ColorScheme::from_media(&str) -> Option<ColorScheme>`
  - `InlineStyle::{Strong, Emphasis, Strikethrough, Code, Link(String), Underline, Subscript, Superscript, Mark, Small, Keyboard}`
  - Unchanged: `Block`, `ListItem`, `CellAlign`, `Span`, `RefDef`, `normalize_label`, `parse_document`, `parse_blocks`, `PARSE_OPTIONS`.
  - Removed: `BlockKind::Images`, `BlockKind::Html`, `InlineStyle::ImageAlt`.

- [ ] **Step 1: Replace `src/preview/model.rs`**

```rust
//! Markdown source, including GitHub's HTML subset, to top-level preview blocks. Pure Rust with no
//! Win32, so every rule here is unit-tested. Inline styles are recorded as UTF-16 ranges because
//! DirectWrite addresses text in UTF-16 code units.

use crate::preview::html::{self, Sanitizer, Token};
use pulldown_cmark::{
    Alignment, BrokenLink, CodeBlockKind, CowStr, Event, Options, Parser, Tag, TagEnd,
};
use std::ops::Range;

/// GitHub-flavored extensions the preview renders. Footnotes, math, and metadata stay literal.
pub const PARSE_OPTIONS: Options = Options::ENABLE_TABLES
    .union(Options::ENABLE_STRIKETHROUGH)
    .union(Options::ENABLE_TASKLISTS);

/// The character an inline image occupies in `RichText::text`.
pub const OBJECT_REPLACEMENT: char = '\u{FFFC}';

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    /// Source byte range, including the block's trailing newline when the parser includes it.
    pub bytes: Range<usize>,
    /// Zero-based source lines the block spans, end exclusive.
    pub lines: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockKind {
    Heading {
        level: u8,
        align: TextAlign,
        /// An explicit anchor from the heading's `id` or an `<a name>` inside it.
        anchor: Option<String>,
        text: RichText,
    },
    Paragraph {
        align: TextAlign,
        text: RichText,
    },
    List {
        start: Option<u64>,
        items: Vec<ListItem>,
    },
    Quote(Vec<BlockKind>),
    Code {
        language: String,
        text: String,
    },
    Table {
        alignments: Vec<CellAlign>,
        head: Vec<RichText>,
        rows: Vec<Vec<RichText>>,
    },
    Rule,
    /// `<div>`: its children, with an alignment they inherit.
    Container {
        align: TextAlign,
        children: Vec<BlockKind>,
    },
    /// `<details>`: collapsed unless `open`.
    Details {
        open: bool,
        summary: RichText,
        children: Vec<BlockKind>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListItem {
    pub task: Option<bool>,
    pub blocks: Vec<BlockKind>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellAlign {
    None,
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextAlign {
    #[default]
    Inherit,
    Left,
    Center,
    Right,
    Justify,
}

impl TextAlign {
    /// An HTML `align` value, case-insensitive; `middle` centres like `center`. Anything else
    /// inherits.
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "left" => Self::Left,
            "center" | "middle" => Self::Center,
            "right" => Self::Right,
            "justify" => Self::Justify,
            _ => Self::Inherit,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RichText {
    pub text: String,
    pub utf16_len: u32,
    pub spans: Vec<Span>,
    /// Images placed inline, each on an `OBJECT_REPLACEMENT` character in `text`.
    pub images: Vec<InlineImage>,
}

impl RichText {
    pub fn plain(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            utf16_len: text.encode_utf16().count() as u32,
            ..Self::default()
        }
    }

    pub fn plain_text(&self) -> &str {
        &self.text
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub range: Range<u32>,
    pub style: InlineStyle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InlineStyle {
    Strong,
    Emphasis,
    Strikethrough,
    Code,
    Link(String),
    Underline,
    Subscript,
    Superscript,
    Mark,
    Small,
    Keyboard,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineImage {
    /// UTF-16 offset of the `OBJECT_REPLACEMENT` character the image occupies.
    pub position: u32,
    pub image: ImageRef,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImageRef {
    pub dest: String,
    pub alt: String,
    pub title: String,
    pub width: Option<Length>,
    pub height: Option<Length>,
    /// `<picture>` candidates in document order; empty for a plain image.
    pub sources: Vec<ImageSource>,
}

/// An HTML size attribute. Whole numbers only, so blocks stay comparable with `Eq`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Length {
    Pixels(u32),
    Percent(u32),
}

impl Length {
    /// `96`, `96px`, and `50%`; fractions are truncated, and zero or anything else is ignored.
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        let (number, percent) = match value.strip_suffix('%') {
            Some(number) => (number, true),
            None => (value.strip_suffix("px").unwrap_or(value), false),
        };
        let whole = number.trim().split('.').next()?;
        let parsed = whole.parse::<u32>().ok().filter(|value| *value > 0)?;
        Some(if percent {
            Self::Percent(parsed)
        } else {
            Self::Pixels(parsed)
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageSource {
    /// The first candidate URL of the source's `srcset`.
    pub url: String,
    pub scheme: Option<ColorScheme>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorScheme {
    Light,
    Dark,
}

impl ColorScheme {
    /// `(prefers-color-scheme: dark)` or `(prefers-color-scheme: light)`, ignoring whitespace and
    /// case. Other media queries are `None`.
    pub fn from_media(media: &str) -> Option<Self> {
        let compact = media
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>()
            .to_ascii_lowercase();
        match compact.as_str() {
            "(prefers-color-scheme:dark)" => Some(Self::Dark),
            "(prefers-color-scheme:light)" => Some(Self::Light),
            _ => None,
        }
    }
}

/// A link reference definition (`[label]: dest "title"`), kept so slices parsed on their own still
/// resolve references defined elsewhere in the document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefDef {
    pub key: String,
    pub dest: String,
    pub title: String,
    pub span: Range<usize>,
}

/// Case-folds and collapses whitespace the way reference labels are matched.
pub fn normalize_label(label: &str) -> String {
    label
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

pub fn parse_document(source: &str) -> (Vec<Block>, Vec<RefDef>) {
    let mut events = Parser::new_ext(source, PARSE_OPTIONS).into_offset_iter();
    let refdefs = events
        .reference_definitions()
        .iter()
        .map(|(label, definition)| RefDef {
            key: normalize_label(label),
            dest: definition.dest.to_string(),
            title: definition.title.as_deref().unwrap_or_default().to_owned(),
            span: definition.span.clone(),
        })
        .collect();
    let blocks = collect_blocks(&mut events, source, 0, 0);
    (blocks, refdefs)
}

pub fn parse_blocks(
    source: &str,
    base_byte: usize,
    base_line: usize,
    refdefs: &[RefDef],
) -> Vec<Block> {
    let resolve = |link: BrokenLink<'_>| {
        let key = normalize_label(&link.reference);
        refdefs
            .iter()
            .find(|definition| definition.key == key)
            .map(|definition| {
                (
                    CowStr::from(definition.dest.clone()),
                    CowStr::from(definition.title.clone()),
                )
            })
    };
    let mut events = Parser::new_with_broken_link_callback(source, PARSE_OPTIONS, Some(resolve))
        .into_offset_iter();
    collect_blocks(&mut events, source, base_byte, base_line)
}

fn collect_blocks<'a>(
    events: impl Iterator<Item = (Event<'a>, Range<usize>)>,
    source: &str,
    base_byte: usize,
    base_line: usize,
) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut builder = Builder::default();
    let mut lines = LineCounter::new(source);
    let mut top: Option<Range<usize>> = None;
    for (event, range) in events {
        if builder.is_idle() && top.is_none() {
            top = Some(range.clone());
        }
        builder.event(event);
        if let Some(kind) = builder.take_block() {
            let span = top.take().unwrap_or(range.clone());
            let bytes = span.start..span.end.max(range.end);
            push_block(&mut blocks, &mut lines, kind, bytes, base_byte, base_line);
        } else if builder.is_idle() {
            // An HTML block that produced nothing, such as a comment.
            top = None;
        }
    }
    // Elements still open at the end of the input close there, as a browser closes them.
    if let Some(kind) = builder.finish() {
        let start = top.map_or(source.len(), |span| span.start);
        push_block(
            &mut blocks,
            &mut lines,
            kind,
            start..source.len(),
            base_byte,
            base_line,
        );
    }
    blocks
}

fn push_block(
    blocks: &mut Vec<Block>,
    lines: &mut LineCounter<'_>,
    kind: BlockKind,
    bytes: Range<usize>,
    base_byte: usize,
    base_line: usize,
) {
    let first_line = lines.line_of(bytes.start);
    let last_line = lines.line_of(bytes.end.saturating_sub(1).max(bytes.start));
    blocks.push(Block {
        kind,
        bytes: base_byte + bytes.start..base_byte + bytes.end,
        lines: base_line + first_line..base_line + last_line + 1,
    });
}

/// Counts newlines forward from the last query; top-level block offsets only grow.
struct LineCounter<'a> {
    bytes: &'a [u8],
    position: usize,
    line: usize,
}

impl<'a> LineCounter<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            bytes: source.as_bytes(),
            position: 0,
            line: 0,
        }
    }

    fn line_of(&mut self, offset: usize) -> usize {
        let offset = offset.min(self.bytes.len());
        if offset < self.position {
            self.position = 0;
            self.line = 0;
        }
        self.line += self.bytes[self.position..offset]
            .iter()
            .filter(|byte| **byte == b'\n')
            .count();
        self.position = offset;
        self.line
    }
}

#[derive(Default)]
struct TextBuilder {
    text: String,
    utf16_len: u32,
    spans: Vec<Span>,
    images: Vec<InlineImage>,
    open: Vec<OpenStyle>,
    /// Markdown images whose alt text is being collected; images can nest in alt text.
    image_stack: Vec<ImageRef>,
    /// HTML text arrived: the collapsed trailing space is trimmed when the text is finished.
    html: bool,
}

struct OpenStyle {
    /// `None` for tags that only group text (`span`, `abbr`, `q`, or `a` without `href`).
    style: Option<InlineStyle>,
    start: u32,
    /// The HTML tag that opened the style, or `None` for Markdown.
    tag: Option<String>,
}

impl TextBuilder {
    fn push(&mut self, value: &str) {
        if let Some(image) = self.image_stack.last_mut() {
            image.alt.push_str(value);
            return;
        }
        self.text.push_str(value);
        self.utf16_len += value.encode_utf16().count() as u32;
    }

    /// HTML text: whitespace runs collapse to one space, and a line never starts with a space.
    fn push_html(&mut self, value: &str) {
        self.html = true;
        let mut previous = match self.image_stack.last() {
            Some(image) => image.alt.chars().next_back(),
            None => self.text.chars().next_back(),
        };
        let mut collapsed = String::with_capacity(value.len());
        for character in value.chars() {
            let character = if character.is_ascii_whitespace() {
                ' '
            } else {
                character
            };
            if character == ' ' && matches!(previous, None | Some(' ' | '\n')) {
                continue;
            }
            collapsed.push(character);
            previous = Some(character);
        }
        self.push(&collapsed);
    }

    fn open(&mut self, style: Option<InlineStyle>, tag: Option<&str>) {
        self.open.push(OpenStyle {
            style,
            start: self.utf16_len,
            tag: tag.map(str::to_owned),
        });
    }

    /// Closes the innermost open style `matches` accepts and every style opened after it. Returns
    /// false when none matches, as for a stray end tag.
    fn close_where(&mut self, matches: impl Fn(&OpenStyle) -> bool) -> bool {
        let Some(index) = self.open.iter().rposition(matches) else {
            return false;
        };
        while self.open.len() > index {
            self.close_last();
        }
        true
    }

    fn close_markdown(&mut self) {
        self.close_where(|open| open.tag.is_none());
    }

    fn close_last(&mut self) {
        let Some(open) = self.open.pop() else {
            return;
        };
        if open.tag.as_deref() == Some("q") {
            self.push("\u{201D}");
        }
        if let Some(style) = open.style
            && open.start < self.utf16_len
        {
            self.spans.push(Span {
                range: open.start..self.utf16_len,
                style,
            });
        }
    }

    fn push_image(&mut self, image: ImageRef) {
        if let Some(outer) = self.image_stack.last_mut() {
            outer.alt.push_str(&image.alt);
            return;
        }
        self.images.push(InlineImage {
            position: self.utf16_len,
            image,
        });
        self.text.push(OBJECT_REPLACEMENT);
        self.utf16_len += 1;
    }

    fn finish_markdown_image(&mut self) {
        if let Some(image) = self.image_stack.pop() {
            self.push_image(image);
        }
    }

    fn is_blank(&self) -> bool {
        self.images.is_empty() && self.text.trim().is_empty()
    }

    fn into_rich_text(mut self) -> RichText {
        while !self.open.is_empty() {
            self.close_last();
        }
        if self.html {
            let trimmed = self.text.trim_end_matches(' ').len();
            // Spaces are one byte and one UTF-16 unit each.
            self.utf16_len -= (self.text.len() - trimmed) as u32;
            self.text.truncate(trimmed);
            let end = self.utf16_len;
            self.spans.retain_mut(|span| {
                span.range.end = span.range.end.min(end);
                span.range.start < span.range.end
            });
        }
        RichText {
            text: self.text,
            utf16_len: self.utf16_len,
            spans: self.spans,
            images: self.images,
        }
    }
}

enum Frame {
    Heading {
        level: u8,
        align: TextAlign,
        anchor: Option<String>,
        text: TextBuilder,
        html: bool,
    },
    Paragraph {
        align: TextAlign,
        text: TextBuilder,
        html: bool,
    },
    List {
        start: Option<u64>,
        items: Vec<ListItem>,
    },
    Item {
        task: Option<bool>,
        blocks: Vec<BlockKind>,
        loose: Option<TextBuilder>,
    },
    Quote {
        blocks: Vec<BlockKind>,
        loose: Option<TextBuilder>,
    },
    Code {
        language: String,
        text: String,
    },
    Table {
        alignments: Vec<CellAlign>,
        head: Vec<RichText>,
        rows: Vec<Vec<RichText>>,
    },
    Row(Vec<RichText>),
    Cell(TextBuilder),
    Container {
        align: TextAlign,
        children: Vec<BlockKind>,
        loose: Option<TextBuilder>,
    },
    Details {
        open: bool,
        summary: Option<RichText>,
        children: Vec<BlockKind>,
        loose: Option<TextBuilder>,
    },
    Summary(TextBuilder),
}

impl Frame {
    /// Opened by an HTML tag. HTML end tags close these, and so does the end of the Markdown
    /// container they sit in.
    fn is_html(&self) -> bool {
        matches!(
            self,
            Frame::Heading { html: true, .. }
                | Frame::Paragraph { html: true, .. }
                | Frame::Container { .. }
                | Frame::Details { .. }
                | Frame::Summary(_)
        )
    }

    /// An HTML element holding only inline content: any block start implies its end.
    fn is_html_inline(&self) -> bool {
        matches!(
            self,
            Frame::Heading { html: true, .. } | Frame::Paragraph { html: true, .. } | Frame::Summary(_)
        )
    }

    /// Markdown inline content (a paragraph, heading, or table cell), where HTML block tags do
    /// nothing.
    fn is_markdown_inline(&self) -> bool {
        matches!(
            self,
            Frame::Heading { html: false, .. } | Frame::Paragraph { html: false, .. } | Frame::Cell(_)
        )
    }

    /// Text arriving directly inside a container forms an implicit paragraph.
    fn loose(&mut self) -> Option<&mut Option<TextBuilder>> {
        match self {
            Frame::Item { loose, .. }
            | Frame::Quote { loose, .. }
            | Frame::Container { loose, .. }
            | Frame::Details { loose, .. } => Some(loose),
            _ => None,
        }
    }

    fn children(&mut self) -> Option<&mut Vec<BlockKind>> {
        match self {
            Frame::Item { blocks, .. }
            | Frame::Quote { blocks, .. }
            | Frame::Container {
                children: blocks, ..
            }
            | Frame::Details {
                children: blocks, ..
            } => Some(blocks),
            _ => None,
        }
    }
}

#[derive(Default)]
struct Builder {
    stack: Vec<Frame>,
    /// Top-level blocks finished but not yet taken; more than one only inside an HTML block.
    done: Vec<BlockKind>,
    /// Top-level HTML text outside any element: an implicit paragraph.
    loose: Option<TextBuilder>,
    /// The HTML block being read. It is tokenized when it ends, so a tag split across lines stays
    /// whole.
    html_block: Option<String>,
    sanitizer: Sanitizer,
    /// The `<source>` candidates of the open `<picture>`.
    picture: Option<Vec<ImageSource>>,
}

impl Builder {
    /// Nothing in progress: the next event starts a new top-level block.
    fn is_idle(&self) -> bool {
        self.stack.is_empty()
            && self.done.is_empty()
            && self.loose.is_none()
            && self.html_block.is_none()
    }

    /// The finished top-level block, once nothing is open. Several blocks finished inside one HTML
    /// block become one container, so every CommonMark top-level block yields at most one block
    /// and no parser state crosses a block boundary.
    fn take_block(&mut self) -> Option<BlockKind> {
        if !self.stack.is_empty()
            || self.html_block.is_some()
            || self.loose.is_some()
            || self.done.is_empty()
        {
            return None;
        }
        self.sanitizer.reset();
        self.picture = None;
        if self.done.len() == 1 {
            self.done.pop()
        } else {
            Some(BlockKind::Container {
                align: TextAlign::Inherit,
                children: std::mem::take(&mut self.done),
            })
        }
    }

    fn finish(&mut self) -> Option<BlockKind> {
        if let Some(html) = self.html_block.take() {
            self.html(&html);
        }
        while !self.stack.is_empty() {
            self.close_top();
        }
        self.flush_loose_text();
        self.take_block()
    }

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(&text),
            Event::Code(code) => {
                if !self.sanitizer.is_removing()
                    && let Some(target) = self.inline_target()
                {
                    target.open(Some(InlineStyle::Code), None);
                    target.push(&code);
                    target.close_markdown();
                }
            }
            Event::Html(html) => {
                if let Some(buffer) = &mut self.html_block {
                    buffer.push_str(&html);
                } else {
                    self.html(&html);
                }
            }
            Event::InlineHtml(html) => self.html(&html),
            Event::InlineMath(text) | Event::DisplayMath(text) | Event::FootnoteReference(text) => {
                self.text(&text)
            }
            Event::SoftBreak => self.text(" "),
            Event::HardBreak => self.text("\n"),
            Event::Rule => self.add_block(BlockKind::Rule),
            Event::TaskListMarker(checked) => match self.stack.last_mut() {
                Some(Frame::Item { task, .. }) => *task = Some(checked),
                // A loose item's marker arrives inside its first paragraph: Start(Item),
                // Start(Paragraph), TaskListMarker. The item frame sits one below the top.
                _ => {
                    let len = self.stack.len();
                    if len >= 2
                        && matches!(self.stack[len - 1], Frame::Paragraph { .. })
                        && let Frame::Item { task, blocks, .. } = &mut self.stack[len - 2]
                        && blocks.is_empty()
                    {
                        *task = Some(checked);
                    }
                }
            },
        }
    }

    fn text(&mut self, value: &str) {
        if self.sanitizer.is_removing() {
            return;
        }
        if let Some(Frame::Code { text, .. }) = self.stack.last_mut() {
            text.push_str(value);
        } else if let Some(target) = self.inline_target() {
            target.push(value);
        }
    }

    fn inline_target(&mut self) -> Option<&mut TextBuilder> {
        match self.stack.last_mut() {
            None => Some(self.loose.get_or_insert_with(TextBuilder::default)),
            Some(
                Frame::Heading { text, .. }
                | Frame::Paragraph { text, .. }
                | Frame::Cell(text)
                | Frame::Summary(text),
            ) => Some(text),
            Some(frame) => frame
                .loose()
                .map(|loose| loose.get_or_insert_with(TextBuilder::default)),
        }
    }

    fn in_markdown_inline(&self) -> bool {
        self.stack.last().is_some_and(Frame::is_markdown_inline)
    }

    fn push_inline(&mut self, value: &str) {
        if let Some(target) = self.inline_target() {
            target.push(value);
        }
    }

    fn open_markdown_style(&mut self, style: InlineStyle) {
        if let Some(target) = self.inline_target() {
            target.open(Some(style), None);
        }
    }

    fn open_html_style(&mut self, style: Option<InlineStyle>, tag: &str) {
        if let Some(target) = self.inline_target() {
            target.open(style, Some(tag));
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Emphasis => self.open_markdown_style(InlineStyle::Emphasis),
            Tag::Strong => self.open_markdown_style(InlineStyle::Strong),
            Tag::Strikethrough => self.open_markdown_style(InlineStyle::Strikethrough),
            Tag::Link { dest_url, .. } => {
                self.open_markdown_style(InlineStyle::Link(dest_url.to_string()))
            }
            Tag::Image {
                dest_url, title, ..
            } => {
                if let Some(target) = self.inline_target() {
                    target.image_stack.push(ImageRef {
                        dest: dest_url.to_string(),
                        title: title.to_string(),
                        ..ImageRef::default()
                    });
                }
            }
            Tag::Paragraph => self.push_block_frame(Frame::Paragraph {
                align: TextAlign::Inherit,
                text: TextBuilder::default(),
                html: false,
            }),
            Tag::Heading { level, .. } => self.push_block_frame(Frame::Heading {
                level: level as u8,
                align: TextAlign::Inherit,
                anchor: None,
                text: TextBuilder::default(),
                html: false,
            }),
            Tag::BlockQuote(_) => self.push_block_frame(Frame::Quote {
                blocks: Vec::new(),
                loose: None,
            }),
            Tag::CodeBlock(kind) => {
                let language = match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .to_owned(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.push_block_frame(Frame::Code {
                    language,
                    text: String::new(),
                });
            }
            Tag::HtmlBlock => self.html_block = Some(String::new()),
            Tag::List(start) => self.push_block_frame(Frame::List {
                start,
                items: Vec::new(),
            }),
            Tag::Item => self.stack.push(Frame::Item {
                task: None,
                blocks: Vec::new(),
                loose: None,
            }),
            Tag::Table(alignments) => self.push_block_frame(Frame::Table {
                alignments: alignments.into_iter().map(cell_align).collect(),
                head: Vec::new(),
                rows: Vec::new(),
            }),
            Tag::TableHead | Tag::TableRow => self.stack.push(Frame::Row(Vec::new())),
            Tag::TableCell => self.stack.push(Frame::Cell(TextBuilder::default())),
            // Not enabled by PARSE_OPTIONS; their content flows into the enclosing block.
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link => {
                if let Some(target) = self.inline_target() {
                    target.close_markdown();
                }
            }
            TagEnd::Image => {
                if let Some(target) = self.inline_target() {
                    target.finish_markdown_image();
                }
            }
            TagEnd::HtmlBlock => self.end_html_block(),
            TagEnd::Paragraph
            | TagEnd::Heading(_)
            | TagEnd::BlockQuote(_)
            | TagEnd::CodeBlock
            | TagEnd::List(_)
            | TagEnd::Table => {
                self.close_html_frames();
                self.close_top();
            }
            TagEnd::Item => {
                self.close_html_frames();
                self.flush_loose_text();
                if let Some(Frame::Item { task, blocks, .. }) = self.stack.pop()
                    && let Some(Frame::List { items, .. }) = self.stack.last_mut()
                {
                    items.push(ListItem { task, blocks });
                }
            }
            TagEnd::TableHead => {
                self.close_html_frames();
                if let Some(Frame::Row(cells)) = self.stack.pop()
                    && let Some(Frame::Table { head, .. }) = self.stack.last_mut()
                {
                    *head = cells;
                }
            }
            TagEnd::TableRow => {
                self.close_html_frames();
                if let Some(Frame::Row(cells)) = self.stack.pop()
                    && let Some(Frame::Table { rows, .. }) = self.stack.last_mut()
                {
                    rows.push(cells);
                }
            }
            TagEnd::TableCell => {
                self.close_html_frames();
                if let Some(Frame::Cell(text)) = self.stack.pop()
                    && let Some(Frame::Row(cells)) = self.stack.last_mut()
                {
                    cells.push(text.into_rich_text());
                }
            }
            _ => {}
        }
    }

    fn end_html_block(&mut self) {
        let Some(html) = self.html_block.take() else {
            return;
        };
        self.html(&html);
        if self.stack.is_empty() {
            self.flush_loose_text();
            self.sanitizer.reset();
            self.picture = None;
        }
    }

    fn html(&mut self, html: &str) {
        for token in html::tokenize(html) {
            let Some(token) = self.sanitizer.filter(token) else {
                continue;
            };
            match token {
                Token::Start { name, attrs, .. } => self.html_start(&name, &attrs),
                Token::End { name } => self.html_end(&name),
                Token::Text(text) => {
                    if let Some(target) = self.inline_target() {
                        target.push_html(&text);
                    }
                }
                Token::Comment => {}
            }
        }
    }

    fn html_start(&mut self, name: &str, attrs: &[(String, String)]) {
        let align = html::attr(attrs, "align").map_or(TextAlign::Inherit, TextAlign::parse);
        match name {
            "div" => self.html_block_frame(Frame::Container {
                align,
                children: Vec::new(),
                loose: None,
            }),
            "p" => self.html_block_frame(Frame::Paragraph {
                align,
                text: TextBuilder::default(),
                html: true,
            }),
            "details" => self.html_block_frame(Frame::Details {
                open: html::attr(attrs, "open").is_some(),
                summary: None,
                children: Vec::new(),
                loose: None,
            }),
            "summary" => {
                if self.in_markdown_inline() {
                    return;
                }
                self.close_html_inline_frames();
                if matches!(self.stack.last(), Some(Frame::Details { .. })) {
                    self.flush_loose_text();
                    self.stack.push(Frame::Summary(TextBuilder::default()));
                }
            }
            "hr" => {
                if !self.in_markdown_inline() {
                    self.add_block(BlockKind::Rule);
                }
            }
            "br" => self.push_inline("\n"),
            "wbr" => self.push_inline("\u{200B}"),
            "img" => self.html_image(attrs),
            "picture" => self.picture = Some(Vec::new()),
            "source" => {
                if let Some(sources) = &mut self.picture
                    && let Some(url) = html::attr(attrs, "srcset").and_then(first_candidate)
                {
                    sources.push(ImageSource {
                        url,
                        scheme: html::attr(attrs, "media").and_then(ColorScheme::from_media),
                    });
                }
            }
            "a" => {
                if let Some(Frame::Heading { anchor, .. }) = self.stack.last_mut()
                    && anchor.is_none()
                {
                    *anchor = html::attr(attrs, "name")
                        .or_else(|| html::attr(attrs, "id"))
                        .filter(|value| !value.is_empty())
                        .map(str::to_owned);
                }
                let link = html::attr(attrs, "href").map(|href| InlineStyle::Link(href.to_owned()));
                self.open_html_style(link, name);
            }
            "q" => {
                self.push_inline("\u{201C}");
                self.open_html_style(None, name);
            }
            _ => {
                if let Some(level) = heading_level(name) {
                    let anchor = html::attr(attrs, "id")
                        .filter(|value| !value.is_empty())
                        .map(str::to_owned);
                    self.html_block_frame(Frame::Heading {
                        level,
                        align,
                        anchor,
                        text: TextBuilder::default(),
                        html: true,
                    });
                } else if let Some(style) = inline_style(name) {
                    self.open_html_style(style, name);
                }
            }
        }
    }

    fn html_end(&mut self, name: &str) {
        match name {
            "div" => self.close_html_frame(|frame| matches!(frame, Frame::Container { .. })),
            "p" => self.close_html_frame(|frame| matches!(frame, Frame::Paragraph { html: true, .. })),
            "details" => self.close_html_frame(|frame| matches!(frame, Frame::Details { .. })),
            "summary" => self.close_html_frame(|frame| matches!(frame, Frame::Summary(_))),
            "picture" => self.picture = None,
            _ if heading_level(name).is_some() => {
                self.close_html_frame(|frame| matches!(frame, Frame::Heading { html: true, .. }))
            }
            _ => {
                if let Some(target) = self.inline_target() {
                    target.close_where(|open| open.tag.as_deref() == Some(name));
                }
            }
        }
    }

    fn html_image(&mut self, attrs: &[(String, String)]) {
        let Some(src) = html::attr(attrs, "src").filter(|src| !src.trim().is_empty()) else {
            return;
        };
        let image = ImageRef {
            dest: src.to_owned(),
            alt: html::attr(attrs, "alt").unwrap_or_default().to_owned(),
            title: html::attr(attrs, "title").unwrap_or_default().to_owned(),
            width: html::attr(attrs, "width").and_then(Length::parse),
            height: html::attr(attrs, "height").and_then(Length::parse),
            sources: self.picture.clone().unwrap_or_default(),
        };
        if let Some(target) = self.inline_target() {
            target.push_image(image);
        }
    }

    /// An HTML block element. Inside Markdown inline content the tag does nothing and its content
    /// flows into that text: paragraphs, headings, and table cells hold only inline content.
    fn html_block_frame(&mut self, frame: Frame) {
        if !self.in_markdown_inline() {
            self.push_block_frame(frame);
        }
    }

    fn push_block_frame(&mut self, frame: Frame) {
        self.close_html_inline_frames();
        self.flush_loose_text();
        self.stack.push(frame);
    }

    /// A block start ends an open `<p>`, HTML heading, or `<summary>` (HTML's implied end tags).
    fn close_html_inline_frames(&mut self) {
        while self.stack.last().is_some_and(Frame::is_html_inline) {
            self.close_top();
        }
    }

    /// HTML elements left open inside a Markdown container close when the container ends.
    fn close_html_frames(&mut self) {
        while self.stack.last().is_some_and(Frame::is_html) {
            self.close_top();
        }
    }

    /// Closes the nearest open HTML element `matches` accepts, and everything opened after it. A
    /// Markdown frame is a boundary: an end tag never closes anything outside its container, and a
    /// stray end tag is ignored.
    fn close_html_frame(&mut self, matches: impl Fn(&Frame) -> bool) {
        let Some(index) = self
            .stack
            .iter()
            .rposition(|frame| !frame.is_html() || matches(frame))
        else {
            return;
        };
        if !self.stack[index].is_html() {
            return;
        }
        while self.stack.len() > index {
            self.close_top();
        }
    }

    fn close_top(&mut self) {
        if let Some(frame) = self.stack.pop() {
            self.finish_frame(frame);
        }
    }

    fn finish_frame(&mut self, frame: Frame) {
        if let Frame::Summary(text) = frame {
            let summary = text.into_rich_text();
            let first_summary = matches!(
                self.stack.last(),
                Some(Frame::Details { summary: None, .. })
            );
            if first_summary {
                if let Some(Frame::Details { summary: slot, .. }) = self.stack.last_mut() {
                    *slot = Some(summary);
                }
            } else {
                self.add_block(BlockKind::Paragraph {
                    align: TextAlign::Inherit,
                    text: summary,
                });
            }
        } else if let Some(kind) = finish_block(frame) {
            self.add_block(kind);
        }
    }

    /// Text collected directly inside the current container becomes a paragraph before the next
    /// block starts.
    fn flush_loose_text(&mut self) {
        let loose = match self.stack.last_mut() {
            None => self.loose.take(),
            Some(frame) => frame.loose().and_then(Option::take),
        };
        if let Some(text) = loose
            && !text.is_blank()
        {
            self.attach(BlockKind::Paragraph {
                align: TextAlign::Inherit,
                text: text.into_rich_text(),
            });
        }
    }

    fn attach(&mut self, kind: BlockKind) {
        match self.stack.last_mut() {
            None => self.done.push(kind),
            // A block can only close into a container; anything else is a parser invariant break.
            Some(frame) => {
                if let Some(children) = frame.children() {
                    children.push(kind);
                }
            }
        }
    }

    fn add_block(&mut self, kind: BlockKind) {
        self.close_html_inline_frames();
        self.flush_loose_text();
        self.attach(kind);
    }
}

fn finish_block(frame: Frame) -> Option<BlockKind> {
    let kind = match frame {
        Frame::Heading {
            level,
            align,
            anchor,
            text,
            ..
        } => BlockKind::Heading {
            level,
            align,
            anchor,
            text: text.into_rich_text(),
        },
        Frame::Paragraph { align, text, html } => {
            if html && text.is_blank() {
                return None;
            }
            BlockKind::Paragraph {
                align,
                text: text.into_rich_text(),
            }
        }
        Frame::List { start, items } => BlockKind::List { start, items },
        Frame::Quote { mut blocks, loose } => {
            push_loose(&mut blocks, loose);
            BlockKind::Quote(blocks)
        }
        Frame::Code { language, text } => BlockKind::Code {
            language,
            text: text.strip_suffix('\n').unwrap_or(&text).to_owned(),
        },
        Frame::Table {
            alignments,
            head,
            rows,
        } => BlockKind::Table {
            alignments,
            head,
            rows,
        },
        Frame::Container {
            align,
            mut children,
            loose,
        } => {
            push_loose(&mut children, loose);
            if children.is_empty() {
                return None;
            }
            BlockKind::Container { align, children }
        }
        Frame::Details {
            open,
            summary,
            mut children,
            loose,
        } => {
            push_loose(&mut children, loose);
            BlockKind::Details {
                open,
                // The label a browser shows for a section without a summary.
                summary: summary.unwrap_or_else(|| RichText::plain("Details")),
                children,
            }
        }
        // Items, rows, cells, and summaries close through their own paths; these arms only keep
        // the match exhaustive without a panic in an abort-on-panic build.
        Frame::Item { .. } | Frame::Row(_) | Frame::Cell(_) | Frame::Summary(_) => return None,
    };
    Some(kind)
}

fn push_loose(blocks: &mut Vec<BlockKind>, loose: Option<TextBuilder>) {
    if let Some(text) = loose
        && !text.is_blank()
    {
        blocks.push(BlockKind::Paragraph {
            align: TextAlign::Inherit,
            text: text.into_rich_text(),
        });
    }
}

/// `h1`–`h8`. GitHub allows `h7` and `h8`, which render as `h6`.
fn heading_level(name: &str) -> Option<u8> {
    let level = name.strip_prefix('h')?.parse::<u8>().ok()?;
    (1..=8).contains(&level).then_some(level.min(6))
}

/// The style an HTML formatting tag applies: `Some(None)` for tags that only group text, `None`
/// for tags that are not inline formatting.
fn inline_style(name: &str) -> Option<Option<InlineStyle>> {
    Some(match name {
        "b" | "strong" => Some(InlineStyle::Strong),
        "i" | "em" | "cite" | "dfn" | "var" => Some(InlineStyle::Emphasis),
        "s" | "strike" | "del" => Some(InlineStyle::Strikethrough),
        "code" | "tt" | "samp" => Some(InlineStyle::Code),
        "ins" => Some(InlineStyle::Underline),
        "sub" => Some(InlineStyle::Subscript),
        "sup" => Some(InlineStyle::Superscript),
        "mark" => Some(InlineStyle::Mark),
        "small" => Some(InlineStyle::Small),
        "kbd" => Some(InlineStyle::Keyboard),
        "span" | "abbr" | "bdo" | "time" => None,
        _ => return None,
    })
}

/// The first URL of a `srcset`, before any width or density descriptor.
fn first_candidate(srcset: &str) -> Option<String> {
    srcset
        .split(|character: char| character.is_ascii_whitespace() || character == ',')
        .find(|part| !part.is_empty())
        .map(str::to_owned)
}

fn cell_align(alignment: Alignment) -> CellAlign {
    match alignment {
        Alignment::None => CellAlign::None,
        Alignment::Left => CellAlign::Left,
        Alignment::Center => CellAlign::Center,
        Alignment::Right => CellAlign::Right,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<BlockKind> {
        parse_document(source)
            .0
            .into_iter()
            .map(|block| block.kind)
            .collect()
    }

    fn plain(text: &str) -> RichText {
        RichText::plain(text)
    }

    fn styled(text: &str, spans: Vec<Span>) -> RichText {
        RichText {
            spans,
            ..plain(text)
        }
    }

    fn span(range: Range<u32>, style: InlineStyle) -> Span {
        Span { range, style }
    }

    fn para(text: RichText) -> BlockKind {
        BlockKind::Paragraph {
            align: TextAlign::Inherit,
            text,
        }
    }

    fn heading(level: u8, text: RichText) -> BlockKind {
        BlockKind::Heading {
            level,
            align: TextAlign::Inherit,
            anchor: None,
            text,
        }
    }

    fn container(align: TextAlign, children: Vec<BlockKind>) -> BlockKind {
        BlockKind::Container { align, children }
    }

    fn image(dest: &str, alt: &str) -> ImageRef {
        ImageRef {
            dest: dest.into(),
            alt: alt.into(),
            ..ImageRef::default()
        }
    }

    fn with_images(text: &str, images: Vec<(u32, ImageRef)>) -> RichText {
        RichText {
            images: images
                .into_iter()
                .map(|(position, image)| InlineImage { position, image })
                .collect(),
            ..plain(text)
        }
    }

    #[test]
    fn headings_and_paragraphs_carry_source_ranges() {
        let source = "# Title\n\nHello *world*\n";
        let (blocks, _) = parse_document(source);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].kind, heading(1, plain("Title")));
        assert_eq!(source[blocks[0].bytes.clone()].trim_end(), "# Title");
        assert_eq!(blocks[0].lines, 0..1);
        assert_eq!(
            blocks[1].kind,
            para(styled(
                "Hello world",
                vec![span(6..11, InlineStyle::Emphasis)]
            ))
        );
        assert_eq!(source[blocks[1].bytes.clone()].trim_end(), "Hello *world*");
        assert_eq!(blocks[1].lines, 2..3);
    }

    #[test]
    fn setext_headings_are_headings() {
        assert_eq!(kinds("Title\n=====\n"), vec![heading(1, plain("Title"))]);
    }

    #[test]
    fn task_lists_nest_and_record_checked_state() {
        let expected = BlockKind::List {
            start: None,
            items: vec![
                ListItem {
                    task: Some(true),
                    blocks: vec![para(plain("done"))],
                },
                ListItem {
                    task: Some(false),
                    blocks: vec![
                        para(plain("todo")),
                        BlockKind::List {
                            start: None,
                            items: vec![ListItem {
                                task: None,
                                blocks: vec![para(plain("nested"))],
                            }],
                        },
                    ],
                },
            ],
        };
        assert_eq!(
            kinds("- [x] done\n- [ ] todo\n  - nested\n"),
            vec![expected]
        );
    }

    #[test]
    fn loose_task_lists_keep_their_checkboxes() {
        let expected = BlockKind::List {
            start: None,
            items: vec![
                ListItem {
                    task: Some(true),
                    blocks: vec![para(plain("a"))],
                },
                ListItem {
                    task: Some(false),
                    blocks: vec![para(plain("b"))],
                },
            ],
        };
        assert_eq!(kinds("- [x] a\n\n- [ ] b\n"), vec![expected]);
    }

    #[test]
    fn ordered_lists_keep_their_start_number() {
        let kinds = kinds("3. a\n4. b\n");
        let [BlockKind::List { start, items }] = kinds.as_slice() else {
            panic!("expected one list, got {kinds:?}");
        };
        assert_eq!(*start, Some(3));
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn quotes_nest() {
        assert_eq!(
            kinds("> quote\n>\n> > inner\n"),
            vec![BlockKind::Quote(vec![
                para(plain("quote")),
                BlockKind::Quote(vec![para(plain("inner"))]),
            ])]
        );
    }

    #[test]
    fn fenced_and_indented_code_blocks() {
        assert_eq!(
            kinds("```rust\nfn main() {}\n```\n"),
            vec![BlockKind::Code {
                language: "rust".into(),
                text: "fn main() {}".into()
            }]
        );
        assert_eq!(
            kinds("    x = 1\n"),
            vec![BlockKind::Code {
                language: String::new(),
                text: "x = 1".into()
            }]
        );
    }

    #[test]
    fn tables_keep_alignment_head_and_styled_cells() {
        assert_eq!(
            kinds("| a | b |\n|:--|--:|\n| 1 | **2** |\n"),
            vec![BlockKind::Table {
                alignments: vec![CellAlign::Left, CellAlign::Right],
                head: vec![plain("a"), plain("b")],
                rows: vec![vec![
                    plain("1"),
                    styled("2", vec![span(0..1, InlineStyle::Strong)])
                ]],
            }]
        );
    }

    #[test]
    fn strikethrough_links_and_inline_code_become_spans() {
        assert_eq!(
            kinds("~~old~~ [site](https://x.dev) `code`\n"),
            vec![para(styled(
                "old site code",
                vec![
                    span(0..3, InlineStyle::Strikethrough),
                    span(4..8, InlineStyle::Link("https://x.dev".into())),
                    span(9..13, InlineStyle::Code),
                ],
            ))]
        );
    }

    #[test]
    fn markdown_images_flow_inline_with_their_title() {
        assert_eq!(
            kinds("![logo](img/logo.png)\n"),
            vec![para(with_images("\u{FFFC}", vec![(0, image("img/logo.png", "logo"))]))]
        );
        let titled = ImageRef {
            title: "Logo".into(),
            ..image("a.png", "logo")
        };
        assert_eq!(
            kinds("See ![logo](a.png \"Logo\") here\n"),
            vec![para(with_images("See \u{FFFC} here", vec![(4, titled)]))]
        );
    }

    #[test]
    fn badge_rows_are_linked_inline_images_in_markdown_and_html() {
        let expected = vec![para(RichText {
            spans: vec![
                span(0..1, InlineStyle::Link("https://a".into())),
                span(2..3, InlineStyle::Link("https://b".into())),
            ],
            ..with_images(
                "\u{FFFC} \u{FFFC}",
                vec![(0, image("a.svg", "a")), (2, image("b.svg", "b"))],
            )
        })];
        assert_eq!(
            kinds("[![a](a.svg)](https://a) [![b](b.svg)](https://b)\n"),
            expected
        );
        assert_eq!(
            kinds(
                "<a href=\"https://a\"><img src=\"a.svg\" alt=\"a\"></a> <a href=\"https://b\"><img src=\"b.svg\" alt=\"b\"></a>\n"
            ),
            expected
        );
    }

    #[test]
    fn span_offsets_are_utf16() {
        assert_eq!(
            kinds("**é😀**\n"),
            vec![para(styled("é😀", vec![span(0..3, InlineStyle::Strong)]))]
        );
    }

    #[test]
    fn slices_are_offset_by_their_base_position() {
        let blocks = parse_blocks("para\n", 100, 7, &[]);
        assert_eq!(blocks[0].bytes.start, 100);
        assert_eq!(blocks[0].lines, 7..8);
    }

    #[test]
    fn reference_definitions_are_collected_and_resolve_in_slices() {
        let (blocks, refdefs) = parse_document("[site]\n\n[Site]: https://x.dev\n");
        assert_eq!(refdefs.len(), 1);
        assert_eq!(refdefs[0].key, "site");
        assert_eq!(refdefs[0].dest, "https://x.dev");
        let link = para(styled(
            "site",
            vec![span(0..4, InlineStyle::Link("https://x.dev".into()))],
        ));
        assert_eq!(blocks[0].kind, link);
        assert_eq!(parse_blocks("[site]\n", 0, 0, &refdefs)[0].kind, link);
    }

    #[test]
    fn footnote_syntax_stays_literal() {
        assert_eq!(kinds("a[^1]\n"), vec![para(plain("a[^1]"))]);
    }

    #[test]
    fn a_centred_div_wraps_the_markdown_blocks_inside_it() {
        let source = "<div align=\"center\">\n\n# FastPad\n\nFast.\n\n</div>\n\nAfter\n";
        let (blocks, _) = parse_document(source);
        assert_eq!(
            blocks.iter().map(|block| block.kind.clone()).collect::<Vec<_>>(),
            vec![
                container(
                    TextAlign::Center,
                    vec![heading(1, plain("FastPad")), para(plain("Fast."))]
                ),
                para(plain("After")),
            ]
        );
        assert_eq!(blocks[0].lines, 0..7);
        assert!(source[blocks[0].bytes.clone()].trim_end().ends_with("</div>"));
    }

    #[test]
    fn rules_and_html_blocks() {
        assert_eq!(
            kinds("---\n\n<div>hi</div>\n"),
            vec![
                BlockKind::Rule,
                container(TextAlign::Inherit, vec![para(plain("hi"))])
            ]
        );
    }

    #[test]
    fn html_images_carry_their_attributes() {
        let expected = ImageRef {
            dest: "icon.svg".into(),
            alt: "FastPad".into(),
            title: "Logo".into(),
            width: Some(Length::Pixels(96)),
            height: Some(Length::Pixels(96)),
            sources: Vec::new(),
        };
        assert_eq!(
            kinds(
                "<img src=\"icon.svg\" alt=\"FastPad\" width=\"96\" height=\"96px\" title=\"Logo\">\n"
            ),
            vec![para(with_images("\u{FFFC}", vec![(0, expected)]))]
        );
        assert_eq!(kinds("<img alt=\"no source\">\n"), vec![]);
    }

    #[test]
    fn inline_html_styles_become_spans() {
        assert_eq!(
            kinds(
                "Press <kbd>Ctrl</kbd>+<kbd>S</kbd>, H<sub>2</sub>O, x<sup>2</sup>, <mark>hot</mark>, <ins>new</ins>, <small>fine</small>, <b>b</b><i>i</i><s>s</s><code>c</code>\n"
            ),
            vec![para(styled(
                "Press Ctrl+S, H2O, x2, hot, new, fine, bisc",
                vec![
                    span(6..10, InlineStyle::Keyboard),
                    span(11..12, InlineStyle::Keyboard),
                    span(15..16, InlineStyle::Subscript),
                    span(20..21, InlineStyle::Superscript),
                    span(23..26, InlineStyle::Mark),
                    span(28..31, InlineStyle::Underline),
                    span(33..37, InlineStyle::Small),
                    span(39..40, InlineStyle::Strong),
                    span(40..41, InlineStyle::Emphasis),
                    span(41..42, InlineStyle::Strikethrough),
                    span(42..43, InlineStyle::Code),
                ],
            ))]
        );
    }

    #[test]
    fn quotes_and_grouping_tags_keep_their_text() {
        assert_eq!(
            kinds("<q>hi</q> <cite>c</cite> <span>s</span> <abbr title=\"x\">a</abbr>\n"),
            vec![para(styled(
                "\u{201C}hi\u{201D} c s a",
                vec![span(5..6, InlineStyle::Emphasis)]
            ))]
        );
    }

    #[test]
    fn html_text_collapses_whitespace_and_br_breaks_lines() {
        assert_eq!(
            kinds("<p align=\"right\">\n  one   two<br>\n  three\n</p>\n"),
            vec![BlockKind::Paragraph {
                align: TextAlign::Right,
                text: plain("one two\nthree"),
            }]
        );
    }

    #[test]
    fn details_hold_a_summary_and_markdown_children() {
        assert_eq!(
            kinds(
                "<details>\n<summary><b>More</b> ways</summary>\n\n| a |\n|---|\n| 1<br>2 |\n\n</details>\n"
            ),
            vec![BlockKind::Details {
                open: false,
                summary: styled("More ways", vec![span(0..4, InlineStyle::Strong)]),
                children: vec![BlockKind::Table {
                    alignments: vec![CellAlign::None],
                    head: vec![plain("a")],
                    rows: vec![vec![plain("1\n2")]],
                }],
            }]
        );
    }

    #[test]
    fn details_can_start_open_and_default_their_summary() {
        assert_eq!(
            kinds("<details open>\n\nBody\n\n</details>\n"),
            vec![BlockKind::Details {
                open: true,
                summary: plain("Details"),
                children: vec![para(plain("Body"))],
            }]
        );
    }

    #[test]
    fn unclosed_tags_close_at_the_end_of_their_container_or_document() {
        let source = "<div align=\"center\">\n\nA\n\nB\n";
        let (blocks, _) = parse_document(source);
        assert_eq!(blocks.len(), 1);
        assert_eq!(
            blocks[0].kind,
            container(TextAlign::Center, vec![para(plain("A")), para(plain("B"))])
        );
        assert_eq!(blocks[0].bytes, 0..source.len());
        assert_eq!(
            kinds("> <div>\n> quoted\n\nafter\n"),
            vec![
                BlockKind::Quote(vec![container(
                    TextAlign::Inherit,
                    vec![para(plain("quoted"))]
                )]),
                para(plain("after")),
            ]
        );
    }

    #[test]
    fn stray_end_tags_are_ignored() {
        assert_eq!(kinds("</div></p>text</b>\n"), vec![para(plain("text"))]);
    }

    #[test]
    fn block_tags_imply_the_end_of_paragraphs_and_headings() {
        assert_eq!(
            kinds("<p>one<div>two</div>\n"),
            vec![container(
                TextAlign::Inherit,
                vec![
                    para(plain("one")),
                    container(TextAlign::Inherit, vec![para(plain("two"))]),
                ]
            )]
        );
        assert_eq!(
            kinds("<h1>a<h2>b</h2>\n"),
            vec![container(
                TextAlign::Inherit,
                vec![heading(1, plain("a")), heading(2, plain("b"))]
            )]
        );
        assert_eq!(kinds("<h7>\nx\n</h7>\n"), vec![heading(6, plain("x"))]);
    }

    #[test]
    fn block_tags_inside_markdown_inline_content_do_nothing() {
        assert_eq!(
            kinds("| <div>x</div> |\n|---|\n| <p>y |\n"),
            vec![BlockKind::Table {
                alignments: vec![CellAlign::None],
                head: vec![plain("x")],
                rows: vec![vec![plain("y")]],
            }]
        );
    }

    #[test]
    fn picture_sources_record_their_colour_scheme() {
        let source = "<picture>\n  <source media=\"(prefers-color-scheme: dark)\" srcset=\"dark.png 1x, dark@2x.png 2x\">\n  <source media=\"( PREFERS-COLOR-SCHEME : light )\" srcset=\"light.png\">\n  <source media=\"(min-width: 600px)\" srcset=\"wide.png\">\n  <img src=\"plain.png\" alt=\"Logo\">\n</picture>\n";
        let expected = ImageRef {
            sources: vec![
                ImageSource {
                    url: "dark.png".into(),
                    scheme: Some(ColorScheme::Dark),
                },
                ImageSource {
                    url: "light.png".into(),
                    scheme: Some(ColorScheme::Light),
                },
                ImageSource {
                    url: "wide.png".into(),
                    scheme: None,
                },
            ],
            ..image("plain.png", "Logo")
        };
        assert_eq!(
            kinds(source),
            vec![para(with_images("\u{FFFC}", vec![(0, expected)]))]
        );
    }

    #[test]
    fn scripts_are_removed_and_unknown_tags_keep_their_text() {
        assert_eq!(
            kinds("a <script>x</script> b <center>c</center>\n"),
            vec![para(plain("a  b c"))]
        );
        let (blocks, _) = parse_document("<script>\nalert(1)\n</script>\n\nafter\n");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind, para(plain("after")));
        assert_eq!(blocks[0].lines, 4..5);
    }

    #[test]
    fn comments_produce_no_block() {
        let (blocks, _) = parse_document("<!-- note -->\n\ntext\n");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind, para(plain("text")));
        assert_eq!(blocks[0].bytes.start, 15);
    }

    #[test]
    fn ids_and_named_links_anchor_headings() {
        assert_eq!(
            kinds("<h2 id=\"Install\">Install it</h2>\n\n## <a name=\"usage\"></a>Usage\n"),
            vec![
                BlockKind::Heading {
                    level: 2,
                    align: TextAlign::Inherit,
                    anchor: Some("Install".into()),
                    text: plain("Install it"),
                },
                BlockKind::Heading {
                    level: 2,
                    align: TextAlign::Inherit,
                    anchor: Some("usage".into()),
                    text: plain("Usage"),
                },
            ]
        );
    }

    #[test]
    fn sizes_alignments_and_media_parse_like_github() {
        assert_eq!(Length::parse("96"), Some(Length::Pixels(96)));
        assert_eq!(Length::parse(" 96px "), Some(Length::Pixels(96)));
        assert_eq!(Length::parse("50%"), Some(Length::Percent(50)));
        assert_eq!(Length::parse("12.7"), Some(Length::Pixels(12)));
        for invalid in ["0", "", "auto", "12em", ".5"] {
            assert_eq!(Length::parse(invalid), None, "{invalid}");
        }
        assert_eq!(TextAlign::parse("CENTER"), TextAlign::Center);
        assert_eq!(TextAlign::parse("middle"), TextAlign::Center);
        assert_eq!(TextAlign::parse("justify"), TextAlign::Justify);
        assert_eq!(TextAlign::parse("top"), TextAlign::Inherit);
        assert_eq!(
            ColorScheme::from_media("(prefers-color-scheme:dark)"),
            Some(ColorScheme::Dark)
        );
        assert_eq!(ColorScheme::from_media("print"), None);
    }
}
```

- [ ] **Step 2: Keep `layout.rs` compiling against the new model**

These are interim edits; Part 6 replaces the image and style handling.

1. Imports: `use crate::preview::model::{BlockKind, CellAlign, InlineStyle, ListItem, RichText};` (drop `ImageRef`; `resolve_image_path` becomes unused, so remove that import too).
2. `block_has_link`:

```rust
pub fn block_has_link(kind: &BlockKind) -> bool {
    let rich = |text: &RichText| {
        text.spans
            .iter()
            .any(|span| matches!(span.style, InlineStyle::Link(_)))
    };
    match kind {
        BlockKind::Heading { text, .. } | BlockKind::Paragraph { text, .. } => rich(text),
        BlockKind::List { items, .. } => items
            .iter()
            .any(|item| item.blocks.iter().any(block_has_link)),
        BlockKind::Quote(blocks)
        | BlockKind::Container {
            children: blocks, ..
        } => blocks.iter().any(block_has_link),
        BlockKind::Details {
            summary, children, ..
        } => rich(summary) || children.iter().any(block_has_link),
        BlockKind::Table { head, rows, .. } => {
            head.iter().any(rich) || rows.iter().flatten().any(rich)
        }
        BlockKind::Code { .. } | BlockKind::Rule => false,
    }
}
```

3. In `rich_layout`, delete the `InlineStyle::ImageAlt` arm and add `_ => Ok(()),` as the last arm (Part 6 implements the new styles).
4. In `layout_kind`: change `BlockKind::Paragraph(text)` to `BlockKind::Paragraph { text, .. }` and `BlockKind::Heading { level, text }` to `BlockKind::Heading { level, text, .. }`; delete the `BlockKind::Html(html)` and `BlockKind::Images(images)` arms; add:

```rust
        BlockKind::Container { children, .. } => Ok(Extent {
            content: layout_children(context, children, x, y, width, style, output)?,
            margin,
        }),
        BlockKind::Details {
            open,
            summary,
            children,
        } => push_details(
            context, *open, summary, children, x, y, width, style, output, margin,
        ),
```

5. Delete `push_images`, and add:

```rust
/// A `<details>` section: a disclosure triangle and the summary, then the children when open.
#[allow(clippy::too_many_arguments)]
fn push_details(
    context: &LayoutContext<'_>,
    open: bool,
    summary: &RichText,
    children: &[BlockKind],
    x: f32,
    y: f32,
    width: f32,
    style: Style,
    output: &mut Output,
    margin: f32,
) -> Result<Extent> {
    let unit = context.fonts.unit();
    let marker = context.plain_layout(
        if open { "\u{25BE}" } else { "\u{25B8}" },
        false,
        context.fonts.body_size,
        DWRITE_FONT_WEIGHT_NORMAL,
        100.0,
    )?;
    let marker_width = metrics(&marker)?.widthIncludingTrailingWhitespace + 4.0 * unit;
    let summary_height = push_rich_text(
        context,
        summary,
        context.fonts.body_size,
        DWRITE_FONT_WEIGHT_NORMAL,
        x + marker_width,
        y,
        (width - marker_width).max(1.0),
        style.role,
        false,
        output,
    )?;
    output.ops.push(DrawOp::Text {
        layout: marker,
        x,
        y,
        role: style.role,
    });
    let row_height = summary_height.max(context.line_height);
    let mut content = row_height;
    if open && !children.is_empty() {
        let indent = 16.0 * unit;
        let top = y + row_height + 8.0 * unit;
        let inner = layout_children(
            context,
            children,
            x + indent,
            top,
            (width - indent).max(1.0),
            style,
            output,
        )?;
        content = top - y + inner;
    }
    Ok(Extent { content, margin })
}
```

- [ ] **Step 3: Compile and run the model tests**

Run: `cargo clippy --all-targets --all-features -- -D warnings`
Expected: no warnings. (Layout image tests are knowingly stale until Part 6; do not run them yet.)

Run: `cargo test --lib preview::model`
Expected: PASS. If a structural expectation fails, print `kinds(source)` and fix the builder, not the expectation, unless the expectation contradicts GitHub's rendering of the same Markdown.

- [ ] **Step 4: Commit**

```bash
git add src/preview/model.rs src/preview/layout.rs
git commit -m "feat(preview): build blocks from GitHub's HTML subset with inline images"
```

---
## Part 5: Incremental reparsing with HTML

**Files:**
- Modify: `src/preview/incremental.rs` (tests only, unless the property test finds a divergence)

**Interfaces:**
- Consumes: `parse_document` / `parse_blocks` with HTML support (Part 4).
- Produces: no API change.

- [ ] **Step 1: Add HTML fixtures, HTML edits, and two targeted tests**

In the `tests` module of `src/preview/incremental.rs`, replace `FIXTURES` and `INSERTS` with:

```rust
    const FIXTURES: [&str; 8] = [
        "# Title\n\nFirst paragraph with *emphasis*.\n\nSecond paragraph.\n",
        "- one\n- two\n\n- three after a blank\n\n  continued item\n\nTail paragraph.\n",
        "> quote line\n> more\n\nlazy\n\n```rust\nfn main() {}\n\nlet x = 1;\n```\n\nafter code\n",
        "| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n\ntext\n\n    indented code\n\n    more code\n",
        "See [site] and [other][o].\n\n[site]: https://x.dev\n[o]: https://o.dev\n\nEnd.\n",
        "<!-- comment\n\nstill comment -->\n\nParagraph\n\n<div>\nhtml\n</div>\n",
        "<div align=\"center\">\n\n<img src=\"a.svg\" width=\"96\">\n\n# Title\n\n</div>\n\nText with <kbd>K</kbd>.\n\n<details>\n<summary>More</summary>\n\n- item\n\n</details>\n\nTail\n",
        "<p align=\"center\">\n  <a href=\"x\"><img src=\"b.png\"></a>\n</p>\n\n<picture>\n<source media=\"(prefers-color-scheme: dark)\" srcset=\"d.png\">\n<img src=\"l.png\">\n</picture>\n\n<!-- note -->\n\nEnd <br> line\n",
    ];

    const INSERTS: [&str; 28] = [
        "x",
        "\n",
        "\n\n",
        "```",
        "~~~",
        "- ",
        "1. ",
        "> ",
        "    ",
        "|",
        "---",
        "[a]: /u\n",
        "<!--",
        "-->",
        "**",
        "`",
        " ",
        "===\n",
        "é",
        "😀",
        "<",
        ">",
        "</div>",
        "<details>",
        "<summary>",
        "<div align=\"center\">",
        "</p>",
        "<br>",
    ];
```

and add:

```rust
    #[test]
    fn an_edit_inside_a_div_replaces_only_the_div() {
        let mut text = String::from("a\n\nb\n\nc\n\n<div>\n\none\n\ntwo\n\n</div>\n\nd\n\ne\n\nf\n");
        let mut document = PreviewDocument::parse(&text);
        assert_eq!(document.blocks.len(), 7);
        let position = text.find("two").unwrap();
        text.insert(position, 'x');
        let update = document.apply(
            text.as_str(),
            &[Edit {
                position,
                removed: 0,
                inserted: 1,
                lines_delta: 0,
            }],
        );
        assert_eq!(update, Update::Replaced { old: 3..4, new: 3..4 });
        assert_eq!(document.blocks, parse_document(&text).0);
    }

    #[test]
    fn deleting_a_closing_tag_lets_the_div_take_the_rest_of_the_document() {
        let mut text = String::from("<div>\n\none\n\n</div>\n\na\n\nb\n");
        let mut document = PreviewDocument::parse(&text);
        assert_eq!(document.blocks.len(), 3);
        let position = text.find("</div>").unwrap();
        text.replace_range(position..position + "</div>".len(), "");
        document.apply(
            text.as_str(),
            &[Edit {
                position,
                removed: "</div>".len(),
                inserted: 0,
                lines_delta: 0,
            }],
        );
        assert_eq!(document.blocks, parse_document(&text).0);
        assert_eq!(document.blocks.len(), 1);
    }
```

- [ ] **Step 2: Run the incremental tests**

Run: `cargo test --lib preview::incremental`
Expected: PASS.

If `incremental_updates_always_equal_a_full_parse` fails, it prints the fixture, seed, step, and text. Reproduce that text in a new focused test (full parse versus `apply` of the recorded edit), find which slice was accepted with a wrong result, and add the narrowest rule to `try_apply` that rejects it (returning `None` or widening), following superpowers:systematic-debugging. Never change rendering or the builder to hide a divergence. Keep the focused test as a regression test with a comment naming what it catches.

- [ ] **Step 3: Commit**

```bash
git add src/preview/incremental.rs
git commit -m "test(preview): cover HTML edits in the incremental reparse property test"
```

---
## Part 6: Layout, rendering, details interaction, and anchors

**Files:**
- Create: `src/preview/outline.rs`
- Modify: `src/preview/mod.rs`, `src/catppuccin.rs`, `src/preview/colors.rs`, `src/preview/layout.rs` (replace the whole file), `src/preview/render.rs`, `src/preview/view.rs`, `src/preview/accessible.rs` (test fixtures only)

**Interfaces:**
- Consumes: the Part 4 model; `inline_object::inline_box` (Part 3).
- Produces:
  - `outline::{DetailsKey { summary: String, occurrence: u32 }, Anchor { slug, block, heading: u32, enclosing: Vec<usize> }, Outline { details: Vec<Vec<DetailsKey>>, anchors: Vec<Anchor> }, build(&[Block]) -> Outline, count_nested(&[BlockKind]) -> (u32, usize), details_open_attribute(&BlockKind, usize) -> Option<bool>, has_details(&BlockKind) -> bool, summary_text(&str) -> String}`
  - `colors::ColorRole::{Mark, KbdBorder}`; `PreviewColors { mark, kbd_border, .. }`; `PreviewColors::is_dark(&self) -> bool`; `catppuccin::Flavor::yellow`
  - `layout::TargetKind::{Link(String), Disclosure { key: DetailsKey, expanded: bool }}`; `layout::Target { kind, text, rects, layout, range, scrolls, clip }` with `visible_rects(f32)` and `dest() -> Option<&str>` (replaces `LinkHit`); `layout::block_has_target` (replaces `block_has_link`); `LaidBlock { height, ops, targets, images, scroll_width, headings: Vec<(u32, f32)> }`; `ImageSlot { path, rect, alt, alt_origin: (f32, f32) }`; `DrawOp::RoundedStroke { rect, radius, role }`
  - `LayoutContext::new(graphics, brushes, fonts, document_dir, image_size, dark: bool, details: &HashMap<DetailsKey, bool>)`; `layout_block(context, kind, width, keys: &[DetailsKey])`
  - `view::VisibleLink { text, dest, rect, disclosure: Option<Disclosure>, focused: bool }`; `view::Disclosure { key: DetailsKey, expanded: bool }`; `PreviewStats::content_height: f32`

- [ ] **Step 1: Create `outline.rs`**

Register `pub mod outline;` in `src/preview/mod.rs`, then create `src/preview/outline.rs`:

```rust
//! Facts about nested blocks the view needs without laying anything out: which `<details>` section
//! is which, and where every heading anchor points, including headings inside collapsed sections.

use crate::preview::links::SlugSet;
use crate::preview::model::{Block, BlockKind, OBJECT_REPLACEMENT};
use std::collections::HashMap;

/// Identifies a `<details>` section across edits: its summary text and its index among sections
/// with the same summary, in document order. Editing the section's body keeps the key; editing its
/// summary starts a new one.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DetailsKey {
    pub summary: String,
    pub occurrence: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Anchor {
    pub slug: String,
    pub block: usize,
    /// The heading's index among its top-level block's headings, in document order.
    pub heading: u32,
    /// Indices into `Outline::details[block]` of the sections enclosing the heading.
    pub enclosing: Vec<usize>,
}

#[derive(Debug, Default)]
pub struct Outline {
    /// For each top-level block, the keys of its `<details>` sections in document order.
    pub details: Vec<Vec<DetailsKey>>,
    pub anchors: Vec<Anchor>,
}

#[derive(Default)]
struct Walk {
    slugs: SlugSet,
    occurrences: HashMap<String, u32>,
    anchors: Vec<Anchor>,
    headings: u32,
    enclosing: Vec<usize>,
}

pub fn build(blocks: &[Block]) -> Outline {
    let mut walk = Walk::default();
    let details = blocks
        .iter()
        .enumerate()
        .map(|(index, block)| {
            let mut keys = Vec::new();
            walk.headings = 0;
            visit(&block.kind, index, &mut walk, &mut keys);
            keys
        })
        .collect();
    Outline {
        details,
        anchors: walk.anchors,
    }
}

/// Visits in the order layout lays blocks out, so heading and section indices agree with it.
fn visit(kind: &BlockKind, block: usize, walk: &mut Walk, keys: &mut Vec<DetailsKey>) {
    match kind {
        BlockKind::Heading { text, anchor, .. } => {
            let heading = walk.headings;
            walk.headings += 1;
            let slug = walk.slugs.unique(text.plain_text());
            walk.anchors.push(Anchor {
                slug,
                block,
                heading,
                enclosing: walk.enclosing.clone(),
            });
            if let Some(anchor) = anchor {
                walk.anchors.push(Anchor {
                    slug: anchor.to_lowercase(),
                    block,
                    heading,
                    enclosing: walk.enclosing.clone(),
                });
            }
        }
        BlockKind::Details {
            summary, children, ..
        } => {
            let summary = summary_text(summary.plain_text());
            let count = walk.occurrences.entry(summary.clone()).or_insert(0);
            let key = DetailsKey {
                summary,
                occurrence: *count,
            };
            *count += 1;
            walk.enclosing.push(keys.len());
            keys.push(key);
            for child in children {
                visit(child, block, walk, keys);
            }
            walk.enclosing.pop();
        }
        BlockKind::Container { children, .. } | BlockKind::Quote(children) => {
            for child in children {
                visit(child, block, walk, keys);
            }
        }
        BlockKind::List { items, .. } => {
            for child in items.iter().flat_map(|item| &item.blocks) {
                visit(child, block, walk, keys);
            }
        }
        BlockKind::Paragraph { .. }
        | BlockKind::Code { .. }
        | BlockKind::Table { .. }
        | BlockKind::Rule => {}
    }
}

/// A summary's text as a key and an accessible name: inline images removed, whitespace trimmed.
pub fn summary_text(plain: &str) -> String {
    plain.replace(OBJECT_REPLACEMENT, "").trim().to_owned()
}

/// How many headings and `<details>` sections `blocks` hold at any depth. Layout skips a collapsed
/// section's children but still counts them, so later indices match `Outline`.
pub fn count_nested(blocks: &[BlockKind]) -> (u32, usize) {
    blocks.iter().fold((0, 0), |(headings, details), kind| {
        let (inner_headings, inner_details) = match kind {
            BlockKind::Heading { .. } => (1, 0),
            BlockKind::Details { children, .. } => {
                let (headings, details) = count_nested(children);
                (headings, details + 1)
            }
            BlockKind::Container { children, .. } | BlockKind::Quote(children) => {
                count_nested(children)
            }
            BlockKind::List { items, .. } => items.iter().fold((0, 0), |(h, d), item| {
                let (inner_h, inner_d) = count_nested(&item.blocks);
                (h + inner_h, d + inner_d)
            }),
            BlockKind::Paragraph { .. }
            | BlockKind::Code { .. }
            | BlockKind::Table { .. }
            | BlockKind::Rule => (0, 0),
        };
        (headings + inner_headings, details + inner_details)
    })
}

pub fn has_details(kind: &BlockKind) -> bool {
    count_nested(std::slice::from_ref(kind)).1 > 0
}

/// Whether section `index` (document order within `kind`) has the `open` attribute.
pub fn details_open_attribute(kind: &BlockKind, index: usize) -> Option<bool> {
    fn find(kind: &BlockKind, index: usize, seen: &mut usize) -> Option<bool> {
        match kind {
            BlockKind::Details { open, children, .. } => {
                if *seen == index {
                    return Some(*open);
                }
                *seen += 1;
                children.iter().find_map(|child| find(child, index, seen))
            }
            BlockKind::Container { children, .. } | BlockKind::Quote(children) => {
                children.iter().find_map(|child| find(child, index, seen))
            }
            BlockKind::List { items, .. } => items
                .iter()
                .flat_map(|item| &item.blocks)
                .find_map(|child| find(child, index, seen)),
            _ => None,
        }
    }
    find(kind, index, &mut 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::model::parse_document;

    fn key(summary: &str, occurrence: u32) -> DetailsKey {
        DetailsKey {
            summary: summary.into(),
            occurrence,
        }
    }

    #[test]
    fn details_keys_count_repeated_summaries_across_blocks_and_nesting() {
        let (blocks, _) = parse_document(
            "<details>\n<summary>More</summary>\n\n<details>\n<summary>More</summary>\n\nx\n\n</details>\n\n</details>\n\n<details>\n<summary>Other</summary>\n\ny\n\n</details>\n\n<details>\n<summary> More <img src=\"i.png\"></summary>\n\nz\n\n</details>\n",
        );
        assert_eq!(
            build(&blocks).details,
            vec![
                vec![key("More", 0), key("More", 1)],
                vec![key("Other", 0)],
                vec![key("More", 2)],
            ]
        );
    }

    #[test]
    fn anchors_include_headings_in_collapsed_sections_and_explicit_ids() {
        let (blocks, _) = parse_document(
            "# Intro\n\n<details>\n<summary>S</summary>\n\n## Intro\n\n</details>\n\n<h2 id=\"Setup\">Set up</h2>\n",
        );
        let outline = build(&blocks);
        let found = |slug: &str| {
            outline
                .anchors
                .iter()
                .find(|anchor| anchor.slug == slug)
                .cloned()
                .unwrap_or_else(|| panic!("no anchor {slug}"))
        };
        assert_eq!(
            found("intro"),
            Anchor {
                slug: "intro".into(),
                block: 0,
                heading: 0,
                enclosing: vec![],
            }
        );
        assert_eq!(
            found("intro-1"),
            Anchor {
                slug: "intro-1".into(),
                block: 1,
                heading: 0,
                enclosing: vec![0],
            }
        );
        assert_eq!(found("set-up").block, 2);
        assert_eq!(found("setup").block, 2);
    }

    #[test]
    fn nested_counts_and_open_attributes() {
        let (blocks, _) = parse_document(
            "<details open>\n<summary>A</summary>\n\n# H\n\n<details>\n<summary>B</summary>\n\n## I\n\n</details>\n\n</details>\n",
        );
        let BlockKind::Details { children, .. } = &blocks[0].kind else {
            panic!("expected a details block, got {:?}", blocks[0].kind);
        };
        assert_eq!(count_nested(children), (2, 1));
        assert_eq!(details_open_attribute(&blocks[0].kind, 0), Some(true));
        assert_eq!(details_open_attribute(&blocks[0].kind, 1), Some(false));
        assert_eq!(details_open_attribute(&blocks[0].kind, 2), None);
        assert!(has_details(&blocks[0].kind));
        assert!(!has_details(&parse_document("# x\n").0[0].kind));
    }
}
```

- [ ] **Step 2: Colours for `<mark>` and `<kbd>`, and theme darkness**

In `src/catppuccin.rs`, add `pub yellow: u32,` to `Flavor` after `peach`, and to each flavor after its `peach` line: `yellow: hex(0xDF8E1D),` (Latte), `yellow: hex(0xE5C890),` (Frappé), `yellow: hex(0xEED49F),` (Macchiato), `yellow: hex(0xF9E2AF),` (Mocha).

In `src/preview/colors.rs`:

1. Add `Mark` and `KbdBorder` after `Focus` in `ColorRole`, make `ALL` a `[ColorRole; 12]` ending with `Self::Focus, Self::Mark, Self::KbdBorder`.
2. Add `pub mark: u32,` and `pub kbd_border: u32,` to `PreviewColors`, and to `get`: `ColorRole::Mark => self.mark,` and `ColorRole::KbdBorder => self.kbd_border,`.
3. Add below `get`:

```rust
    /// Whether the preview background is dark, for `<picture>` sources that depend on the scheme.
    pub fn is_dark(&self) -> bool {
        let channel = |shift: u32| (self.background >> shift) & 0xFF;
        299 * channel(0) + 587 * channel(8) + 114 * channel(16) < 128_000
    }
```

4. Values: `GITHUB_LIGHT` gets `mark: rgb(255, 248, 197), kbd_border: rgb(209, 217, 224),`; `GITHUB_DARK` gets `mark: rgb(69, 56, 25), kbd_border: rgb(61, 68, 77),`; `catppuccin_colors` gets `mark: catppuccin::blend(flavor.yellow, flavor.base, 64), kbd_border: flavor.surface1,`; the high-contrast branch gets `mark: palette.editor_background, kbd_border: palette.editor_foreground,`.
5. Tests:

```rust
    #[test]
    fn mark_and_keyboard_roles_stand_out_on_every_theme() {
        for theme in Theme::ALL {
            let colors = preview_colors(theme, false);
            assert_ne!(colors.mark, colors.background, "{theme:?}");
            assert_ne!(colors.kbd_border, colors.background, "{theme:?}");
        }
    }

    #[test]
    fn darkness_follows_the_background() {
        assert!(!preview_colors(Theme::Light, false).is_dark());
        assert!(preview_colors(Theme::Dark, false).is_dark());
        assert!(preview_colors(Theme::CatppuccinMocha, false).is_dark());
        assert!(!preview_colors(Theme::CatppuccinLatte, false).is_dark());
    }
```

- [ ] **Step 3: Replace `src/preview/layout.rs`**

```rust
//! Blocks to positioned drawing operations using DirectWrite text layouts. A `LaidBlock` is valid
//! for one content width, one `PreviewFonts`, one `Brushes` (links and muted spans use brush drawing
//! effects), one colour scheme (`<picture>` sources), and one set of `<details>` states, so the view
//! discards layouts when any of those change.

use crate::Result;
use crate::preview::colors::ColorRole;
use crate::preview::dwrite::{Graphics, hresult_error};
use crate::preview::inline_object::inline_box;
use crate::preview::links::resolve_image_path;
use crate::preview::model::{
    BlockKind, CellAlign, ColorScheme, ImageRef, InlineStyle, Length, ListItem, OBJECT_REPLACEMENT,
    RichText, TextAlign,
};
use crate::preview::outline::{DetailsKey, count_nested, summary_text};
use crate::preview::render::{Brushes, RectF};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FONT_FEATURE, DWRITE_FONT_FEATURE_TAG_SUBSCRIPT, DWRITE_FONT_FEATURE_TAG_SUPERSCRIPT,
    DWRITE_FONT_STYLE_ITALIC, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_HIT_TEST_METRICS,
    DWRITE_LINE_METRICS, DWRITE_TEXT_ALIGNMENT, DWRITE_TEXT_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_JUSTIFIED, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING,
    DWRITE_TEXT_METRICS, DWRITE_TEXT_RANGE, DWRITE_WORD_WRAPPING_NO_WRAP, IDWriteTextFormat,
    IDWriteTextLayout,
};
use windows::core::PCWSTR;

const HEADING_SCALE: [f32; 6] = [2.0, 1.5, 1.25, 1.0, 0.875, 0.875];
const BULLETS: [&str; 3] = ["\u{2022}", "\u{25E6}", "\u{25AA}"];
const CODE_SCALE: f32 = 0.85;
/// `<small>`, relative to the surrounding text.
const SMALL_SCALE: f32 = 0.875;
/// `<sub>` and `<sup>`. DirectWrite cannot shift the baseline, so glyphs a font has no subscript or
/// superscript form for render smaller on the baseline.
const SCRIPT_SCALE: f32 = 0.75;

#[derive(Clone, Debug, PartialEq)]
pub struct PreviewFonts {
    pub body_family: String,
    pub code_family: String,
    /// Body text size in DIPs.
    pub body_size: f32,
}

impl PreviewFonts {
    pub fn from_settings(font_face: &str, font_size_points: u16) -> Self {
        Self {
            body_family: "Segoe UI".to_owned(),
            code_family: font_face.to_owned(),
            body_size: f32::from(font_size_points) * 96.0 / 72.0,
        }
    }

    /// GitHub's spacing is expressed for 16 px body text; everything scales with the body size.
    pub fn unit(&self) -> f32 {
        self.body_size / 16.0
    }
}

pub enum DrawOp {
    Text {
        layout: IDWriteTextLayout,
        x: f32,
        y: f32,
        role: ColorRole,
    },
    Fill {
        rect: RectF,
        role: ColorRole,
    },
    RoundedFill {
        rect: RectF,
        radius: f32,
        role: ColorRole,
    },
    RoundedStroke {
        rect: RectF,
        radius: f32,
        role: ColorRole,
    },
    Stroke {
        rect: RectF,
        role: ColorRole,
    },
    Checkbox {
        rect: RectF,
        checked: bool,
    },
    Image {
        slot: usize,
    },
    /// Content wider than the pane: drawn clipped to `clip`, shifted by the block's horizontal
    /// scroll offset.
    Scrollable {
        clip: RectF,
        content_width: f32,
        ops: Vec<DrawOp>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TargetKind {
    Link(String),
    /// A `<details>` summary row; `expanded` is the state it was laid out in.
    Disclosure { key: DetailsKey, expanded: bool },
}

/// Something the pointer, the keyboard, or assistive technology can activate.
pub struct Target {
    pub kind: TargetKind,
    /// The accessible name: the link text, or the summary text of a disclosure.
    pub text: String,
    pub rects: Vec<RectF>,
    pub layout: IDWriteTextLayout,
    pub range: DWRITE_TEXT_RANGE,
    /// Whether the rects move with the block's horizontal scroll offset.
    pub scrolls: bool,
    /// For scrolling targets, the block-coordinate area they are drawn clipped to.
    pub clip: Option<RectF>,
}

impl Target {
    /// The target's rectangles as currently shown, in block coordinates: shifted by the block's
    /// horizontal scroll offset and cut to its clip. Parts scrolled out of view are dropped.
    pub fn visible_rects(&self, h_offset: f32) -> Vec<RectF> {
        visible_link_rects(&self.rects, self.scrolls, self.clip, h_offset)
    }

    pub fn dest(&self) -> Option<&str> {
        match &self.kind {
            TargetKind::Link(dest) => Some(dest),
            TargetKind::Disclosure { .. } => None,
        }
    }
}

pub fn visible_link_rects(
    rects: &[RectF],
    scrolls: bool,
    clip: Option<RectF>,
    h_offset: f32,
) -> Vec<RectF> {
    if !scrolls {
        return rects.to_vec();
    }
    rects
        .iter()
        .filter_map(|rect| {
            let shifted = rect.offset(-h_offset, 0.0);
            match clip {
                Some(clip) => shifted.intersect(&clip),
                None => Some(shifted),
            }
        })
        .collect()
}

/// Whether a block holds a link or a disclosure anywhere, including nested lists, quotes, sections,
/// and table cells. Answers from the model alone, so keyboard navigation can skip blocks unlaid.
pub fn block_has_target(kind: &BlockKind) -> bool {
    let rich = |text: &RichText| {
        text.spans
            .iter()
            .any(|span| matches!(span.style, InlineStyle::Link(_)))
    };
    match kind {
        BlockKind::Heading { text, .. } | BlockKind::Paragraph { text, .. } => rich(text),
        BlockKind::List { items, .. } => items
            .iter()
            .any(|item| item.blocks.iter().any(block_has_target)),
        BlockKind::Quote(blocks)
        | BlockKind::Container {
            children: blocks, ..
        } => blocks.iter().any(block_has_target),
        BlockKind::Details { .. } => true,
        BlockKind::Table { head, rows, .. } => {
            head.iter().any(rich) || rows.iter().flatten().any(rich)
        }
        BlockKind::Code { .. } | BlockKind::Rule => false,
    }
}

pub struct ImageSlot {
    pub path: Option<PathBuf>,
    pub rect: RectF,
    /// Alt text, shown while the image is pending, remote, missing, or failed.
    pub alt: IDWriteTextLayout,
    /// Where `alt` is drawn, relative to the top-left corner of `rect`.
    pub alt_origin: (f32, f32),
}

pub struct LaidBlock {
    pub height: f32,
    pub ops: Vec<DrawOp>,
    pub targets: Vec<Target>,
    pub images: Vec<ImageSlot>,
    /// The widest horizontally scrollable content, or 0 when nothing scrolls.
    pub scroll_width: f32,
    /// Laid-out headings: their document-order index within the block and their y.
    pub headings: Vec<(u32, f32)>,
}

pub struct LayoutContext<'a> {
    graphics: &'a Graphics,
    brushes: &'a Brushes,
    fonts: &'a PreviewFonts,
    document_dir: Option<&'a Path>,
    image_size: &'a dyn Fn(&Path) -> Option<(u32, u32)>,
    /// The preview background is dark: `<picture>` prefers dark-scheme sources.
    dark: bool,
    /// `<details>` sections the user toggled away from their `open` attribute.
    details: &'a HashMap<DetailsKey, bool>,
    formats: RefCell<HashMap<(bool, u32, i32), IDWriteTextFormat>>,
    line_height: f32,
}

/// A text layout and the images placed on its `OBJECT_REPLACEMENT` characters.
struct RichLayout {
    layout: IDWriteTextLayout,
    images: Vec<ImageBox>,
}

/// An inline image's reserved size and what to draw in it.
struct ImageBox {
    position: u32,
    width: f32,
    height: f32,
    path: Option<PathBuf>,
    alt: String,
    /// No size is known and none is coming yet: a one-line chip labelled with the alt text.
    chip: bool,
}

impl<'a> LayoutContext<'a> {
    pub fn new(
        graphics: &'a Graphics,
        brushes: &'a Brushes,
        fonts: &'a PreviewFonts,
        document_dir: Option<&'a Path>,
        image_size: &'a dyn Fn(&Path) -> Option<(u32, u32)>,
        dark: bool,
        details: &'a HashMap<DetailsKey, bool>,
    ) -> Result<Self> {
        let mut context = Self {
            graphics,
            brushes,
            fonts,
            document_dir,
            image_size,
            dark,
            details,
            formats: RefCell::new(HashMap::new()),
            line_height: 0.0,
        };
        let sample = context.plain_layout(
            "Ag",
            false,
            fonts.body_size,
            DWRITE_FONT_WEIGHT_NORMAL,
            1000.0,
        )?;
        context.line_height = metrics(&sample)?.height;
        Ok(context)
    }

    pub fn line_height(&self) -> f32 {
        self.line_height
    }

    fn format(
        &self,
        code: bool,
        size: f32,
        weight: DWRITE_FONT_WEIGHT,
    ) -> Result<IDWriteTextFormat> {
        let key = (code, size.to_bits(), weight.0);
        if let Some(format) = self.formats.borrow().get(&key) {
            return Ok(format.clone());
        }
        let family = if code {
            &self.fonts.code_family
        } else {
            &self.fonts.body_family
        };
        let format = self
            .graphics
            .text_format(family, size, weight, DWRITE_FONT_STYLE_NORMAL)?;
        self.formats.borrow_mut().insert(key, format.clone());
        Ok(format)
    }

    fn plain_layout(
        &self,
        text: &str,
        code: bool,
        size: f32,
        weight: DWRITE_FONT_WEIGHT,
        width: f32,
    ) -> Result<IDWriteTextLayout> {
        let format = self.format(code, size, weight)?;
        let wide = text.encode_utf16().collect::<Vec<_>>();
        unsafe {
            self.graphics
                .dwrite
                .CreateTextLayout(&wide, &format, width.max(1.0), f32::MAX)
        }
        .map_err(hresult_error)
    }

    /// `available` resolves percentage image widths and caps every image's width.
    fn rich_layout(
        &self,
        rich: &RichText,
        size: f32,
        weight: DWRITE_FONT_WEIGHT,
        width: f32,
        available: f32,
    ) -> Result<RichLayout> {
        let layout = self.plain_layout(&rich.text, false, size, weight, width)?;
        let code_family = crate::platform::wide_null(&self.fonts.code_family);
        for span in &rich.spans {
            let range = text_range(span.range.start, span.range.end);
            unsafe {
                match &span.style {
                    InlineStyle::Strong => {
                        layout.SetFontWeight(DWRITE_FONT_WEIGHT_SEMI_BOLD, range)
                    }
                    InlineStyle::Emphasis => layout.SetFontStyle(DWRITE_FONT_STYLE_ITALIC, range),
                    InlineStyle::Strikethrough => layout.SetStrikethrough(true, range),
                    InlineStyle::Underline => layout.SetUnderline(true, range),
                    InlineStyle::Code | InlineStyle::Keyboard => layout
                        .SetFontFamilyName(PCWSTR(code_family.as_ptr()), range)
                        .and_then(|()| layout.SetFontSize(size * CODE_SCALE, range)),
                    InlineStyle::Small => layout.SetFontSize(size * SMALL_SCALE, range),
                    InlineStyle::Subscript | InlineStyle::Superscript => {
                        let feature = if matches!(span.style, InlineStyle::Subscript) {
                            DWRITE_FONT_FEATURE_TAG_SUBSCRIPT
                        } else {
                            DWRITE_FONT_FEATURE_TAG_SUPERSCRIPT
                        };
                        layout
                            .SetFontSize(size * SCRIPT_SCALE, range)
                            .and_then(|()| self.graphics.dwrite.CreateTypography())
                            .and_then(|typography| {
                                typography.AddFontFeature(DWRITE_FONT_FEATURE {
                                    nameTag: feature,
                                    parameter: 1,
                                })?;
                                layout.SetTypography(&typography, range)
                            })
                    }
                    InlineStyle::Link(_) => {
                        layout.SetDrawingEffect(self.brushes.get(ColorRole::Link), range)
                    }
                    // Drawn as a fill behind the text by `push_laid_text`.
                    InlineStyle::Mark => Ok(()),
                }
            }
            .map_err(hresult_error)?;
        }
        let mut images = Vec::with_capacity(rich.images.len());
        for inline in &rich.images {
            let image = self.image_box(&inline.image, inline.position, available)?;
            unsafe {
                layout.SetInlineObject(
                    &inline_box(image.width, image.height),
                    text_range(inline.position, inline.position + 1),
                )
            }
            .map_err(hresult_error)?;
            images.push(image);
        }
        Ok(RichLayout { layout, images })
    }

    /// The source a `<picture>` shows in the current colour scheme, else the first source without a
    /// scheme, else the image's own `src`.
    fn image_dest<'i>(&self, image: &'i ImageRef) -> &'i str {
        let wanted = if self.dark {
            ColorScheme::Dark
        } else {
            ColorScheme::Light
        };
        image
            .sources
            .iter()
            .find(|source| source.scheme == Some(wanted))
            .or_else(|| image.sources.iter().find(|source| source.scheme.is_none()))
            .map_or(image.dest.as_str(), |source| source.url.as_str())
    }

    /// Size in DIPs: explicit `width` and `height`; one of them completed from the natural aspect
    /// ratio; the natural size; or a placeholder. Never wider than `available`.
    fn image_box(&self, image: &ImageRef, position: u32, available: f32) -> Result<ImageBox> {
        let path = resolve_image_path(self.image_dest(image), self.document_dir);
        let natural = path
            .as_deref()
            .and_then(|path| (self.image_size)(path))
            .filter(|(width, height)| *width > 0 && *height > 0)
            .map(|(width, height)| (width as f32, height as f32));
        let alt = if image.alt.is_empty() {
            "image".to_owned()
        } else {
            image.alt.clone()
        };
        let width = image.width.map(|length| match length {
            Length::Pixels(value) => value as f32,
            Length::Percent(value) => available * value as f32 / 100.0,
        });
        let height = match image.height {
            Some(Length::Pixels(value)) => Some(value as f32),
            // A percentage height has no containing height to resolve against.
            Some(Length::Percent(_)) | None => None,
        };
        let (mut box_width, mut box_height, chip) = match (width, height, natural) {
            (Some(width), Some(height), _) => (width, height, false),
            (Some(width), None, Some((natural_width, natural_height))) => {
                (width, width * natural_height / natural_width, false)
            }
            (None, Some(height), Some((natural_width, natural_height))) => {
                (height * natural_width / natural_height, height, false)
            }
            (None, None, Some(size)) => (size.0, size.1, false),
            (Some(width), None, None) => (width, self.line_height, false),
            (None, Some(height), None) => (self.chip_width(&alt)?, height, false),
            (None, None, None) => (self.chip_width(&alt)?, self.line_height, true),
        };
        if box_width > available {
            box_height *= available / box_width;
            box_width = available;
        }
        Ok(ImageBox {
            position,
            width: box_width.max(1.0),
            height: box_height.max(1.0),
            path,
            alt,
            chip,
        })
    }

    fn chip_width(&self, alt: &str) -> Result<f32> {
        let layout = self.plain_layout(
            alt,
            false,
            self.fonts.body_size,
            DWRITE_FONT_WEIGHT_NORMAL,
            100_000.0,
        )?;
        Ok(metrics(&layout)?.widthIncludingTrailingWhitespace + 12.0 * self.fonts.unit())
    }
}

/// Everything laid out for one top-level block, plus the running document-order counters that
/// keep heading and section indices in step with `outline::build`.
struct Output<'k> {
    ops: Vec<DrawOp>,
    targets: Vec<Target>,
    images: Vec<ImageSlot>,
    scroll_width: f32,
    headings: Vec<(u32, f32)>,
    keys: &'k [DetailsKey],
    next_heading: u32,
    next_details: usize,
}

impl<'k> Output<'k> {
    fn new(keys: &'k [DetailsKey]) -> Self {
        Self {
            ops: Vec::new(),
            targets: Vec::new(),
            images: Vec::new(),
            scroll_width: 0.0,
            headings: Vec::new(),
            keys,
            next_heading: 0,
            next_details: 0,
        }
    }
}

/// A laid-out block's own height and the space it wants below it.
#[derive(Clone, Copy)]
struct Extent {
    content: f32,
    margin: f32,
}

#[derive(Clone, Copy)]
struct Style {
    role: ColorRole,
    list_depth: usize,
    nested: bool,
    /// Inherited alignment for inline content.
    align: TextAlign,
}

/// `keys` are the block's `<details>` keys in document order (`Outline::details[index]`); empty
/// for blocks without sections.
pub fn layout_block(
    context: &LayoutContext<'_>,
    kind: &BlockKind,
    width: f32,
    keys: &[DetailsKey],
) -> Result<LaidBlock> {
    let mut output = Output::new(keys);
    let style = Style {
        role: ColorRole::Text,
        list_depth: 0,
        nested: false,
        align: TextAlign::Inherit,
    };
    let extent = layout_kind(context, kind, 0.0, 0.0, width, style, &mut output)?;
    Ok(LaidBlock {
        height: extent.content + extent.margin,
        ops: output.ops,
        targets: output.targets,
        images: output.images,
        scroll_width: output.scroll_width,
        headings: output.headings,
    })
}

fn layout_kind(
    context: &LayoutContext<'_>,
    kind: &BlockKind,
    x: f32,
    y: f32,
    width: f32,
    style: Style,
    output: &mut Output<'_>,
) -> Result<Extent> {
    let unit = context.fonts.unit();
    let margin = if style.nested {
        4.0 * unit
    } else {
        16.0 * unit
    };
    match kind {
        BlockKind::Paragraph { align, text } => {
            let height = push_rich_text(
                context,
                text,
                context.fonts.body_size,
                DWRITE_FONT_WEIGHT_NORMAL,
                x,
                y,
                width,
                effective_align(*align, style.align),
                style.role,
                false,
                output,
            )?;
            Ok(Extent {
                content: height,
                margin,
            })
        }
        BlockKind::Heading {
            level, align, text, ..
        } => {
            output.headings.push((output.next_heading, y));
            output.next_heading += 1;
            let size = context.fonts.body_size * HEADING_SCALE[usize::from(*level).clamp(1, 6) - 1];
            let role = match (style.role, *level) {
                (ColorRole::Text, 5 | 6) => ColorRole::Muted,
                (ColorRole::Text, _) => ColorRole::Heading,
                (role, _) => role,
            };
            let top = y + 8.0 * unit;
            let mut bottom = top
                + push_rich_text(
                    context,
                    text,
                    size,
                    DWRITE_FONT_WEIGHT_SEMI_BOLD,
                    x,
                    top,
                    width,
                    effective_align(*align, style.align),
                    role,
                    false,
                    output,
                )?;
            if *level <= 2 {
                bottom += 0.3 * size;
                output.ops.push(DrawOp::Fill {
                    rect: RectF::new(x, bottom, x + width, bottom + 1.0),
                    role: ColorRole::Border,
                });
                bottom += 1.0;
            }
            Ok(Extent {
                content: bottom - y,
                margin,
            })
        }
        BlockKind::Code { text, .. } => push_code(context, text, x, y, width, output, margin),
        BlockKind::Rule => {
            output.ops.push(DrawOp::Fill {
                rect: RectF::new(x, y + 8.0 * unit, x + width, y + 8.0 * unit + 1.0),
                role: ColorRole::Border,
            });
            Ok(Extent {
                content: 16.0 * unit + 1.0,
                margin: 8.0 * unit,
            })
        }
        BlockKind::Quote(blocks) => {
            let bar_index = output.ops.len();
            let indent = 16.0 * unit;
            let inner = Style {
                role: ColorRole::Muted,
                ..style
            };
            let content = layout_children(
                context,
                blocks,
                x + indent,
                y,
                width - indent,
                inner,
                output,
            )?;
            output.ops.insert(
                bar_index,
                DrawOp::Fill {
                    rect: RectF::new(x, y, x + 4.0 * unit, y + content),
                    role: ColorRole::QuoteBar,
                },
            );
            Ok(Extent { content, margin })
        }
        BlockKind::List { start, items } => {
            push_list(context, *start, items, x, y, width, style, output, margin)
        }
        BlockKind::Table {
            alignments,
            head,
            rows,
        } => push_table(
            context, alignments, head, rows, x, y, width, style.role, output, margin,
        ),
        BlockKind::Container { align, children } => {
            let inner = Style {
                align: effective_align(*align, style.align),
                ..style
            };
            let content = layout_children(context, children, x, y, width, inner, output)?;
            Ok(Extent { content, margin })
        }
        BlockKind::Details {
            open,
            summary,
            children,
        } => push_details(
            context, *open, summary, children, x, y, width, style, output, margin,
        ),
    }
}

fn layout_children(
    context: &LayoutContext<'_>,
    blocks: &[BlockKind],
    x: f32,
    y: f32,
    width: f32,
    style: Style,
    output: &mut Output<'_>,
) -> Result<f32> {
    let mut cursor = y;
    let mut last_margin = 0.0;
    for block in blocks {
        let extent = layout_kind(context, block, x, cursor, width, style, output)?;
        cursor += extent.content + extent.margin;
        last_margin = extent.margin;
    }
    Ok((cursor - y - last_margin).max(0.0))
}

#[allow(clippy::too_many_arguments)]
fn push_rich_text(
    context: &LayoutContext<'_>,
    text: &RichText,
    size: f32,
    weight: DWRITE_FONT_WEIGHT,
    x: f32,
    y: f32,
    width: f32,
    align: TextAlign,
    role: ColorRole,
    scrolls: bool,
    output: &mut Output<'_>,
) -> Result<f32> {
    let rich = context.rich_layout(text, size, weight, width, width)?;
    unsafe { rich.layout.SetTextAlignment(text_alignment(align)) }.map_err(hresult_error)?;
    push_laid_text(context, text, rich, x, y, role, scrolls, output)
}

#[allow(clippy::too_many_arguments)]
fn push_laid_text(
    context: &LayoutContext<'_>,
    text: &RichText,
    rich: RichLayout,
    x: f32,
    y: f32,
    role: ColorRole,
    scrolls: bool,
    output: &mut Output<'_>,
) -> Result<f32> {
    let unit = context.fonts.unit();
    let RichLayout { layout, images } = rich;
    for span in &text.spans {
        match &span.style {
            InlineStyle::Code => {
                for rect in range_rects(&layout, span.range.start, span.range.end, x, y)? {
                    output.ops.push(DrawOp::RoundedFill {
                        rect: rect.inflate(2.0 * unit),
                        radius: 4.0 * unit,
                        role: ColorRole::CodeBackground,
                    });
                }
            }
            InlineStyle::Keyboard => {
                for rect in range_rects(&layout, span.range.start, span.range.end, x, y)? {
                    let rect = rect.inflate(2.0 * unit);
                    output.ops.push(DrawOp::RoundedFill {
                        rect,
                        radius: 4.0 * unit,
                        role: ColorRole::CodeBackground,
                    });
                    output.ops.push(DrawOp::RoundedStroke {
                        rect,
                        radius: 4.0 * unit,
                        role: ColorRole::KbdBorder,
                    });
                }
            }
            InlineStyle::Mark => {
                for rect in range_rects(&layout, span.range.start, span.range.end, x, y)? {
                    output.ops.push(DrawOp::RoundedFill {
                        rect: rect.inflate(unit),
                        radius: 2.0 * unit,
                        role: ColorRole::Mark,
                    });
                }
            }
            InlineStyle::Link(dest) => output.targets.push(Target {
                kind: TargetKind::Link(dest.clone()),
                text: link_name(text, &span.range, dest),
                rects: range_rects(&layout, span.range.start, span.range.end, x, y)?,
                layout: layout.clone(),
                range: text_range(span.range.start, span.range.end),
                scrolls,
                clip: None,
            }),
            _ => {}
        }
    }
    let height = metrics(&layout)?.height;
    if !images.is_empty() {
        let lines = line_metrics(&layout)?;
        for image in images {
            push_image_slot(context, &layout, &lines, image, x, y, output)?;
        }
    }
    output.ops.push(DrawOp::Text { layout, x, y, role });
    Ok(height)
}

/// A link's accessible name: its visible text, else its images' alt text, else their title, else
/// the destination.
fn link_name(text: &RichText, range: &Range<u32>, dest: &str) -> String {
    let units = text.text.encode_utf16().collect::<Vec<_>>();
    let visible = units
        .get(range.start as usize..range.end as usize)
        .map(String::from_utf16_lossy)
        .unwrap_or_default()
        .replace(OBJECT_REPLACEMENT, "");
    let visible = visible.trim();
    if !visible.is_empty() {
        return visible.to_owned();
    }
    let images = || {
        text.images
            .iter()
            .filter(|image| range.contains(&image.position))
    };
    let alts = images()
        .map(|image| image.image.alt.as_str())
        .filter(|alt| !alt.is_empty())
        .collect::<Vec<_>>();
    if !alts.is_empty() {
        return alts.join(" ");
    }
    images()
        .map(|image| image.image.title.as_str())
        .find(|title| !title.is_empty())
        .unwrap_or(dest)
        .to_owned()
}

/// Places an image where DirectWrite put its inline object: bottom edge on the line's baseline.
fn push_image_slot(
    context: &LayoutContext<'_>,
    layout: &IDWriteTextLayout,
    lines: &[DWRITE_LINE_METRICS],
    image: ImageBox,
    x: f32,
    y: f32,
    output: &mut Output<'_>,
) -> Result<()> {
    let unit = context.fonts.unit();
    let (mut point_x, mut point_y) = (0.0, 0.0);
    let mut hit = DWRITE_HIT_TEST_METRICS::default();
    unsafe { layout.HitTestTextPosition(image.position, false, &mut point_x, &mut point_y, &mut hit) }
        .map_err(hresult_error)?;
    let (line_top, baseline) = line_of(lines, image.position);
    let left = x + hit.left;
    let bottom = y + line_top + baseline;
    let rect = RectF::new(left, bottom - image.height, left + image.width, bottom);
    let padding = if image.chip { 6.0 * unit } else { 8.0 * unit };
    let alt = context.plain_layout(
        &image.alt,
        false,
        context.fonts.body_size,
        DWRITE_FONT_WEIGHT_NORMAL,
        (image.width - 2.0 * padding).max(1.0),
    )?;
    let alt_top = if image.chip {
        unsafe { alt.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP) }.map_err(hresult_error)?;
        ((image.height - metrics(&alt)?.height) / 2.0).max(0.0)
    } else {
        padding
    };
    output.images.push(ImageSlot {
        path: image.path,
        rect,
        alt,
        alt_origin: (padding, alt_top),
    });
    output.ops.push(DrawOp::Image {
        slot: output.images.len() - 1,
    });
    Ok(())
}

fn line_metrics(layout: &IDWriteTextLayout) -> Result<Vec<DWRITE_LINE_METRICS>> {
    let mut count = 0_u32;
    // The first call only reports how many lines there are.
    let _ = unsafe { layout.GetLineMetrics(None, &mut count) };
    let mut lines = vec![DWRITE_LINE_METRICS::default(); count as usize];
    unsafe { layout.GetLineMetrics(Some(&mut lines), &mut count) }.map_err(hresult_error)?;
    lines.truncate(count as usize);
    Ok(lines)
}

/// The top and baseline offset of the line holding UTF-16 `position`.
fn line_of(lines: &[DWRITE_LINE_METRICS], position: u32) -> (f32, f32) {
    let mut top = 0.0;
    let mut start = 0;
    for line in lines {
        if position < start + line.length {
            return (top, line.baseline);
        }
        start += line.length;
        top += line.height;
    }
    lines
        .last()
        .map_or((0.0, 0.0), |line| (top - line.height, line.baseline))
}

fn push_code(
    context: &LayoutContext<'_>,
    text: &str,
    x: f32,
    y: f32,
    width: f32,
    output: &mut Output<'_>,
    margin: f32,
) -> Result<Extent> {
    let unit = context.fonts.unit();
    let padding = 16.0 * unit;
    let layout = context.plain_layout(
        text,
        true,
        context.fonts.body_size * CODE_SCALE,
        DWRITE_FONT_WEIGHT_NORMAL,
        width,
    )?;
    unsafe { layout.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP) }.map_err(hresult_error)?;
    let text_metrics = metrics(&layout)?;
    let height = text_metrics.height.max(context.line_height) + 2.0 * padding;
    let rect = RectF::new(x, y, x + width, y + height);
    output.ops.push(DrawOp::RoundedFill {
        rect,
        radius: 6.0 * unit,
        role: ColorRole::CodeBackground,
    });
    let content_width = text_metrics.widthIncludingTrailingWhitespace + 2.0 * padding;
    let text_op = DrawOp::Text {
        layout,
        x: x + padding,
        y: y + padding,
        role: ColorRole::Text,
    };
    if content_width > width {
        output.scroll_width = output.scroll_width.max(content_width);
        output.ops.push(DrawOp::Scrollable {
            clip: rect,
            content_width,
            ops: vec![text_op],
        });
    } else {
        output.ops.push(text_op);
    }
    Ok(Extent {
        content: height,
        margin,
    })
}

#[allow(clippy::too_many_arguments)]
fn push_list(
    context: &LayoutContext<'_>,
    start: Option<u64>,
    items: &[ListItem],
    x: f32,
    y: f32,
    width: f32,
    style: Style,
    output: &mut Output<'_>,
    margin: f32,
) -> Result<Extent> {
    let unit = context.fonts.unit();
    let indent = 32.0 * unit;
    let item_gap = 4.0 * unit;
    let mut cursor = y;
    for (index, item) in items.iter().enumerate() {
        if let Some(checked) = item.task {
            let size = 14.0 * unit;
            let top = cursor + (context.line_height - size) / 2.0;
            output.ops.push(DrawOp::Checkbox {
                rect: RectF::new(
                    x + indent - size - 8.0 * unit,
                    top,
                    x + indent - 8.0 * unit,
                    top + size,
                ),
                checked,
            });
        } else {
            let marker = match start {
                Some(first) => format!("{}.", first + index as u64),
                None => BULLETS[style.list_depth % BULLETS.len()].to_owned(),
            };
            let layout = context.plain_layout(
                &marker,
                false,
                context.fonts.body_size,
                DWRITE_FONT_WEIGHT_NORMAL,
                indent - 8.0 * unit,
            )?;
            unsafe { layout.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING) }
                .map_err(hresult_error)?;
            output.ops.push(DrawOp::Text {
                layout,
                x,
                y: cursor,
                role: style.role,
            });
        }
        let inner = Style {
            role: style.role,
            list_depth: style.list_depth + 1,
            nested: true,
            align: style.align,
        };
        let content = layout_children(
            context,
            &item.blocks,
            x + indent,
            cursor,
            width - indent,
            inner,
            output,
        )?;
        cursor += content.max(context.line_height) + item_gap;
    }
    Ok(Extent {
        content: (cursor - y - item_gap).max(0.0),
        margin: if style.nested { 0.0 } else { margin },
    })
}

#[allow(clippy::too_many_arguments)]
fn push_table(
    context: &LayoutContext<'_>,
    alignments: &[CellAlign],
    head: &[RichText],
    rows: &[Vec<RichText>],
    x: f32,
    y: f32,
    width: f32,
    role: ColorRole,
    output: &mut Output<'_>,
    margin: f32,
) -> Result<Extent> {
    let unit = context.fonts.unit();
    let (pad_x, pad_y) = (13.0 * unit, 6.0 * unit);
    let columns = alignments
        .len()
        .max(head.len())
        .max(rows.iter().map(Vec::len).max().unwrap_or(0));
    let all_rows = std::iter::once(head)
        .chain(rows.iter().map(Vec::as_slice))
        .collect::<Vec<_>>();
    let empty = RichText::default();
    let mut layouts = Vec::with_capacity(all_rows.len());
    let mut column_widths = vec![0.0_f32; columns];
    for (row_index, row) in all_rows.iter().enumerate() {
        let weight = if row_index == 0 {
            DWRITE_FONT_WEIGHT_SEMI_BOLD
        } else {
            DWRITE_FONT_WEIGHT_NORMAL
        };
        let mut row_layouts = Vec::with_capacity(columns);
        for (column, column_width) in column_widths.iter_mut().enumerate() {
            let cell = row.get(column).unwrap_or(&empty);
            let rich = context.rich_layout(cell, context.fonts.body_size, weight, 100_000.0, width)?;
            let natural = metrics(&rich.layout)?.widthIncludingTrailingWhitespace;
            *column_width = column_width.max(natural + 2.0 * pad_x);
            row_layouts.push(rich);
        }
        layouts.push(row_layouts);
    }
    let table_width: f32 = column_widths.iter().sum();
    let scrolls = table_width > width;
    let mut table_output = Output::new(&[]);
    let mut cursor = y;
    for (row_index, row_layouts) in layouts.into_iter().enumerate() {
        let mut row_height = context.line_height;
        for (column, rich) in row_layouts.iter().enumerate() {
            let inner = column_widths[column] - 2.0 * pad_x;
            unsafe {
                rich.layout.SetMaxWidth(inner.max(1.0)).map_err(hresult_error)?;
                rich.layout
                    .SetTextAlignment(alignment(
                        alignments.get(column).copied().unwrap_or(CellAlign::None),
                    ))
                    .map_err(hresult_error)?;
            }
            row_height = row_height.max(metrics(&rich.layout)?.height);
        }
        row_height += 2.0 * pad_y;
        if row_index > 0 && row_index % 2 == 0 {
            table_output.ops.push(DrawOp::Fill {
                rect: RectF::new(x, cursor, x + table_width, cursor + row_height),
                role: ColorRole::TableStripe,
            });
        }
        let mut cell_x = x;
        for (column, rich) in row_layouts.into_iter().enumerate() {
            let cell_rect = RectF::new(
                cell_x,
                cursor,
                cell_x + column_widths[column],
                cursor + row_height,
            );
            table_output.ops.push(DrawOp::Stroke {
                rect: cell_rect,
                role: ColorRole::Border,
            });
            let cell = all_rows[row_index].get(column).unwrap_or(&empty);
            push_laid_text(
                context,
                cell,
                rich,
                cell_x + pad_x,
                cursor + pad_y,
                role,
                scrolls,
                &mut table_output,
            )?;
            cell_x += column_widths[column];
        }
        cursor += row_height;
    }
    let clip = RectF::new(x, y, x + width, cursor);
    if scrolls {
        for target in &mut table_output.targets {
            target.clip = Some(clip);
        }
    }
    output.targets.append(&mut table_output.targets);
    // Cell images are numbered within the table; renumber them into the block's slots.
    let base = output.images.len();
    output.images.append(&mut table_output.images);
    for op in &mut table_output.ops {
        if let DrawOp::Image { slot } = op {
            *slot += base;
        }
    }
    if scrolls {
        output.scroll_width = output.scroll_width.max(table_width);
        output.ops.push(DrawOp::Scrollable {
            clip,
            content_width: table_width,
            ops: table_output.ops,
        });
    } else {
        output.ops.append(&mut table_output.ops);
    }
    Ok(Extent {
        content: cursor - y,
        margin,
    })
}

/// A `<details>` section: a disclosure row (triangle and summary), then its children when open.
/// The row is a disclosure target; the children of a collapsed section are counted, not laid out.
#[allow(clippy::too_many_arguments)]
fn push_details(
    context: &LayoutContext<'_>,
    open_attribute: bool,
    summary: &RichText,
    children: &[BlockKind],
    x: f32,
    y: f32,
    width: f32,
    style: Style,
    output: &mut Output<'_>,
    margin: f32,
) -> Result<Extent> {
    let unit = context.fonts.unit();
    let key = output.keys.get(output.next_details).cloned();
    output.next_details += 1;
    let open = key
        .as_ref()
        .and_then(|key| context.details.get(key))
        .copied()
        .unwrap_or(open_attribute);
    let marker = context.plain_layout(
        if open { "\u{25BE}" } else { "\u{25B8}" },
        false,
        context.fonts.body_size,
        DWRITE_FONT_WEIGHT_NORMAL,
        100.0,
    )?;
    let marker_width = metrics(&marker)?.widthIncludingTrailingWhitespace + 4.0 * unit;
    // The row's disclosure comes before links inside the summary in keyboard order.
    let target_index = output.targets.len();
    let summary_height = push_rich_text(
        context,
        summary,
        context.fonts.body_size,
        DWRITE_FONT_WEIGHT_NORMAL,
        x + marker_width,
        y,
        (width - marker_width).max(1.0),
        TextAlign::Inherit,
        style.role,
        false,
        output,
    )?;
    let row_height = summary_height.max(context.line_height);
    if let Some(key) = key {
        output.targets.insert(
            target_index,
            Target {
                kind: TargetKind::Disclosure {
                    key,
                    expanded: open,
                },
                text: summary_text(summary.plain_text()),
                rects: vec![RectF::new(x, y, x + width, y + row_height)],
                layout: marker.clone(),
                range: text_range(0, 1),
                scrolls: false,
                clip: None,
            },
        );
    }
    output.ops.push(DrawOp::Text {
        layout: marker,
        x,
        y,
        role: style.role,
    });
    let mut content = row_height;
    if open {
        if !children.is_empty() {
            let indent = 16.0 * unit;
            let top = y + row_height + 8.0 * unit;
            let inner = layout_children(
                context,
                children,
                x + indent,
                top,
                (width - indent).max(1.0),
                style,
                output,
            )?;
            content = top - y + inner;
        }
    } else {
        let (headings, details) = count_nested(children);
        output.next_heading += headings;
        output.next_details += details;
    }
    Ok(Extent { content, margin })
}

fn alignment(align: CellAlign) -> DWRITE_TEXT_ALIGNMENT {
    match align {
        CellAlign::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
        CellAlign::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
        CellAlign::None | CellAlign::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
    }
}

fn text_alignment(align: TextAlign) -> DWRITE_TEXT_ALIGNMENT {
    match align {
        TextAlign::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
        TextAlign::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
        TextAlign::Justify => DWRITE_TEXT_ALIGNMENT_JUSTIFIED,
        TextAlign::Inherit | TextAlign::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
    }
}

/// A block's own alignment wins over the one it inherits.
fn effective_align(own: TextAlign, inherited: TextAlign) -> TextAlign {
    if own == TextAlign::Inherit {
        inherited
    } else {
        own
    }
}

fn text_range(start: u32, end: u32) -> DWRITE_TEXT_RANGE {
    DWRITE_TEXT_RANGE {
        startPosition: start,
        length: end.saturating_sub(start),
    }
}

fn metrics(layout: &IDWriteTextLayout) -> Result<DWRITE_TEXT_METRICS> {
    let mut value = DWRITE_TEXT_METRICS::default();
    unsafe { layout.GetMetrics(&mut value) }.map_err(hresult_error)?;
    Ok(value)
}

fn range_rects(
    layout: &IDWriteTextLayout,
    start: u32,
    end: u32,
    x: f32,
    y: f32,
) -> Result<Vec<RectF>> {
    let length = end.saturating_sub(start);
    if length == 0 {
        return Ok(Vec::new());
    }
    let mut count = 0_u32;
    // The first call only reports how many rectangles the range needs.
    let _ = unsafe { layout.HitTestTextRange(start, length, x, y, None, &mut count) };
    let mut hits = vec![DWRITE_HIT_TEST_METRICS::default(); count as usize];
    unsafe { layout.HitTestTextRange(start, length, x, y, Some(&mut hits), &mut count) }
        .map_err(hresult_error)?;
    Ok(hits
        .iter()
        .take(count as usize)
        .map(|hit| {
            RectF::new(
                hit.left,
                hit.top,
                hit.left + hit.width,
                hit.top + hit.height,
            )
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::theme::Theme;
    use crate::preview::colors::preview_colors;
    use crate::preview::model::parse_document;
    use crate::preview::outline;
    use crate::preview::render::{TestWindow, create_hwnd_target};
    use windows::core::BOOL;

    fn with_setup<R>(
        image_size: &dyn Fn(&Path) -> Option<(u32, u32)>,
        document_dir: Option<&Path>,
        dark: bool,
        details: &HashMap<DetailsKey, bool>,
        test: impl FnOnce(&LayoutContext<'_>) -> R,
    ) -> R {
        let graphics = Graphics::load().unwrap();
        let window = TestWindow::new(400, 300);
        let target = create_hwnd_target(&graphics, window.0, 400, 300, 96).unwrap();
        let brushes = Brushes::create(&target, &preview_colors(Theme::Light, false)).unwrap();
        let fonts = PreviewFonts::from_settings("Consolas", 12);
        let context = LayoutContext::new(
            &graphics,
            &brushes,
            &fonts,
            document_dir,
            image_size,
            dark,
            details,
        )
        .unwrap();
        test(&context)
    }

    fn with_context<R>(
        image_size: &dyn Fn(&Path) -> Option<(u32, u32)>,
        document_dir: Option<&Path>,
        test: impl FnOnce(&LayoutContext<'_>) -> R,
    ) -> R {
        with_setup(image_size, document_dir, false, &HashMap::new(), test)
    }

    fn no_images(_: &Path) -> Option<(u32, u32)> {
        None
    }

    fn laid(context: &LayoutContext<'_>, source: &str, width: f32) -> LaidBlock {
        let (blocks, _) = parse_document(source);
        let outline = outline::build(&blocks);
        layout_block(context, &blocks[0].kind, width, &outline.details[0]).unwrap()
    }

    fn flatten(ops: &[DrawOp]) -> Vec<&DrawOp> {
        ops.iter()
            .flat_map(|op| match op {
                DrawOp::Scrollable { ops, .. } => {
                    let mut nested = vec![op];
                    nested.extend(flatten(ops));
                    nested
                }
                _ => vec![op],
            })
            .collect()
    }

    fn first_text_layout(block: &LaidBlock) -> (IDWriteTextLayout, f32) {
        flatten(&block.ops)
            .into_iter()
            .find_map(|op| match op {
                DrawOp::Text { layout, x, .. } => Some((layout.clone(), *x)),
                _ => None,
            })
            .expect("a text op")
    }

    fn first_glyph_x(block: &LaidBlock) -> f32 {
        let (layout, x) = first_text_layout(block);
        let (mut point_x, mut point_y) = (0.0, 0.0);
        let mut hit = DWRITE_HIT_TEST_METRICS::default();
        unsafe { layout.HitTestTextPosition(0, false, &mut point_x, &mut point_y, &mut hit) }
            .unwrap();
        x + point_x
    }

    #[test]
    fn headings_are_taller_than_paragraphs_with_the_same_text() {
        with_context(&no_images, None, |context| {
            let heading = laid(context, "# Same\n", 400.0);
            let paragraph = laid(context, "Same\n", 400.0);
            assert!(heading.height > paragraph.height);
            assert_eq!(heading.headings, vec![(0, 0.0)]);
        });
    }

    #[test]
    fn narrow_widths_wrap_paragraphs() {
        with_context(&no_images, None, |context| {
            let text = "word ".repeat(60) + "\n";
            assert!(laid(context, &text, 100.0).height > laid(context, &text, 1000.0).height);
        });
    }

    #[test]
    fn links_get_hit_rectangles() {
        with_context(&no_images, None, |context| {
            let block = laid(context, "go [here](https://x.dev)\n", 400.0);
            assert_eq!(block.targets.len(), 1);
            assert_eq!(block.targets[0].dest(), Some("https://x.dev"));
            assert_eq!(block.targets[0].text, "here");
            let rect = block.targets[0].rects[0];
            assert!(rect.left > 0.0 && rect.width() > 0.0 && rect.height() > 0.0);
            assert!(!block.targets[0].scrolls);
        });
    }

    #[test]
    fn wide_tables_and_long_code_lines_scroll_horizontally() {
        with_context(&no_images, None, |context| {
            let wide = "wide ".repeat(30);
            let table = laid(
                context,
                &format!("| {wide} | {wide} |\n|---|---|\n| a | b |\n"),
                200.0,
            );
            assert!(table.scroll_width > 200.0);
            assert!(
                flatten(&table.ops)
                    .iter()
                    .any(|op| matches!(op, DrawOp::Scrollable { .. }))
            );
            let code = laid(context, &format!("```\n{wide}{wide}\n```\n"), 200.0);
            assert!(code.scroll_width > 200.0);
        });
    }

    #[test]
    fn scrolled_link_rects_are_shifted_and_clipped() {
        let rects = [RectF::new(300.0, 0.0, 360.0, 20.0)];
        let clip = Some(RectF::new(0.0, 0.0, 200.0, 40.0));
        assert!(visible_link_rects(&rects, true, clip, 0.0).is_empty());
        assert_eq!(
            visible_link_rects(&rects, true, clip, 130.0),
            vec![RectF::new(170.0, 0.0, 200.0, 20.0)]
        );
        assert_eq!(
            visible_link_rects(&rects, true, clip, 200.0),
            vec![RectF::new(100.0, 0.0, 160.0, 20.0)]
        );
        assert_eq!(
            visible_link_rects(&rects, false, None, 500.0),
            rects.to_vec()
        );
    }

    #[test]
    fn wide_table_links_carry_the_table_clip() {
        with_context(&no_images, None, |context| {
            let wide = "wide ".repeat(30);
            let block = laid(
                context,
                &format!("| {wide} | [far](https://far.dev) |\n|---|---|\n| a | b |\n"),
                200.0,
            );
            let link = &block.targets[0];
            assert!(link.scrolls);
            assert_eq!(
                link.clip.map(|clip| (clip.left, clip.right)),
                Some((0.0, 200.0))
            );
            assert!(link.visible_rects(0.0).is_empty());
            let far = link.rects[0].left;
            assert!(!link.visible_rects(far).is_empty());
        });
    }

    #[test]
    fn target_search_sees_links_and_sections_only_from_the_model() {
        let has = |source: &str| {
            let (blocks, _) = parse_document(source);
            block_has_target(&blocks[0].kind)
        };
        assert!(has("see [a](b)\n"));
        assert!(has("# [a](b)\n"));
        assert!(has("- item\n  - [a](b)\n"));
        assert!(has("> quote\n>\n> - [a](b)\n"));
        assert!(has("| h |\n|---|\n| [a](b) |\n"));
        assert!(has("<div>\n\n[a](b)\n\n</div>\n"));
        assert!(has("<details>\n<summary>S</summary>\n\nx\n\n</details>\n"));
        assert!(!has("plain *text*\n"));
        assert!(!has("```\n[a](b)\n```\n"));
        assert!(!has("![alt](a.png)\n"));
    }

    #[test]
    fn task_items_draw_checkboxes() {
        with_context(&no_images, None, |context| {
            let block = laid(context, "- [x] done\n- [ ] todo\n", 400.0);
            let checks = flatten(&block.ops)
                .into_iter()
                .filter_map(|op| match op {
                    DrawOp::Checkbox { checked, .. } => Some(*checked),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(checks, vec![true, false]);
        });
    }

    #[test]
    fn images_scale_down_to_width_and_use_placeholders_when_unknown() {
        let dir = PathBuf::from(r"C:\docs");
        let known = |_: &Path| Some((800, 400));
        with_context(&known, Some(&dir), |context| {
            let block = laid(context, "![a](a.png)\n", 400.0);
            assert_eq!(
                block.images[0].path.as_deref(),
                Some(Path::new(r"C:\docs\a.png"))
            );
            assert_eq!(block.images[0].rect.width(), 400.0);
            assert_eq!(block.images[0].rect.height(), 200.0);
        });
        with_context(&no_images, Some(&dir), |context| {
            let block = laid(context, "![a](a.png)\n", 400.0);
            assert!(block.images[0].rect.height() > 0.0);
            assert!(block.images[0].rect.width() <= 400.0);
        });
    }

    #[test]
    fn inline_images_share_a_line_and_wrap_when_they_do_not_fit() {
        let dir = PathBuf::from(r"C:\docs");
        let known = |_: &Path| Some((100, 50));
        with_context(&known, Some(&dir), |context| {
            let source = "![a](a.png) ![b](b.png) ![c](c.png)\n";
            let wide = laid(context, source, 400.0);
            let tops = wide.images.iter().map(|slot| slot.rect.top).collect::<Vec<_>>();
            assert_eq!(tops.len(), 3);
            assert!(tops.iter().all(|top| *top == tops[0]), "{tops:?}");
            assert!(wide.images[1].rect.left > wide.images[0].rect.right);
            let narrow = laid(context, source, 250.0);
            assert!(narrow.images[2].rect.top > narrow.images[0].rect.top);
            assert_eq!(narrow.images[0].rect.width(), 100.0);
        });
    }

    #[test]
    fn images_without_a_size_are_one_line_chips_labelled_with_alt_text() {
        with_context(&no_images, None, |context| {
            let block = laid(context, "![build status](https://x.dev/badge.svg) text\n", 400.0);
            let slot = &block.images[0];
            assert!(slot.path.is_none());
            assert_eq!(slot.rect.height(), context.line_height());
            assert!(slot.rect.width() > 0.0 && slot.rect.width() < 400.0);
            assert!(slot.alt_origin.0 > 0.0);
        });
    }

    #[test]
    fn image_sizes_follow_attributes_then_natural_size_and_never_exceed_the_width() {
        let dir = PathBuf::from(r"C:\docs");
        let known = |_: &Path| Some((800, 400));
        with_context(&known, Some(&dir), |context| {
            let sized = |width, height| {
                let image = ImageRef {
                    dest: "a.png".into(),
                    width,
                    height,
                    ..ImageRef::default()
                };
                let placed = context.image_box(&image, 0, 400.0).unwrap();
                (placed.width, placed.height, placed.chip)
            };
            assert_eq!(
                sized(Some(Length::Pixels(96)), Some(Length::Pixels(48))),
                (96.0, 48.0, false)
            );
            assert_eq!(sized(Some(Length::Percent(50)), None), (200.0, 100.0, false));
            assert_eq!(sized(None, Some(Length::Pixels(100))), (200.0, 100.0, false));
            assert_eq!(sized(None, None), (400.0, 200.0, false));
            assert_eq!(
                sized(Some(Length::Pixels(1000)), Some(Length::Pixels(100))),
                (400.0, 40.0, false)
            );
        });
        with_context(&no_images, None, |context| {
            let image = ImageRef {
                dest: "https://x.dev/badge.svg".into(),
                alt: "build".into(),
                ..ImageRef::default()
            };
            let placed = context.image_box(&image, 0, 400.0).unwrap();
            assert!(placed.chip && placed.path.is_none());
            assert_eq!(placed.height, context.line_height());
        });
    }

    #[test]
    fn image_only_links_are_named_by_alt_text_then_title_then_destination() {
        with_context(&no_images, None, |context| {
            let name = |source: &str| laid(context, source, 400.0).targets[0].text.clone();
            assert_eq!(name("[![Build](b.png)](https://ci)\n"), "Build");
            assert_eq!(name("[![](b.png \"Status\")](https://ci)\n"), "Status");
            assert_eq!(name("[![](b.png)](https://ci)\n"), "https://ci");
            assert_eq!(name("[see ![Build](b.png)](https://ci)\n"), "see");
        });
    }

    #[test]
    fn pictures_pick_the_source_for_the_colour_scheme() {
        let dir = PathBuf::from(r"C:\docs");
        let source = "<picture>\n<source media=\"(prefers-color-scheme: dark)\" srcset=\"dark.png\">\n<source media=\"(prefers-color-scheme: light)\" srcset=\"light.png\">\n<img src=\"plain.png\">\n</picture>\n";
        for (dark, expected) in [(true, "dark.png"), (false, "light.png")] {
            with_setup(&no_images, Some(&dir), dark, &HashMap::new(), |context| {
                let block = laid(context, source, 400.0);
                assert_eq!(
                    block.images[0].path.as_deref(),
                    Some(dir.join(expected).as_path())
                );
            });
        }
        with_setup(&no_images, Some(&dir), true, &HashMap::new(), |context| {
            let block = laid(
                context,
                "<picture><source media=\"print\" srcset=\"any.png\"><img src=\"plain.png\"></picture>\n",
                400.0,
            );
            assert_eq!(
                block.images[0].path.as_deref(),
                Some(dir.join("any.png").as_path())
            );
        });
    }

    #[test]
    fn alignment_centres_text_and_inherits_through_containers() {
        with_context(&no_images, None, |context| {
            assert!(first_glyph_x(&laid(context, "<p align=\"center\">Hi</p>\n", 400.0)) > 150.0);
            assert!(
                first_glyph_x(&laid(context, "<div align=\"center\">\n\nHi\n\n</div>\n", 400.0))
                    > 150.0
            );
            assert!(
                first_glyph_x(&laid(
                    context,
                    "<div align=\"center\">\n\n<p align=\"left\">Hi</p>\n\n</div>\n",
                    400.0
                )) < 10.0
            );
            assert!(first_glyph_x(&laid(context, "Hi\n", 400.0)) < 10.0);
        });
        assert_eq!(
            text_alignment(TextAlign::Justify),
            DWRITE_TEXT_ALIGNMENT_JUSTIFIED
        );
    }

    #[test]
    fn mark_and_keyboard_draw_fills_and_a_border() {
        with_context(&no_images, None, |context| {
            let block = laid(context, "a <mark>b</mark> <kbd>Ctrl</kbd>\n", 400.0);
            let ops = flatten(&block.ops);
            assert!(ops.iter().any(|op| matches!(
                op,
                DrawOp::RoundedFill {
                    role: ColorRole::Mark,
                    ..
                }
            )));
            assert!(ops.iter().any(|op| matches!(
                op,
                DrawOp::RoundedStroke {
                    role: ColorRole::KbdBorder,
                    ..
                }
            )));
        });
    }

    #[test]
    fn scripts_and_small_text_shrink_and_insertions_are_underlined() {
        with_context(&no_images, None, |context| {
            let block = laid(
                context,
                "x<sup>2</sup> y<sub>3</sub> <small>s</small> <ins>n</ins>\n",
                400.0,
            );
            let (layout, _) = first_text_layout(&block);
            let size_at = |position| {
                let mut size = 0.0;
                unsafe { layout.GetFontSize(position, &mut size, None) }.unwrap();
                size
            };
            let body = context.fonts.body_size;
            assert_eq!(size_at(0), body);
            assert_eq!(size_at(1), body * SCRIPT_SCALE);
            assert_eq!(size_at(4), body * SCRIPT_SCALE);
            assert_eq!(size_at(6), body * SMALL_SCALE);
            let mut underline = BOOL::default();
            unsafe { layout.GetUnderline(8, &mut underline, None) }.unwrap();
            assert!(underline.as_bool());
        });
    }

    #[test]
    fn quotes_draw_a_bar_and_indent_their_content() {
        with_context(&no_images, None, |context| {
            let block = laid(context, "> quoted\n", 400.0);
            assert!(block.ops.iter().any(|op| matches!(
                op,
                DrawOp::Fill {
                    role: ColorRole::QuoteBar,
                    ..
                }
            )));
            let text_x = block.ops.iter().find_map(|op| match op {
                DrawOp::Text { x, .. } => Some(*x),
                _ => None,
            });
            assert!(text_x.unwrap() > 0.0);
        });
    }

    #[test]
    fn details_start_collapsed_and_open_through_their_state() {
        let source = "<details>\n<summary>More</summary>\n\n# Inside\n\n[x](https://x.dev)\n\n</details>\n";
        let key = DetailsKey {
            summary: "More".into(),
            occurrence: 0,
        };
        let kinds = |block: &LaidBlock| {
            block
                .targets
                .iter()
                .map(|target| target.kind.clone())
                .collect::<Vec<_>>()
        };
        let collapsed = with_context(&no_images, None, |context| {
            let block = laid(context, source, 400.0);
            assert_eq!(
                kinds(&block),
                vec![TargetKind::Disclosure {
                    key: key.clone(),
                    expanded: false
                }]
            );
            assert_eq!(block.targets[0].text, "More");
            assert!(block.headings.is_empty());
            block.height
        });
        let opened = HashMap::from([(key.clone(), true)]);
        let open = with_setup(&no_images, None, false, &opened, |context| {
            let block = laid(context, source, 400.0);
            assert_eq!(
                kinds(&block),
                vec![
                    TargetKind::Disclosure {
                        key: key.clone(),
                        expanded: true
                    },
                    TargetKind::Link("https://x.dev".into()),
                ]
            );
            assert_eq!(block.headings.len(), 1);
            block.height
        });
        assert!(open > collapsed);
    }

    #[test]
    fn headings_after_a_collapsed_section_keep_their_document_index() {
        with_context(&no_images, None, |context| {
            let block = laid(
                context,
                "<div>\n\n<details>\n<summary>S</summary>\n\n# A\n\n</details>\n\n# B\n\n</div>\n",
                400.0,
            );
            assert_eq!(
                block.headings.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
                vec![1]
            );
        });
    }
}
```

Notes on this file: `push_code` lost its `role` and `boxed` parameters because the grey HTML block that used them is gone. Table cell images keep working inside horizontally scrolling tables because their `DrawOp::Image` ops live in the `Scrollable` group.

- [ ] **Step 4: Update `render.rs`**

In `draw_ops`, add an arm after `DrawOp::RoundedFill`:

```rust
                DrawOp::RoundedStroke { rect, radius, role } => target.DrawRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect: rect.offset(x, y).inflate(-0.5).to_d2d(),
                        radiusX: *radius,
                        radiusY: *radius,
                    },
                    brushes.get(*role),
                    1.0,
                    None,
                ),
```

and replace the placeholder branch (`None => { … }` inside `DrawOp::Image`) with:

```rust
                        None => {
                            target.DrawRectangle(
                                &rect.inflate(-0.5).to_d2d(),
                                brushes.get(ColorRole::Border),
                                1.0,
                                None,
                            );
                            target.PushAxisAlignedClip(&rect.to_d2d(), D2D1_ANTIALIAS_MODE_ALIASED);
                            target.DrawTextLayout(
                                Vector2 {
                                    X: rect.left + slot.alt_origin.0,
                                    Y: rect.top + slot.alt_origin.1,
                                },
                                &slot.alt,
                                brushes.get(ColorRole::Muted),
                                D2D1_DRAW_TEXT_OPTIONS_NONE,
                            );
                            target.PopAxisAlignedClip();
                        }
```

- [ ] **Step 5: Wire the view to targets, sections, and the outline**

Edit `src/preview/view.rs`:

1. Imports: replace the `layout`, `links`, and `model` imports with

```rust
use crate::preview::layout::{
    LaidBlock, LayoutContext, PreviewFonts, Target, TargetKind, block_has_target, layout_block,
};
use crate::preview::outline::{self, DetailsKey, Outline, details_open_attribute, has_details};
```

and add `VK_SPACE` to the `KeyboardAndMouse` import list.

2. `PreviewStats` gains a field (set on every painted frame):

```rust
    /// The document's laid-out height in DIPs as of the last paint.
    pub content_height: f32,
```

3. Replace `VisibleLink` and its hand-written trait impls with:

```rust
/// A disclosure's identity and state, as a snapshot for accessibility clients.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Disclosure {
    pub key: DetailsKey,
    pub expanded: bool,
}

/// A link or disclosure on screen. `Debug`, `PartialEq`, and `Eq` are written by hand: windows-sys
/// `RECT` derives none of them.
#[derive(Clone)]
pub struct VisibleLink {
    pub text: String,
    /// Empty for a disclosure.
    pub dest: String,
    /// Client pixels of the target's first line.
    pub rect: RECT,
    pub disclosure: Option<Disclosure>,
    /// Whether keyboard focus is on this target.
    pub focused: bool,
}

impl std::fmt::Debug for VisibleLink {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let RECT {
            left,
            top,
            right,
            bottom,
        } = self.rect;
        formatter
            .debug_struct("VisibleLink")
            .field("text", &self.text)
            .field("dest", &self.dest)
            .field("rect", &(left, top, right, bottom))
            .field("disclosure", &self.disclosure)
            .field("focused", &self.focused)
            .finish()
    }
}

impl PartialEq for VisibleLink {
    fn eq(&self, other: &Self) -> bool {
        let rect = |rect: &RECT| (rect.left, rect.top, rect.right, rect.bottom);
        self.text == other.text
            && self.dest == other.dest
            && rect(&self.rect) == rect(&other.rect)
            && self.disclosure == other.disclosure
            && self.focused == other.focused
    }
}

impl Eq for VisibleLink {}
```

4. `ViewState`: replace the `anchors` field and its comment with

```rust
    /// Section keys and heading anchors; `None` until something needs them after the document
    /// changed.
    outline: Option<Outline>,
    /// `<details>` sections the user toggled away from their `open` attribute.
    details_overrides: HashMap<DetailsKey, bool>,
```

and in `PreviewView::create` replace `anchors: None,` with `outline: None, details_overrides: HashMap::new(),`.

5. `replace_document`: after `state.scroll_y = 0.0;` add `state.details_overrides.clear();`. `release`: replace `state.anchors = None;` with `state.outline = None; state.details_overrides = HashMap::new();`. `accept_update`: replace `state.anchors = None;` with `state.outline = None;` (keep the comment, now saying "walking every block on every keystroke's update is O(document)").

6. Replace `scroll_to_anchor` with:

```rust
    pub fn scroll_to_anchor(&self, anchor: &str) -> bool {
        self.with(|state| {
            let Some(found) = outline(state)
                .anchors
                .iter()
                .find(|entry| entry.slug == anchor)
                .cloned()
            else {
                return false;
            };
            // Chrome and Edge open every section around the target before scrolling to it.
            let keys = outline(state).details[found.block].clone();
            let mut opened = false;
            for &index in &found.enclosing {
                let default =
                    details_open_attribute(&state.document.blocks[found.block].kind, index)
                        .unwrap_or(false);
                if !state
                    .details_overrides
                    .get(&keys[index])
                    .copied()
                    .unwrap_or(default)
                {
                    set_details_open(state, &keys[index], default, true);
                    opened = true;
                }
            }
            if opened {
                state.layouts[found.block] = None;
            }
            let reading = state.heights.anchor(state.scroll_y);
            if matches!(ensure_layouts(state, &[found.block]), Ok(true)) {
                state.scroll_y = state.heights.scroll_for_anchor(reading);
            }
            let within = state.layouts[found.block]
                .as_ref()
                .and_then(|laid| {
                    laid.headings
                        .iter()
                        .find(|(heading, _)| *heading == found.heading)
                })
                .map_or(0.0, |(_, y)| *y);
            let top = state.heights.top(found.block) + within;
            set_scroll(state, top, true);
            true
        })
        .unwrap_or(false)
    }
```

7. Replace the `anchors` function with:

```rust
fn outline(state: &mut ViewState) -> &Outline {
    state
        .outline
        .get_or_insert_with(|| outline::build(&state.document.blocks))
}
```

8. Replace the start of `ensure_layouts` (through the `LayoutContext::new` call and the `layout_block` call) so it reads:

```rust
fn ensure_layouts(state: &mut ViewState, indices: &[usize]) -> Result<bool> {
    // Section keys need a walk of the whole document, so only blocks holding a section pay for it.
    let sectioned = indices
        .iter()
        .copied()
        .filter(|&index| {
            state.layouts.get(index).is_some_and(Option::is_none)
                && has_details(&state.document.blocks[index].kind)
        })
        .collect::<Vec<_>>();
    let keys = sectioned
        .into_iter()
        .map(|index| (index, outline(state).details[index].clone()))
        .collect::<HashMap<_, _>>();
    let dark = state.colors.is_dark();
    let ViewState {
        hwnd,
        graphics,
        brushes,
        fonts,
        document,
        document_dir,
        images,
        layouts,
        heights,
        layout_width,
        details_overrides,
        ..
    } = state;
    let Some(brushes) = brushes.as_ref() else {
        return Ok(false);
    };
    let mut requests = Vec::new();
    let mut changed = false;
    {
        let sizes = |path: &Path| images.size(path);
        let context = LayoutContext::new(
            graphics,
            brushes,
            fonts,
            document_dir.as_deref(),
            &sizes,
            dark,
            details_overrides,
        )?;
        for &index in indices {
            if index >= document.blocks.len() || layouts[index].is_some() {
                continue;
            }
            let block_keys = keys.get(&index).map_or(&[][..], Vec::as_slice);
            let laid = layout_block(
                &context,
                &document.blocks[index].kind,
                *layout_width,
                block_keys,
            )?;
```

(the rest of the function is unchanged).

9. `paint`: in the focus-ring loop replace `laid.links.get(focus_link)` with `laid.targets.get(focus_link)`; in the `Ok(())` branch, before `let links = visible_links(state);`, add `state.stats.content_height = state.heights.total();`.

10. Replace `link_at`, `link_dest`, and `set_underline` with:

```rust
fn target_at(state: &mut ViewState, x: i32, y: i32) -> Option<(usize, usize)> {
    let scale = dpi_scale(state.hwnd);
    let (view_width, _) = view_size(state.hwnd);
    let (content_left, _) = content_frame(view_width, state.centered);
    let document_y = y as f32 / scale + state.scroll_y - bar_height(state);
    let index = state.heights.index_at(document_y);
    let top = state.heights.top(index);
    let laid = state.layouts.get(index)?.as_ref()?;
    let block_x = x as f32 / scale - content_left;
    let block_y = document_y - top;
    let offset = state.h_scroll.get(&index).copied().unwrap_or(0.0);
    let hit = |target: &Target| {
        target
            .visible_rects(offset)
            .iter()
            .any(|rect| rect.contains(block_x, block_y))
    };
    // A link inside a summary wins over the row's disclosure.
    laid.targets
        .iter()
        .position(|target| matches!(target.kind, TargetKind::Link(_)) && hit(target))
        .or_else(|| laid.targets.iter().position(hit))
        .map(|target| (index, target))
}

fn target_dest(state: &ViewState, (block, target): (usize, usize)) -> Option<String> {
    state
        .layouts
        .get(block)?
        .as_ref()?
        .targets
        .get(target)?
        .dest()
        .map(str::to_owned)
}

fn set_underline(state: &ViewState, target: Option<(usize, usize)>, underline: bool) {
    if let Some((block, index)) = target
        && let Some(Some(laid)) = state.layouts.get(block)
        && let Some(target) = laid.targets.get(index)
        && matches!(target.kind, TargetKind::Link(_))
    {
        let _ = unsafe { target.layout.SetUnderline(underline, target.range) };
    }
}

/// Follows a link or toggles a section.
fn activate(state: &mut ViewState, (block, index): (usize, usize)) {
    let Some(kind) = state
        .layouts
        .get(block)
        .and_then(Option::as_ref)
        .and_then(|laid| laid.targets.get(index))
        .map(|target| target.kind.clone())
    else {
        return;
    };
    match kind {
        TargetKind::Link(dest) => post_link(state.hwnd, dest),
        TargetKind::Disclosure { key, .. } => toggle_details(state, &key),
    }
}

fn set_details_open(state: &mut ViewState, key: &DetailsKey, default: bool, open: bool) {
    if open == default {
        state.details_overrides.remove(key);
    } else {
        state.details_overrides.insert(key.clone(), open);
    }
}

/// Expands or collapses a section. Only its top-level block is laid out again, and the block at the
/// top of the view stays where it is.
fn toggle_details(state: &mut ViewState, key: &DetailsKey) {
    let found = outline(state)
        .details
        .iter()
        .enumerate()
        .find_map(|(block, keys)| {
            keys.iter()
                .position(|candidate| candidate == key)
                .map(|index| (block, index))
        });
    // A stale key (its summary was edited since) toggles nothing.
    let Some((block, index)) = found else {
        return;
    };
    let default =
        details_open_attribute(&state.document.blocks[block].kind, index).unwrap_or(false);
    let open = state.details_overrides.get(key).copied().unwrap_or(default);
    set_details_open(state, key, default, !open);
    set_underline(state, state.hover, false);
    state.hover = None;
    state.pressed = None;
    let reading = state.heights.anchor(state.scroll_y);
    state.layouts[block] = None;
    if matches!(ensure_layouts(state, &[block]), Ok(true)) {
        state.scroll_y = state.heights.scroll_for_anchor(reading);
    }
    invalidate(state.hwnd);
}
```

11. Replace `visible_links` with:

```rust
fn visible_links(state: &mut ViewState) -> Vec<VisibleLink> {
    let scale = dpi_scale(state.hwnd);
    let (view_width, view_height) = view_size(state.hwnd);
    let (content_left, _) = content_frame(view_width, state.centered);
    let bar = bar_height(state);
    let mut links = Vec::new();
    for index in visible_indices(state, view_height - bar) {
        let top = state.heights.top(index) - state.scroll_y + bar;
        let offset = state.h_scroll.get(&index).copied().unwrap_or(0.0);
        let Some(Some(laid)) = state.layouts.get(index) else {
            continue;
        };
        for (target_index, target) in laid.targets.iter().enumerate() {
            // Targets scrolled out of a wide table's clip are not on screen.
            let Some(rect) = target.visible_rects(offset).first().copied() else {
                continue;
            };
            let rect = rect.offset(content_left, top);
            links.push(VisibleLink {
                text: target.text.clone(),
                dest: target.dest().unwrap_or_default().to_owned(),
                rect: RECT {
                    left: (rect.left * scale) as i32,
                    top: (rect.top * scale) as i32,
                    right: (rect.right * scale) as i32,
                    bottom: (rect.bottom * scale) as i32,
                },
                disclosure: match &target.kind {
                    TargetKind::Link(_) => None,
                    TargetKind::Disclosure { key, expanded } => Some(Disclosure {
                        key: key.clone(),
                        expanded: *expanded,
                    }),
                },
                focused: state.focus == Some((index, target_index)),
            });
        }
    }
    links
}
```

12. `move_focus`: `block_has_link` becomes `block_has_target`, `laid.links.len()` becomes `laid.targets.len()`, and the doc comment says "the next (or previous) link or disclosure". `focus_link`: `laid.links.get(link)` becomes `laid.targets.get(link)`.

13. `WM_KEYDOWN`: replace the `VK_RETURN` arm with

```rust
                    VK_RETURN => {
                        if let Some(focus) = state.focus {
                            activate(state, focus);
                        }
                    }
                    VK_SPACE => {
                        if let Some((block, index)) = state.focus
                            && state
                                .layouts
                                .get(block)
                                .and_then(Option::as_ref)
                                .and_then(|laid| laid.targets.get(index))
                                .is_some_and(|target| {
                                    matches!(target.kind, TargetKind::Disclosure { .. })
                                })
                        {
                            activate(state, (block, index));
                        }
                    }
```

14. `WM_MOUSEMOVE`: `link_at` becomes `target_at`; `post_hover(hwnd, hovered.and_then(|link| link_dest(state, link)))` becomes `post_hover(hwnd, hovered.and_then(|target| target_dest(state, target)))`. `WM_LBUTTONDOWN`: `link_at` becomes `target_at`. `WM_LBUTTONUP`: replace the release handling with

```rust
                let released = target_at(state, x, y);
                if released.is_some()
                    && released == state.pressed.take()
                    && let Some(released) = released
                {
                    activate(state, released);
                }
```

15. In `src/preview/accessible.rs` tests, add `disclosure: None, focused: false,` to both `VisibleLink` literals.

16. Add view tests (in the `tests` module; add `VK_RETURN` and `VK_SPACE` to its `KeyboardAndMouse` import):

```rust
    fn click(view: &PreviewView, rect: RECT) {
        let point =
            (((rect.top + rect.bottom) / 2) << 16 | ((rect.left + rect.right) / 2)) as isize;
        unsafe {
            SendMessageW(view.hwnd(), WM_LBUTTONDOWN, 0, point);
            SendMessageW(view.hwnd(), WM_LBUTTONUP, 0, point);
        }
    }

    fn visible_target(view: &PreviewView, disclosure: bool) -> VisibleLink {
        view.visible_links()
            .into_iter()
            .find(|link| link.disclosure.is_some() == disclosure)
            .expect("a visible target")
    }

    fn paragraphs(count: usize) -> String {
        (0..count)
            .map(|index| format!("para {index}\n\n"))
            .collect()
    }

    const SECTION: &str = "<details>\n<summary>More</summary>\n\nHidden body\n\n</details>\n";

    #[test]
    fn clicking_a_disclosure_expands_and_collapses_it() {
        let parent = TestWindow::new(800, 600);
        let view = view_with(&parent, SECTION);
        let collapsed = view.stats().content_height;
        click(&view, visible_target(&view, true).rect);
        repaint(&view);
        assert!(view.stats().content_height > collapsed);
        assert_eq!(
            visible_target(&view, true).disclosure.map(|d| d.expanded),
            Some(true)
        );
        click(&view, visible_target(&view, true).rect);
        repaint(&view);
        assert_eq!(view.stats().content_height, collapsed);
        assert!(take_posted(&parent, WM_FASTPAD_PREVIEW_LINK).is_none());
        view.destroy();
    }

    #[test]
    fn tab_enter_and_space_toggle_a_focused_disclosure() {
        let parent = TestWindow::new(800, 600);
        let view = view_with(&parent, SECTION);
        let collapsed = view.stats().content_height;
        unsafe { SendMessageW(view.hwnd(), WM_KEYDOWN, VK_TAB as usize, 0) };
        repaint(&view);
        assert!(visible_target(&view, true).focused);
        unsafe { SendMessageW(view.hwnd(), WM_KEYDOWN, VK_RETURN as usize, 0) };
        repaint(&view);
        assert!(view.stats().content_height > collapsed);
        unsafe { SendMessageW(view.hwnd(), WM_KEYDOWN, VK_SPACE as usize, 0) };
        repaint(&view);
        assert_eq!(view.stats().content_height, collapsed);
        view.destroy();
    }

    #[test]
    fn a_link_in_a_summary_is_followed_without_toggling() {
        let parent = TestWindow::new(800, 600);
        let view = view_with(
            &parent,
            "<details>\n<summary>See <a href=\"https://x.dev\">site</a></summary>\n\nBody\n\n</details>\n",
        );
        let collapsed = view.stats().content_height;
        click(&view, visible_target(&view, false).rect);
        let message = take_posted(&parent, WM_FASTPAD_PREVIEW_LINK).expect("link message");
        let dest = unsafe { Box::from_raw(message.lParam as *mut String) };
        assert_eq!(*dest, "https://x.dev");
        repaint(&view);
        assert_eq!(view.stats().content_height, collapsed);
        view.destroy();
    }

    #[test]
    fn anchors_inside_collapsed_sections_open_them_first() {
        let parent = TestWindow::new(800, 600);
        // Paragraphs after the section keep the scroll from clamping at the end of the document.
        let source = paragraphs(100)
            + "<details>\n<summary>More</summary>\n\n## Deep Heading\n\nBody\n\n</details>\n\n"
            + &paragraphs(40);
        let view = view_with(&parent, &source);
        assert!(view.scroll_to_anchor("deep-heading"));
        repaint(&view);
        assert_eq!(
            visible_target(&view, true).disclosure.map(|d| d.expanded),
            Some(true)
        );
        assert!(view.top_line() >= 200);
        view.destroy();
    }

    #[test]
    fn a_collapsed_section_maps_its_source_lines_to_its_disclosure_row() {
        let parent = TestWindow::new(800, 600);
        let mut source = paragraphs(60);
        let details_line = source.lines().count();
        source.push_str("<details>\n<summary>More</summary>\n\n");
        source.push_str(
            &(0..40)
                .map(|index| format!("hidden {index}\n\n"))
                .collect::<String>(),
        );
        source.push_str("</details>\n\n");
        source.push_str(&paragraphs(60));
        let view = view_with(&parent, &source);
        view.scroll_to_line(details_line + 30);
        repaint(&view);
        view.scroll_to_line(details_line + 30);
        let (top, height, scroll) = with_state(view.hwnd(), |state| {
            let index = state
                .document
                .blocks
                .iter()
                .position(|block| {
                    matches!(block.kind, crate::preview::model::BlockKind::Details { .. })
                })
                .unwrap();
            (
                state.heights.top(index),
                state.heights.height(index),
                state.scroll_y,
            )
        })
        .unwrap();
        assert!(
            scroll >= top && scroll <= top + height,
            "{scroll} is outside the section at {top}..{}",
            top + height
        );
        with_state(view.hwnd(), |state| set_scroll(state, top, true));
        assert_eq!(view.top_line(), details_line);
        view.destroy();
    }
```

- [ ] **Step 6: Compile and run the part's tests**

Run: `cargo clippy --all-targets --all-features -- -D warnings`
Expected: no warnings.

Run, one after another: `cargo test --lib preview::outline`, `cargo test --lib preview::colors`, `cargo test --lib preview::layout -- --test-threads=1`, `cargo test --lib preview::view -- --test-threads=1`, `cargo test --lib preview::accessible -- --test-threads=1`.
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src/catppuccin.rs src/preview
git commit -m "feat(preview): lay out inline images, alignment, HTML styles, and collapsible sections"
```

---
## Part 7: Disclosures in MSAA

**Files:**
- Modify: `src/preview/accessible.rs`, `src/preview/view.rs`

**Interfaces:**
- Consumes: `VisibleLink { disclosure, focused }`, `toggle_details` (Part 6).
- Produces: `view::ACTIVATE_DISCLOSURE: usize = 1` — the `wparam` of a `WM_FASTPAD_PREVIEW_ACTIVATE` whose `lparam` is a `Box<DetailsKey>`; disclosures report `ROLE_SYSTEM_OUTLINEBUTTON`, `STATE_SYSTEM_EXPANDED` or `STATE_SYSTEM_COLLAPSED`, and default actions "Expand" / "Collapse"; toggling raises `EVENT_OBJECT_STATECHANGE` for the disclosure's child id after the next paint.

- [ ] **Step 1: Raise state changes and accept disclosure activation in the view**

In `src/preview/view.rs`:

1. Below `CLASS_NAME`, add:

```rust
/// `WM_FASTPAD_PREVIEW_ACTIVATE` `wparam` for a disclosure: `lparam` is a `Box<DetailsKey>`. Links use
/// `wparam` 0 with a `Box<String>`.
pub const ACTIVATE_DISCLOSURE: usize = 1;
```

2. `ViewState` gains

```rust
    /// A section toggled since the last paint; its accessible child raises a state change once the
    /// snapshot shows the new state.
    state_change: Option<DetailsKey>,
```

initialised `state_change: None,` in `create`, reset in `release` (`state.state_change = None;`), and set at the end of `toggle_details` (`state.state_change = Some(key.clone());` before `invalidate`).

3. `PaintOutcome` gains

```rust
    /// Accessible child id of a disclosure whose state changed, raised once the borrow has ended.
    state_change: Option<i32>,
```

Every `PaintOutcome { … }` literal gets `state_change: None`, except the successful frame, which becomes:

```rust
            let links = visible_links(state);
            let state_change = state.state_change.take().and_then(|key| {
                links
                    .iter()
                    .position(|link| link.disclosure.as_ref().is_some_and(|d| d.key == key))
                    .map(|index| index as i32 + 1)
            });
            *state
                .accessible
                .write()
                .unwrap_or_else(|error| error.into_inner()) = links;
            Ok(PaintOutcome {
                scroll: Some(scroll_info(state, view_height)),
                repaint,
                state_change,
            })
```

4. In `WM_PAINT`, after the `outcome.repaint` check inside `if let Some(outcome) = outcome { … }`, add:

```rust
                if let Some(child) = outcome.state_change {
                    unsafe {
                        windows_sys::Win32::UI::Accessibility::NotifyWinEvent(
                            windows_sys::Win32::UI::WindowsAndMessaging::EVENT_OBJECT_STATECHANGE,
                            hwnd,
                            windows_sys::Win32::UI::WindowsAndMessaging::OBJID_CLIENT,
                            child,
                        )
                    };
                }
```

5. Replace the `WM_FASTPAD_PREVIEW_ACTIVATE` arm with:

```rust
        WM_FASTPAD_PREVIEW_ACTIVATE => {
            // The accessibility provider resolved the target against the snapshot the client saw;
            // resolving an index here could land on another target after a repaint.
            if lparam != 0 {
                if wparam == ACTIVATE_DISCLOSURE {
                    let key = *unsafe { Box::from_raw(lparam as *mut DetailsKey) };
                    with_state(hwnd, |state| toggle_details(state, &key));
                } else {
                    let dest = *unsafe { Box::from_raw(lparam as *mut String) };
                    post_link(hwnd, dest);
                }
            }
            0
        }
```

6. View test:

```rust
    #[test]
    fn accessible_activation_toggles_a_disclosure_and_reports_the_change_once() {
        let parent = TestWindow::new(800, 600);
        let view = view_with(&parent, SECTION);
        let key = visible_target(&view, true).disclosure.unwrap().key;
        let payload = Box::into_raw(Box::new(key));
        unsafe {
            SendMessageW(
                view.hwnd(),
                WM_FASTPAD_PREVIEW_ACTIVATE,
                ACTIVATE_DISCLOSURE,
                payload as isize,
            )
        };
        assert!(with_state(view.hwnd(), |state| state.state_change.is_some()).unwrap());
        repaint(&view);
        assert!(with_state(view.hwnd(), |state| state.state_change.is_none()).unwrap());
        assert_eq!(
            view.accessible_links().read().unwrap()[0]
                .disclosure
                .as_ref()
                .map(|d| d.expanded),
            Some(true)
        );
        view.destroy();
    }
```

- [ ] **Step 2: Expose disclosures through the provider**

In `src/preview/accessible.rs`:

1. Imports: add `ROLE_SYSTEM_OUTLINEBUTTON` to the `Accessibility` import; add `use crate::preview::view::ACTIVATE_DISCLOSURE;`; add `STATE_SYSTEM_COLLAPSED, STATE_SYSTEM_EXPANDED` to the `WindowsAndMessaging` import. Update the module doc comment to "whose children are the links and section disclosures currently on screen".
2. `value`: `Some(Some(link)) => unsafe { allocate_bstr(&link.dest, output) },` stays; `dest` is already empty for disclosures.
3. `role`:

```rust
        Some(Some(link)) if link.disclosure.is_some() => ROLE_SYSTEM_OUTLINEBUTTON,
        Some(Some(_)) => ROLE_SYSTEM_LINK,
```

4. `state`:

```rust
        Some(Some(link)) => {
            let focused = if link.focused && has_focus(item) {
                STATE_SYSTEM_FOCUSED
            } else {
                0
            };
            let kind = match &link.disclosure {
                Some(disclosure) if disclosure.expanded => STATE_SYSTEM_EXPANDED,
                Some(_) => STATE_SYSTEM_COLLAPSED,
                None => STATE_SYSTEM_LINKED,
            };
            kind | STATE_SYSTEM_FOCUSABLE | focused
        }
```

5. `default_action`:

```rust
        Some(Some(link)) => {
            let action = match &link.disclosure {
                Some(disclosure) if disclosure.expanded => "Collapse",
                Some(_) => "Expand",
                None => "Jump",
            };
            unsafe { allocate_bstr(action, output) }
        }
```

6. `do_default_action`:

```rust
unsafe extern "system" fn do_default_action(this: *mut c_void, child: RawVariant) -> HRESULT {
    let item = unsafe { item(this) };
    let Some(Some(link)) = target(item, &child) else {
        return E_INVALIDARG;
    };
    // Post the destination or section key itself: the window's snapshot may change before the
    // message is handled, and an index would then name another target.
    let posted = match link.disclosure {
        Some(disclosure) => {
            let payload = Box::into_raw(Box::new(disclosure.key));
            let posted = unsafe {
                PostMessageW(
                    item.hwnd,
                    WM_FASTPAD_PREVIEW_ACTIVATE,
                    ACTIVATE_DISCLOSURE,
                    payload as isize,
                )
            } != 0;
            if !posted {
                drop(unsafe { Box::from_raw(payload) });
            }
            posted
        }
        None => {
            let payload = Box::into_raw(Box::new(link.dest));
            let posted = unsafe {
                PostMessageW(item.hwnd, WM_FASTPAD_PREVIEW_ACTIVATE, 0, payload as isize)
            } != 0;
            if !posted {
                drop(unsafe { Box::from_raw(payload) });
            }
            posted
        }
    };
    if posted { S_OK } else { E_FAIL }
}
```

7. Tests: extend `links()` with a third child

```rust
            VisibleLink {
                text: "More".into(),
                dest: String::new(),
                rect: RECT {
                    top: 70,
                    bottom: 90,
                    ..rect
                },
                disclosure: Some(crate::preview::view::Disclosure {
                    key: crate::preview::outline::DetailsKey {
                        summary: "More".into(),
                        occurrence: 0,
                    },
                    expanded: false,
                }),
                focused: false,
            },
```

change the child-count assertion in `the_preview_is_a_document_whose_children_are_its_links` to `3`, its out-of-range probes from child `3` to child `4` (and in `default_action_posts_the_link_destination`, `RawVariant::integer(3)` to `integer(4)`), and add:

```rust
    #[test]
    fn disclosures_are_outline_buttons_that_post_their_section_key() {
        use crate::preview::render::TestWindow;
        use windows_sys::Win32::UI::WindowsAndMessaging::{MSG, PM_REMOVE, PeekMessageW};
        let window = TestWindow::new(100, 100);
        let provider = create_provider(window.0, links());
        let table = &PREVIEW_VTABLE;
        unsafe {
            let mut role = RawVariant::empty();
            assert_eq!((table.get_acc_role)(provider, RawVariant::integer(3), &mut role), S_OK);
            assert_eq!(role.child_id(), Some(ROLE_SYSTEM_OUTLINEBUTTON as i32));

            let mut state = RawVariant::empty();
            assert_eq!((table.get_acc_state)(provider, RawVariant::integer(3), &mut state), S_OK);
            let flags = state.child_id().unwrap() as u32;
            assert_ne!(flags & STATE_SYSTEM_COLLAPSED, 0);
            assert_eq!(flags & STATE_SYSTEM_LINKED, 0);

            let mut action: BSTR = std::ptr::null();
            assert_eq!(
                (table.get_acc_default_action)(provider, RawVariant::integer(3), &mut action),
                S_OK
            );
            assert_eq!(read_bstr(action), "Expand");

            assert_eq!((table.acc_do_default_action)(provider, RawVariant::integer(3)), S_OK);
            let mut msg = MSG::default();
            assert_ne!(
                PeekMessageW(
                    &mut msg,
                    window.0,
                    WM_FASTPAD_PREVIEW_ACTIVATE,
                    WM_FASTPAD_PREVIEW_ACTIVATE,
                    PM_REMOVE,
                ),
                0
            );
            assert_eq!(msg.wParam, ACTIVATE_DISCLOSURE);
            let key = Box::from_raw(msg.lParam as *mut crate::preview::outline::DetailsKey);
            assert_eq!(key.summary, "More");
            (table.release)(provider);
        }
    }
```

(`RawVariant::child_id` reads a `VT_I4` value, which is how roles and states are returned.)

- [ ] **Step 3: Compile and run the part's tests**

Run: `cargo clippy --all-targets --all-features -- -D warnings`, then `cargo test --lib preview::accessible -- --test-threads=1` and `cargo test --lib preview::view -- --test-threads=1`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/preview/accessible.rs src/preview/view.rs
git commit -m "feat(preview): expose collapsible sections to screen readers as outline buttons"
```

---
## Part 8: README fixture, benchmarks, documentation, and final verification

**Files:**
- Create: `tests/fixtures/html-readme.md`
- Modify: `src/preview/model.rs` (one test), `src/preview/view.rs` (`image_states` test hook), `tests/windows/markdown_preview.rs`, `README.md`, `benchmarks/README.md`, `docs/superpowers/specs/2026-09-17-preview-html-rendering-design.md`

**Interfaces:**
- Consumes: everything above.
- Produces: `#[cfg(test)] PreviewView::image_states(&self) -> Vec<(PathBuf, bool)>` (each laid-out image slot with a local path, and whether its pixels are decoded).

- [ ] **Step 1: Add the fixture**

Create `tests/fixtures/html-readme.md`:

```markdown
<div align="center">

<img src="assets/fastpad-icon.svg" alt="FastPad" width="96" height="96">

# FastPad

**The text editor that's ready before you are.**

[![Latest release](https://img.shields.io/github/v/release/coccor/FastPad?label=release)](https://github.com/coccor/FastPad/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

</div>

<!-- A comment GitHub hides. -->

Press <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>V</kbd> to open the preview.

<details>
<summary><b>Other ways to install</b>: Scoop, installer, portable ZIP</summary>

| Method | How |
|---|---|
| **Scoop** | `scoop bucket add fastpad`<br>`scoop install fastpad/fastpad` |
| **Portable** | Extract anywhere<br>run <b>FastPad.exe</b> |

</details>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/images/dark.png">
  <source media="(prefers-color-scheme: light)" srcset="docs/images/light.png">
  <img src="docs/images/light.png" alt="Screenshot">
</picture>

<p align="right">Back to <a href="#fastpad">top</a></p>
```

- [ ] **Step 2: Model test — no literal markup survives**

In `src/preview/model.rs` tests, add:

```rust
    #[test]
    fn the_readme_fixture_renders_no_literal_markup() {
        fn collect(kind: &BlockKind, texts: &mut Vec<String>) {
            match kind {
                BlockKind::Heading { text, .. } | BlockKind::Paragraph { text, .. } => {
                    texts.push(text.text.clone())
                }
                BlockKind::List { items, .. } => {
                    for block in items.iter().flat_map(|item| &item.blocks) {
                        collect(block, texts);
                    }
                }
                BlockKind::Quote(children) | BlockKind::Container { children, .. } => {
                    for child in children {
                        collect(child, texts);
                    }
                }
                BlockKind::Details {
                    summary, children, ..
                } => {
                    texts.push(summary.text.clone());
                    for child in children {
                        collect(child, texts);
                    }
                }
                BlockKind::Table { head, rows, .. } => {
                    texts.extend(head.iter().chain(rows.iter().flatten()).map(|cell| cell.text.clone()))
                }
                BlockKind::Code { text, .. } => texts.push(text.clone()),
                BlockKind::Rule => {}
            }
        }
        let source = include_str!("../../tests/fixtures/html-readme.md");
        let mut texts = Vec::new();
        for block in parse_document(source).0 {
            collect(&block.kind, &mut texts);
        }
        for text in &texts {
            for tag in [
                "div", "img", "details", "summary", "kbd", "b", "br", "picture", "source", "p",
                "a",
            ] {
                assert!(
                    !text.contains(&format!("<{tag}")) && !text.contains(&format!("</{tag}")),
                    "literal <{tag}> in {text:?}"
                );
            }
            assert!(!text.contains("<!--"), "literal comment in {text:?}");
        }
        assert!(texts.iter().any(|text| text.contains("Other ways to install")));
        assert!(texts.iter().any(|text| text == "Back to top"));
    }
```

Run: `cargo test --lib preview::model`
Expected: PASS.

- [ ] **Step 3: Integration test on the fixture**

In `src/preview/view.rs`, add to `impl PreviewView`:

```rust
    /// Each laid-out image with a local path, and whether its pixels are decoded.
    #[cfg(test)]
    pub fn image_states(&self) -> Vec<(PathBuf, bool)> {
        self.with(|state| {
            state
                .layouts
                .iter()
                .flatten()
                .flat_map(|laid| &laid.images)
                .filter_map(|slot| {
                    let path = slot.path.clone()?;
                    let ready = state.images.size(&path).is_some();
                    Some((path, ready))
                })
                .collect()
        })
        .unwrap_or_default()
    }
```

In `tests/windows/markdown_preview.rs`, add `VK_RETURN, VK_TAB` to the `KeyboardAndMouse` import and:

```rust
#[test]
fn the_html_readme_fixture_renders_its_svg_and_toggles_its_section() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    let icon = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("fastpad-icon.svg");
    let fixture = include_str!("../fixtures/html-readme.md")
        .replace("assets/fastpad-icon.svg", icon.to_str().unwrap());
    main.make_markdown(&fixture);
    main.command(CommandId::MarkdownPreviewSide);
    let view = main.view().unwrap();
    pump_until("the SVG icon decodes", Duration::from_secs(10), || {
        view.image_states()
            .iter()
            .any(|(path, ready)| path.ends_with("fastpad-icon.svg") && *ready)
    });
    let disclosure = || {
        view.accessible_links()
            .read()
            .unwrap()
            .iter()
            .find_map(|link| {
                link.disclosure
                    .as_ref()
                    .map(|disclosure| (disclosure.expanded, link.focused))
            })
    };
    for _ in 0..20 {
        if disclosure().is_some_and(|(_, focused)| focused) {
            break;
        }
        unsafe { SendMessageW(view.hwnd(), WM_KEYDOWN, VK_TAB as usize, 0) };
        pump_pending();
    }
    assert_eq!(disclosure(), Some((false, true)), "Tab reaches the section");
    let collapsed = view.stats().content_height;
    unsafe { SendMessageW(view.hwnd(), WM_KEYDOWN, VK_RETURN as usize, 0) };
    pump_until("the section expands", Duration::from_secs(3), || {
        disclosure() == Some((true, true)) && view.stats().content_height > collapsed
    });
    unsafe { SendMessageW(view.hwnd(), WM_KEYDOWN, VK_RETURN as usize, 0) };
    pump_until("the section collapses", Duration::from_secs(3), || {
        disclosure() == Some((false, true))
    });
}
```

Run: `cargo test --test markdown_preview the_html_readme_fixture -- --test-threads=1`
Expected: PASS. (Copy `native\out\x64\*.dll` into the worktree first if the harness cannot load Scintilla.)

- [ ] **Step 4: Benchmarks as ignored tests**

In `tests/windows/markdown_preview.rs`:

1. Add next to `sample_markdown`:

```rust
fn html_markdown(bytes: usize) -> String {
    let section = "<div align=\"center\">\n\n<img src=\"https://x.dev/badge.svg\" alt=\"badge\" width=\"96\">\n\n## Heading\n\n</div>\n\nPress <kbd>Ctrl</kbd>+<kbd>S</kbd> in <b>bold</b> text with a [link](https://x.dev).\n\n<details>\n<summary>More</summary>\n\n| a | b |\n|---|---|\n| 1<br>2 | <code>x</code> |\n\n</details>\n\n";
    section.repeat(bytes / section.len() + 1)[..bytes]
        .rsplit_once("\n\n")
        .map_or_else(String::new, |(text, _)| format!("{text}\n"))
}
```

2. Share the two measurement loops. Add:

```rust
fn preview_open_p95(main: &TestMain, text: &str) -> u64 {
    main.make_markdown(text);
    let mut samples = Vec::new();
    for _ in 0..30 {
        main.command(CommandId::MarkdownPreviewSide);
        let view = main.view().unwrap();
        pump_until("first frame", Duration::from_secs(5), || {
            view.stats().first_frame_micros > 0
        });
        samples.push(view.stats().first_frame_micros);
        main.command(CommandId::MarkdownPreviewClose);
    }
    p95(samples)
}

fn one_paragraph_update_p95(main: &TestMain, text: &str) -> u64 {
    main.make_markdown(text);
    main.command(CommandId::MarkdownPreviewSide);
    let view = main.view().unwrap();
    pump_until("initial render", Duration::from_secs(10), || {
        view.stats().block_count > 0
    });
    unsafe {
        SendMessageW(
            main.editor,
            crate::editor::scintilla_constants::SCI_GOTOPOS,
            text.len() / 2,
            0,
        )
    };
    pump_for(Duration::from_millis(300));
    // Edit a line the synced preview actually shows: an off-screen edit correctly skips the
    // repaint, which would leave `last_update_micros` holding the initial full render.
    unsafe {
        let line_start = SendMessageW(
            main.editor,
            crate::editor::scintilla_constants::SCI_POSITIONFROMLINE,
            view.top_line() + 1,
            0,
        );
        SendMessageW(
            main.editor,
            crate::editor::scintilla_constants::SCI_GOTOPOS,
            line_start as usize,
            0,
        );
    }
    let mut samples = Vec::new();
    for _ in 0..50 {
        let revision = view.stats().revision;
        let painted = view.stats().painted_updates;
        type_text(main.editor, "x");
        pump_until("update", Duration::from_secs(5), || {
            view.stats().revision > revision && view.stats().painted_updates > painted
        });
        pump_for(Duration::from_millis(20));
        samples.push(view.stats().last_update_micros);
    }
    p95(samples)
}
```

and reduce the two existing tests to:

```rust
fn opening_a_100_kb_preview_renders_within_50_ms_p95() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    let p95 = preview_open_p95(&main, &sample_markdown(100_000));
    println!("preview open p95: {p95} us");
    assert!(p95 < 50_000);
}
```

```rust
fn one_paragraph_updates_in_a_1_mb_document_within_2_ms_p95() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    let p95 = one_paragraph_update_p95(&main, &sample_markdown(1_000_000));
    println!("incremental update p95: {p95} us");
    assert!(p95 < 2_000);
}
```

(keeping their `#[test]` and `#[ignore = …]` attributes).

3. Add:

```rust
#[test]
#[ignore = "performance measurement: cargo test --release --test markdown_preview -- --ignored --test-threads=1"]
fn opening_a_100_kb_html_heavy_preview_renders_within_50_ms_p95() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    let p95 = preview_open_p95(&main, &html_markdown(100_000));
    println!("HTML-heavy preview open p95: {p95} us");
    assert!(p95 < 50_000);
}

#[test]
#[ignore = "performance measurement: cargo test --release --test markdown_preview -- --ignored --test-threads=1"]
fn one_paragraph_update_inside_a_div_spanning_1_mb_is_recorded() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    let text = format!("<div>\n\n{}\n</div>\n", sample_markdown(1_000_000));
    let p95 = one_paragraph_update_p95(&main, &text);
    // No target: an element spanning the document reparses whole on every edit (spec §4.5).
    println!("update inside a 1 MB div p95: {p95} us");
}
```

Run: `cargo clippy --all-targets --all-features -- -D warnings`
Expected: no warnings.

- [ ] **Step 5: Documentation**

1. `README.md`, section "Markdown preview": replace the sentences from "The preview renders GitHub-flavored Markdown natively" through "Nothing is loaded from the network." with:

```markdown
The preview renders GitHub-flavored Markdown natively (tables, task lists, strikethrough, code
blocks, images) and the HTML GitHub allows in READMEs: centred `<div>` and `<p>` blocks, sized
`<img>` tags, `<picture>` images that follow the light or dark theme, collapsible
`<details>` sections, and inline tags such as `<kbd>`, `<sup>`, and `<br>`. Local PNG, JPEG, GIF,
and SVG images are shown; remote images show a placeholder with their alt text, because nothing is
loaded from the network. It updates shortly after you stop typing, and scrolling either pane scrolls
the other. Links open when clicked: web and mail links in your default browser, `#anchors` inside
the preview (opening any collapsed section around them), and local files in a FastPad tab.
```

(Draft PR #4 rewrites the README; carry this content into its "Markdown with a live preview" section when both merge.)

2. `docs/superpowers/specs/2026-09-17-preview-html-rendering-design.md`: change `Status:` to `Approved design; phase 1 implemented`, and add a section before "## 12. Risks":

```markdown
## 11a. Implementation clarifications (phase 1)

Recorded from `docs/superpowers/plans/2026-09-17-preview-html-rendering-phase-1.md`:

- SVG bytes reach Direct2D through an `IWICStream` (WIC is already loaded for images), not
  `SHCreateMemStream`; `shlwapi.dll` is never loaded and the import guards are unchanged.
- `Length` holds whole numbers so blocks stay `Eq`; percentage heights are ignored.
- `ImageSource` stores the first `srcset` candidate as `url`; `Heading` gains `anchor`.
- `<picture>` is inline builder state, not a block frame.
- HTML text collapses whitespace and trims trailing spaces.
- Blocks finished inside one CommonMark HTML block are wrapped in one `Container`; HTML blocks that
  produce nothing produce no preview block.
- The sanitizer removes `href`, `src`, and `srcset` values with schemes other than `http`, `https`,
  and `mailto`.
- `outline.rs` owns `DetailsKey`, section keys, and anchors; disclosure activation from MSAA reuses
  `WM_FASTPAD_PREVIEW_ACTIVATE` with `wparam = 1`.
- Scroll sync needed no new mapping: a collapsed section is a short block.
```

- [ ] **Step 6: Final verification (the one full run)**

1. Back up `%LocalAppData%\FastPad\fastpad.ini` (copy it to `$env:CLAUDE_JOB_DIR\tmp\fastpad.ini.bak` or another scratch path).
2. Run: `cargo fmt --check` — Expected: no diff.
3. Run: `cargo clippy --locked --all-targets --all-features -- -D warnings` — Expected: no warnings.
4. Run: `pwsh -File tools/audit-dependencies.ps1` — Expected: PASS with no change to the allowed closure or `windows` features.
5. Run: `cargo test --all-targets -- --test-threads=1` — Expected: PASS, including `binary_does_not_statically_import_preview_graphics_libraries` and `launching_with_a_markdown_file_loads_no_preview_graphics_library`.
6. Run: `cargo test --release --test markdown_preview -- --ignored --test-threads=1 --nocapture` and `cargo test --release --lib preview::svg -- --ignored --nocapture`. Expected: the existing targets still pass; `opening_a_100_kb_html_heavy_preview_renders_within_50_ms_p95` passes; record the printed numbers.
7. Binary size: `cargo build --release --features release-package` and note `target\release\fastpad.exe` size; build `origin/main` the same way in a temporary worktree (`git worktree add ..\fastpad-size-baseline origin/main`, build, note the size, then `git worktree remove ..\fastpad-size-baseline`). Expected growth under 100 KB; if larger, report it rather than trimming features.
8. Startup: on the same machine, run `./tools/benchmark.ps1 -Runs 100 -Warmup 10 -LaunchFile benchmarks/fixtures/sample.md -Output <scratch>\candidate.jsonl` with this branch's release build and the same with the `origin/main` baseline build from item 7 (`-Output <scratch>\baseline.jsonl`), then `cargo run --release --bin fastpad-bench -- compare <scratch>\baseline.jsonl <scratch>\candidate.jsonl`. Expected: no milestone reported as a regression. (Check `tools/benchmark.ps1 -?` for how it selects the binary; point it at each build in turn. Back up and restore `fastpad.ini` around it.)
9. Live check: run the release build on this repository's `README.md` and on `tests/fixtures/html-readme.md` (with `--new-window`), open the preview, and confirm the centred header with the rendered SVG icon, chips for remote badges, the collapsed section expanding on click, and no literal HTML. Restore `fastpad.ini` from the backup afterwards.
10. Record in `benchmarks/README.md` under "## Markdown preview" a paragraph "HTML rendering (phase 1)" with the measured values from items 6–8: HTML-heavy preview open p95, update inside a 1 MB `<div>` p95, SVG icon decode at 256 px (median and max), the release binary size before and after, and the startup comparison result.
11. Run the superpowers:requesting-code-review skill once over the whole branch diff (`git diff origin/main...HEAD`), fix what it confirms, and re-run only the affected targeted tests.

- [ ] **Step 7: Commit and push**

```bash
git add tests/fixtures/html-readme.md tests/windows/markdown_preview.rs src/preview README.md benchmarks/README.md docs/superpowers/specs/2026-09-17-preview-html-rendering-design.md
git commit -m "test(preview): cover the README HTML fixture end to end and record measurements"
git -c credential.helper= -c "credential.helper=!gh auth git-credential" push origin worktree-preview-html
```

Then open a draft PR titled "Preview: render GitHub's HTML subset and SVG images (phase 1)" whose body lists the spec, the plan, the measurements, and the clarifications, ending with the attribution lines from the session.

---

## Spec coverage

- Spec §4.1–4.2 → Part 1; §4.3–4.4 → Part 4; §4.5 → Part 5; §5.1–5.6 → Part 6 (layout, render, colours); §6.1–6.4 → Part 6 (outline, view); §6.5 → Part 6 (image-only link names in `link_name`) and Part 7; §7 → Part 2; §8 → Parts 2, 4, 6 (no failure paths, placeholders); §9.1–9.2 → tests in every part plus Part 8; §9.3 → Part 8 Step 4 and Step 6; §10 → Part 8 Step 5 (the earlier spec note already exists).
- §9.2 import guard for `shlwapi.dll` is unnecessary after clarification 1.
