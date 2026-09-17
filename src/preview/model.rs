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
            Frame::Heading { html: true, .. }
                | Frame::Paragraph { html: true, .. }
                | Frame::Summary(_)
        )
    }

    /// Markdown inline content (a paragraph, heading, or table cell), where HTML block tags do
    /// nothing.
    fn is_markdown_inline(&self) -> bool {
        matches!(
            self,
            Frame::Heading { html: false, .. }
                | Frame::Paragraph { html: false, .. }
                | Frame::Cell(_)
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
            "p" => {
                self.close_html_frame(|frame| matches!(frame, Frame::Paragraph { html: true, .. }))
            }
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
            vec![para(with_images(
                "\u{FFFC}",
                vec![(0, image("img/logo.png", "logo"))]
            ))]
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
            blocks
                .iter()
                .map(|block| block.kind.clone())
                .collect::<Vec<_>>(),
            vec![
                container(
                    TextAlign::Center,
                    vec![heading(1, plain("FastPad")), para(plain("Fast."))]
                ),
                para(plain("After")),
            ]
        );
        assert_eq!(blocks[0].lines, 0..7);
        assert!(
            source[blocks[0].bytes.clone()]
                .trim_end()
                .ends_with("</div>")
        );
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
                BlockKind::Table { head, rows, .. } => texts.extend(
                    head.iter()
                        .chain(rows.iter().flatten())
                        .map(|cell| cell.text.clone()),
                ),
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
                "div", "img", "details", "summary", "kbd", "b", "br", "picture", "source", "p", "a",
            ] {
                assert!(
                    !text.contains(&format!("<{tag}")) && !text.contains(&format!("</{tag}")),
                    "literal <{tag}> in {text:?}"
                );
            }
            assert!(!text.contains("<!--"), "literal comment in {text:?}");
        }
        assert!(
            texts
                .iter()
                .any(|text| text.contains("Other ways to install"))
        );
        assert!(texts.iter().any(|text| text == "Back to top"));
    }
}
