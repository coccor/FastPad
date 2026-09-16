//! Markdown source to top-level preview blocks. Pure Rust with no Win32, so every rule here is
//! unit-tested. Inline styles are recorded as UTF-16 ranges because DirectWrite addresses text in
//! UTF-16 code units.

use pulldown_cmark::{
    Alignment, BrokenLink, CodeBlockKind, CowStr, Event, Options, Parser, Tag, TagEnd,
};
use std::ops::Range;

/// GitHub-flavored extensions the preview renders. Footnotes, math, and metadata stay literal.
pub const PARSE_OPTIONS: Options = Options::ENABLE_TABLES
    .union(Options::ENABLE_STRIKETHROUGH)
    .union(Options::ENABLE_TASKLISTS);

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
    Heading { level: u8, text: RichText },
    Paragraph(RichText),
    Images(Vec<ImageRef>),
    List { start: Option<u64>, items: Vec<ListItem> },
    Quote(Vec<BlockKind>),
    Code { language: String, text: String },
    Table { alignments: Vec<CellAlign>, head: Vec<RichText>, rows: Vec<Vec<RichText>> },
    Rule,
    Html(String),
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RichText {
    pub text: String,
    pub utf16_len: u32,
    pub spans: Vec<Span>,
}

impl RichText {
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
    ImageAlt,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageRef {
    pub dest: String,
    pub alt: String,
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
        refdefs.iter().find(|definition| definition.key == key).map(|definition| {
            (
                CowStr::from(definition.dest.clone()),
                CowStr::from(definition.title.clone()),
            )
        })
    };
    let mut events =
        Parser::new_with_broken_link_callback(source, PARSE_OPTIONS, Some(resolve))
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
        if builder.stack.is_empty() && top.is_none() {
            top = Some(range.clone());
        }
        builder.event(event);
        if let Some(kind) = builder.finished.take() {
            let span = top.take().unwrap_or(range.clone());
            let bytes = span.start..span.end.max(range.end);
            let first_line = lines.line_of(bytes.start);
            let last_line = lines.line_of(bytes.end.saturating_sub(1).max(bytes.start));
            blocks.push(Block {
                kind,
                bytes: base_byte + bytes.start..base_byte + bytes.end,
                lines: base_line + first_line..base_line + last_line + 1,
            });
        }
    }
    blocks
}

/// Counts newlines forward from the last query; top-level block offsets only grow.
struct LineCounter<'a> {
    bytes: &'a [u8],
    position: usize,
    line: usize,
}

impl<'a> LineCounter<'a> {
    fn new(source: &'a str) -> Self {
        Self { bytes: source.as_bytes(), position: 0, line: 0 }
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
    open: Vec<(InlineStyle, u32)>,
    images: Vec<ImageRef>,
    image_stack: Vec<ImageRef>,
    has_text: bool,
}

impl TextBuilder {
    fn push(&mut self, value: &str, counts_as_text: bool) {
        if let Some(image) = self.image_stack.last_mut() {
            image.alt.push_str(value);
            return;
        }
        self.text.push_str(value);
        self.utf16_len += value.encode_utf16().count() as u32;
        self.has_text |= counts_as_text && !value.trim().is_empty();
    }

    fn open(&mut self, style: InlineStyle) {
        self.open.push((style, self.utf16_len));
    }

    fn close(&mut self) {
        if let Some((style, start)) = self.open.pop()
            && start < self.utf16_len
        {
            self.spans.push(Span { range: start..self.utf16_len, style });
        }
    }

    fn finish_image(&mut self) {
        let Some(image) = self.image_stack.pop() else {
            return;
        };
        let start = self.utf16_len;
        let alt = if image.alt.is_empty() { "image".to_owned() } else { image.alt.clone() };
        self.push(&alt, false);
        self.spans.push(Span { range: start..self.utf16_len, style: InlineStyle::ImageAlt });
        self.images.push(image);
    }

    fn into_rich_text(self) -> RichText {
        RichText { text: self.text, utf16_len: self.utf16_len, spans: self.spans }
    }

    /// A paragraph of only images (and whitespace) becomes an image block.
    fn into_paragraph(self) -> BlockKind {
        if !self.has_text && !self.images.is_empty() {
            BlockKind::Images(self.images)
        } else {
            BlockKind::Paragraph(self.into_rich_text())
        }
    }
}

enum Frame {
    Heading { level: u8, text: TextBuilder },
    Paragraph(TextBuilder),
    List { start: Option<u64>, items: Vec<ListItem> },
    Item { task: Option<bool>, blocks: Vec<BlockKind>, loose: Option<TextBuilder> },
    Quote(Vec<BlockKind>),
    Code { language: String, text: String },
    Html(String),
    Table { alignments: Vec<CellAlign>, head: Vec<RichText>, rows: Vec<Vec<RichText>> },
    Row(Vec<RichText>),
    Cell(TextBuilder),
}

#[derive(Default)]
struct Builder {
    stack: Vec<Frame>,
    finished: Option<BlockKind>,
}

impl Builder {
    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(&text, true),
            Event::Code(code) => {
                if let Some(target) = self.inline_target() {
                    target.open(InlineStyle::Code);
                    target.push(&code, true);
                    target.close();
                }
            }
            Event::Html(html) => {
                if let Some(Frame::Html(buffer)) = self.stack.last_mut() {
                    buffer.push_str(&html);
                } else {
                    self.text(&html, true);
                }
            }
            Event::InlineHtml(html)
            | Event::InlineMath(html)
            | Event::DisplayMath(html)
            | Event::FootnoteReference(html) => self.text(&html, true),
            Event::SoftBreak => self.text(" ", false),
            Event::HardBreak => self.text("\n", false),
            Event::Rule => self.add_block(BlockKind::Rule),
            Event::TaskListMarker(checked) => {
                if let Some(Frame::Item { task, .. }) = self.stack.last_mut() {
                    *task = Some(checked);
                }
            }
        }
    }

    fn text(&mut self, value: &str, counts_as_text: bool) {
        match self.stack.last_mut() {
            Some(Frame::Code { text, .. }) => text.push_str(value),
            Some(Frame::Html(buffer)) => buffer.push_str(value),
            _ => {
                if let Some(target) = self.inline_target() {
                    target.push(value, counts_as_text);
                }
            }
        }
    }

    fn inline_target(&mut self) -> Option<&mut TextBuilder> {
        match self.stack.last_mut()? {
            Frame::Heading { text, .. } | Frame::Paragraph(text) | Frame::Cell(text) => Some(text),
            Frame::Item { loose, .. } => Some(loose.get_or_insert_with(TextBuilder::default)),
            _ => None,
        }
    }

    /// Tight list items hold text directly; a nested block ends that implicit paragraph.
    fn flush_loose_item_text(&mut self) {
        if let Some(Frame::Item { blocks, loose, .. }) = self.stack.last_mut()
            && let Some(text) = loose.take()
        {
            blocks.push(text.into_paragraph());
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Emphasis => self.open_style(InlineStyle::Emphasis),
            Tag::Strong => self.open_style(InlineStyle::Strong),
            Tag::Strikethrough => self.open_style(InlineStyle::Strikethrough),
            Tag::Link { dest_url, .. } => self.open_style(InlineStyle::Link(dest_url.to_string())),
            Tag::Image { dest_url, .. } => {
                if let Some(target) = self.inline_target() {
                    target.image_stack.push(ImageRef { dest: dest_url.to_string(), alt: String::new() });
                }
            }
            Tag::Paragraph => self.push_block_frame(Frame::Paragraph(TextBuilder::default())),
            Tag::Heading { level, .. } => self.push_block_frame(Frame::Heading {
                level: level as u8,
                text: TextBuilder::default(),
            }),
            Tag::BlockQuote(_) => self.push_block_frame(Frame::Quote(Vec::new())),
            Tag::CodeBlock(kind) => {
                let language = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().unwrap_or_default().to_owned()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                self.push_block_frame(Frame::Code { language, text: String::new() });
            }
            Tag::HtmlBlock => self.push_block_frame(Frame::Html(String::new())),
            Tag::List(start) => self.push_block_frame(Frame::List { start, items: Vec::new() }),
            Tag::Item => self.stack.push(Frame::Item { task: None, blocks: Vec::new(), loose: None }),
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

    fn open_style(&mut self, style: InlineStyle) {
        if let Some(target) = self.inline_target() {
            target.open(style);
        }
    }

    fn push_block_frame(&mut self, frame: Frame) {
        self.flush_loose_item_text();
        self.stack.push(frame);
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link => {
                if let Some(target) = self.inline_target() {
                    target.close();
                }
            }
            TagEnd::Image => {
                if let Some(target) = self.inline_target() {
                    target.finish_image();
                }
            }
            TagEnd::Paragraph
            | TagEnd::Heading(_)
            | TagEnd::BlockQuote(_)
            | TagEnd::CodeBlock
            | TagEnd::HtmlBlock
            | TagEnd::List(_)
            | TagEnd::Table => {
                if let Some(frame) = self.stack.pop() {
                    let kind = finish_block(frame);
                    self.add_block(kind);
                }
            }
            TagEnd::Item => {
                self.flush_loose_item_text();
                if let Some(Frame::Item { task, blocks, .. }) = self.stack.pop()
                    && let Some(Frame::List { items, .. }) = self.stack.last_mut()
                {
                    items.push(ListItem { task, blocks });
                }
            }
            TagEnd::TableHead => {
                if let Some(Frame::Row(cells)) = self.stack.pop()
                    && let Some(Frame::Table { head, .. }) = self.stack.last_mut()
                {
                    *head = cells;
                }
            }
            TagEnd::TableRow => {
                if let Some(Frame::Row(cells)) = self.stack.pop()
                    && let Some(Frame::Table { rows, .. }) = self.stack.last_mut()
                {
                    rows.push(cells);
                }
            }
            TagEnd::TableCell => {
                if let Some(Frame::Cell(text)) = self.stack.pop()
                    && let Some(Frame::Row(cells)) = self.stack.last_mut()
                {
                    cells.push(text.into_rich_text());
                }
            }
            _ => {}
        }
    }

    fn add_block(&mut self, kind: BlockKind) {
        self.flush_loose_item_text();
        match self.stack.last_mut() {
            None => self.finished = Some(kind),
            Some(Frame::Item { blocks, .. }) | Some(Frame::Quote(blocks)) => blocks.push(kind),
            // A block can only close into a container; anything else is a parser invariant break.
            Some(_) => {}
        }
    }
}

fn finish_block(frame: Frame) -> BlockKind {
    match frame {
        Frame::Heading { level, text } => BlockKind::Heading { level, text: text.into_rich_text() },
        Frame::Paragraph(text) => text.into_paragraph(),
        Frame::List { start, items } => BlockKind::List { start, items },
        Frame::Quote(blocks) => BlockKind::Quote(blocks),
        Frame::Code { language, text } => BlockKind::Code {
            language,
            text: text.strip_suffix('\n').unwrap_or(&text).to_owned(),
        },
        Frame::Html(html) => BlockKind::Html(html.trim_end().to_owned()),
        Frame::Table { alignments, head, rows } => BlockKind::Table { alignments, head, rows },
        // Items, rows, and cells close through their own end tags and never reach this function;
        // these arms only keep the match exhaustive without a panic in an abort-on-panic build.
        Frame::Item { blocks, .. } => BlockKind::Quote(blocks),
        Frame::Row(_) | Frame::Cell(_) => BlockKind::Rule,
    }
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
        parse_document(source).0.into_iter().map(|block| block.kind).collect()
    }

    fn plain(text: &str) -> RichText {
        RichText {
            text: text.to_owned(),
            utf16_len: text.encode_utf16().count() as u32,
            spans: Vec::new(),
        }
    }

    fn styled(text: &str, spans: Vec<Span>) -> RichText {
        RichText { spans, ..plain(text) }
    }

    fn span(range: Range<u32>, style: InlineStyle) -> Span {
        Span { range, style }
    }

    #[test]
    fn headings_and_paragraphs_carry_source_ranges() {
        let source = "# Title\n\nHello *world*\n";
        let (blocks, _) = parse_document(source);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].kind, BlockKind::Heading { level: 1, text: plain("Title") });
        assert_eq!(source[blocks[0].bytes.clone()].trim_end(), "# Title");
        assert_eq!(blocks[0].lines, 0..1);
        assert_eq!(
            blocks[1].kind,
            BlockKind::Paragraph(styled("Hello world", vec![span(6..11, InlineStyle::Emphasis)]))
        );
        assert_eq!(source[blocks[1].bytes.clone()].trim_end(), "Hello *world*");
        assert_eq!(blocks[1].lines, 2..3);
    }

    #[test]
    fn setext_headings_are_headings() {
        assert_eq!(
            kinds("Title\n=====\n"),
            vec![BlockKind::Heading { level: 1, text: plain("Title") }]
        );
    }

    #[test]
    fn task_lists_nest_and_record_checked_state() {
        let expected = BlockKind::List {
            start: None,
            items: vec![
                ListItem { task: Some(true), blocks: vec![BlockKind::Paragraph(plain("done"))] },
                ListItem {
                    task: Some(false),
                    blocks: vec![
                        BlockKind::Paragraph(plain("todo")),
                        BlockKind::List {
                            start: None,
                            items: vec![ListItem {
                                task: None,
                                blocks: vec![BlockKind::Paragraph(plain("nested"))],
                            }],
                        },
                    ],
                },
            ],
        };
        assert_eq!(kinds("- [x] done\n- [ ] todo\n  - nested\n"), vec![expected]);
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
                BlockKind::Paragraph(plain("quote")),
                BlockKind::Quote(vec![BlockKind::Paragraph(plain("inner"))]),
            ])]
        );
    }

    #[test]
    fn fenced_and_indented_code_blocks() {
        assert_eq!(
            kinds("```rust\nfn main() {}\n```\n"),
            vec![BlockKind::Code { language: "rust".into(), text: "fn main() {}".into() }]
        );
        assert_eq!(
            kinds("    x = 1\n"),
            vec![BlockKind::Code { language: String::new(), text: "x = 1".into() }]
        );
    }

    #[test]
    fn tables_keep_alignment_head_and_styled_cells() {
        assert_eq!(
            kinds("| a | b |\n|:--|--:|\n| 1 | **2** |\n"),
            vec![BlockKind::Table {
                alignments: vec![CellAlign::Left, CellAlign::Right],
                head: vec![plain("a"), plain("b")],
                rows: vec![vec![plain("1"), styled("2", vec![span(0..1, InlineStyle::Strong)])]],
            }]
        );
    }

    #[test]
    fn strikethrough_links_and_inline_code_become_spans() {
        assert_eq!(
            kinds("~~old~~ [site](https://x.dev) `code`\n"),
            vec![BlockKind::Paragraph(styled(
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
    fn image_only_paragraphs_become_image_blocks() {
        assert_eq!(
            kinds("![logo](img/logo.png)\n"),
            vec![BlockKind::Images(vec![ImageRef {
                dest: "img/logo.png".into(),
                alt: "logo".into()
            }])]
        );
    }

    #[test]
    fn images_inside_text_render_as_alt_text() {
        assert_eq!(
            kinds("See ![logo](a.png) here\n"),
            vec![BlockKind::Paragraph(styled(
                "See logo here",
                vec![span(4..8, InlineStyle::ImageAlt)]
            ))]
        );
    }

    #[test]
    fn rules_and_raw_html() {
        assert_eq!(
            kinds("---\n\n<div>hi</div>\n"),
            vec![BlockKind::Rule, BlockKind::Html("<div>hi</div>".into())]
        );
    }

    #[test]
    fn span_offsets_are_utf16() {
        assert_eq!(
            kinds("**é😀**\n"),
            vec![BlockKind::Paragraph(styled("é😀", vec![span(0..3, InlineStyle::Strong)]))]
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
        let link = BlockKind::Paragraph(styled(
            "site",
            vec![span(0..4, InlineStyle::Link("https://x.dev".into()))],
        ));
        assert_eq!(blocks[0].kind, link);
        assert_eq!(parse_blocks("[site]\n", 0, 0, &refdefs)[0].kind, link);
    }

    #[test]
    fn footnote_syntax_stays_literal() {
        assert_eq!(kinds("a[^1]\n"), vec![BlockKind::Paragraph(plain("a[^1]"))]);
    }
}
