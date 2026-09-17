//! Blocks to positioned drawing operations using DirectWrite text layouts. A `LaidBlock` is valid
//! for one content width, one `PreviewFonts`, and one `Brushes` (links and muted spans use brush
//! drawing effects), so the view discards layouts when any of those change.

use crate::Result;
use crate::preview::colors::ColorRole;
use crate::preview::dwrite::{Graphics, hresult_error};
use crate::preview::model::{BlockKind, CellAlign, InlineStyle, ListItem, RichText};
use crate::preview::render::{Brushes, RectF};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FONT_STYLE_ITALIC, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_HIT_TEST_METRICS,
    DWRITE_TEXT_ALIGNMENT, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
    DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_TEXT_METRICS, DWRITE_TEXT_RANGE,
    DWRITE_WORD_WRAPPING_NO_WRAP, IDWriteTextFormat, IDWriteTextLayout,
};
use windows::core::PCWSTR;

const HEADING_SCALE: [f32; 6] = [2.0, 1.5, 1.25, 1.0, 0.875, 0.875];
const BULLETS: [&str; 3] = ["\u{2022}", "\u{25E6}", "\u{25AA}"];
const CODE_SCALE: f32 = 0.85;

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

pub struct LinkHit {
    /// The visible link text, used as the accessible name.
    pub text: String,
    pub dest: String,
    pub rects: Vec<RectF>,
    pub layout: IDWriteTextLayout,
    pub range: DWRITE_TEXT_RANGE,
    /// Whether the rects move with the block's horizontal scroll offset.
    pub scrolls: bool,
    /// For scrolling links, the block-coordinate area they are drawn clipped to.
    pub clip: Option<RectF>,
}

impl LinkHit {
    /// The link's rectangles as currently shown, in block coordinates: shifted by the block's
    /// horizontal scroll offset and cut to its clip. Parts scrolled out of view are dropped.
    pub fn visible_rects(&self, h_offset: f32) -> Vec<RectF> {
        visible_link_rects(&self.rects, self.scrolls, self.clip, h_offset)
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

/// Whether a block contains a link anywhere, including nested lists, quotes, and table cells.
/// Answers from the model alone, so keyboard navigation can skip link-free blocks unlaid.
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

pub struct ImageSlot {
    pub path: Option<PathBuf>,
    pub rect: RectF,
    pub alt: IDWriteTextLayout,
}

pub struct LaidBlock {
    pub height: f32,
    pub ops: Vec<DrawOp>,
    pub links: Vec<LinkHit>,
    pub images: Vec<ImageSlot>,
    /// The widest horizontally scrollable content, or 0 when nothing scrolls.
    pub scroll_width: f32,
    pub heading: Option<String>,
}

pub struct LayoutContext<'a> {
    graphics: &'a Graphics,
    brushes: &'a Brushes,
    fonts: &'a PreviewFonts,
    #[allow(dead_code)] // Read again by inline image layout (Part 6).
    document_dir: Option<&'a Path>,
    #[allow(dead_code)]
    image_size: &'a dyn Fn(&Path) -> Option<(u32, u32)>,
    formats: RefCell<HashMap<(bool, u32, i32), IDWriteTextFormat>>,
    line_height: f32,
}

impl<'a> LayoutContext<'a> {
    pub fn new(
        graphics: &'a Graphics,
        brushes: &'a Brushes,
        fonts: &'a PreviewFonts,
        document_dir: Option<&'a Path>,
        image_size: &'a dyn Fn(&Path) -> Option<(u32, u32)>,
    ) -> Result<Self> {
        let mut context = Self {
            graphics,
            brushes,
            fonts,
            document_dir,
            image_size,
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

    fn rich_layout(
        &self,
        rich: &RichText,
        size: f32,
        weight: DWRITE_FONT_WEIGHT,
        width: f32,
    ) -> Result<IDWriteTextLayout> {
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
                    InlineStyle::Code => layout
                        .SetFontFamilyName(PCWSTR(code_family.as_ptr()), range)
                        .and_then(|()| layout.SetFontSize(size * CODE_SCALE, range)),
                    InlineStyle::Link(_) => {
                        layout.SetDrawingEffect(self.brushes.get(ColorRole::Link), range)
                    }
                    _ => Ok(()),
                }
            }
            .map_err(hresult_error)?;
        }
        Ok(layout)
    }
}

#[derive(Default)]
struct Output {
    ops: Vec<DrawOp>,
    links: Vec<LinkHit>,
    images: Vec<ImageSlot>,
    scroll_width: f32,
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
}

pub fn layout_block(
    context: &LayoutContext<'_>,
    kind: &BlockKind,
    width: f32,
) -> Result<LaidBlock> {
    let mut output = Output::default();
    let style = Style {
        role: ColorRole::Text,
        list_depth: 0,
        nested: false,
    };
    let extent = layout_kind(context, kind, 0.0, 0.0, width, style, &mut output)?;
    let heading = match kind {
        BlockKind::Heading { text, .. } => Some(text.plain_text().to_owned()),
        _ => None,
    };
    Ok(LaidBlock {
        height: extent.content + extent.margin,
        ops: output.ops,
        links: output.links,
        images: output.images,
        scroll_width: output.scroll_width,
        heading,
    })
}

fn layout_kind(
    context: &LayoutContext<'_>,
    kind: &BlockKind,
    x: f32,
    y: f32,
    width: f32,
    style: Style,
    output: &mut Output,
) -> Result<Extent> {
    let unit = context.fonts.unit();
    let margin = if style.nested {
        4.0 * unit
    } else {
        16.0 * unit
    };
    match kind {
        BlockKind::Paragraph { text, .. } => {
            let height = push_rich_text(
                context,
                text,
                context.fonts.body_size,
                DWRITE_FONT_WEIGHT_NORMAL,
                x,
                y,
                width,
                style.role,
                false,
                output,
            )?;
            Ok(Extent {
                content: height,
                margin,
            })
        }
        BlockKind::Heading { level, text, .. } => {
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
        BlockKind::Code { text, .. } => push_code(
            context,
            text,
            x,
            y,
            width,
            ColorRole::Text,
            true,
            output,
            margin,
        ),
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
    }
}

fn layout_children(
    context: &LayoutContext<'_>,
    blocks: &[BlockKind],
    x: f32,
    y: f32,
    width: f32,
    style: Style,
    output: &mut Output,
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
    role: ColorRole,
    scrolls: bool,
    output: &mut Output,
) -> Result<f32> {
    let layout = context.rich_layout(text, size, weight, width)?;
    push_laid_text(context, text, layout, x, y, role, scrolls, output)
}

#[allow(clippy::too_many_arguments)]
fn push_laid_text(
    context: &LayoutContext<'_>,
    text: &RichText,
    layout: IDWriteTextLayout,
    x: f32,
    y: f32,
    role: ColorRole,
    scrolls: bool,
    output: &mut Output,
) -> Result<f32> {
    let unit = context.fonts.unit();
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
            InlineStyle::Link(dest) => output.links.push(LinkHit {
                text: text
                    .text
                    .encode_utf16()
                    .collect::<Vec<_>>()
                    .get(span.range.start as usize..span.range.end as usize)
                    .map_or_else(|| dest.clone(), String::from_utf16_lossy),
                dest: dest.clone(),
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
    output.ops.push(DrawOp::Text { layout, x, y, role });
    Ok(height)
}

#[allow(clippy::too_many_arguments)]
fn push_code(
    context: &LayoutContext<'_>,
    text: &str,
    x: f32,
    y: f32,
    width: f32,
    role: ColorRole,
    boxed: bool,
    output: &mut Output,
    margin: f32,
) -> Result<Extent> {
    let unit = context.fonts.unit();
    let padding = if boxed { 16.0 * unit } else { 0.0 };
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
    if boxed {
        output.ops.push(DrawOp::RoundedFill {
            rect,
            radius: 6.0 * unit,
            role: ColorRole::CodeBackground,
        });
    }
    let content_width = text_metrics.widthIncludingTrailingWhitespace + 2.0 * padding;
    let text_op = DrawOp::Text {
        layout,
        x: x + padding,
        y: y + padding,
        role,
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
    output: &mut Output,
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
    output: &mut Output,
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
            let empty = RichText::default();
            let cell = row.get(column).unwrap_or(&empty);
            let layout = context.rich_layout(cell, context.fonts.body_size, weight, 100_000.0)?;
            let natural = metrics(&layout)?.widthIncludingTrailingWhitespace;
            *column_width = column_width.max(natural + 2.0 * pad_x);
            row_layouts.push(layout);
        }
        layouts.push(row_layouts);
    }
    let table_width: f32 = column_widths.iter().sum();
    let scrolls = table_width > width;
    let mut table_output = Output::default();
    let mut cursor = y;
    for (row_index, row_layouts) in layouts.into_iter().enumerate() {
        let mut row_height = context.line_height;
        for (column, layout) in row_layouts.iter().enumerate() {
            let inner = column_widths[column] - 2.0 * pad_x;
            unsafe {
                layout.SetMaxWidth(inner.max(1.0)).map_err(hresult_error)?;
                layout
                    .SetTextAlignment(alignment(
                        alignments.get(column).copied().unwrap_or(CellAlign::None),
                    ))
                    .map_err(hresult_error)?;
            }
            row_height = row_height.max(metrics(layout)?.height);
        }
        row_height += 2.0 * pad_y;
        if row_index > 0 && row_index % 2 == 0 {
            table_output.ops.push(DrawOp::Fill {
                rect: RectF::new(x, cursor, x + table_width, cursor + row_height),
                role: ColorRole::TableStripe,
            });
        }
        let mut cell_x = x;
        for (column, layout) in row_layouts.into_iter().enumerate() {
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
            let empty = RichText::default();
            let cell = all_rows[row_index].get(column).unwrap_or(&empty);
            push_laid_text(
                context,
                cell,
                layout,
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
        for link in &mut table_output.links {
            link.clip = Some(clip);
        }
    }
    output.links.append(&mut table_output.links);
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

fn alignment(align: CellAlign) -> DWRITE_TEXT_ALIGNMENT {
    match align {
        CellAlign::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
        CellAlign::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
        CellAlign::None | CellAlign::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
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
    use crate::preview::render::{TestWindow, create_hwnd_target};

    fn with_context(
        image_size: &dyn Fn(&Path) -> Option<(u32, u32)>,
        document_dir: Option<&Path>,
        test: impl FnOnce(&LayoutContext<'_>),
    ) {
        let graphics = Graphics::load().unwrap();
        let window = TestWindow::new(400, 300);
        let target = create_hwnd_target(&graphics, window.0, 400, 300, 96).unwrap();
        let brushes = Brushes::create(&target, &preview_colors(Theme::Light, false)).unwrap();
        let fonts = PreviewFonts::from_settings("Consolas", 12);
        let context =
            LayoutContext::new(&graphics, &brushes, &fonts, document_dir, image_size).unwrap();
        test(&context);
    }

    fn no_images(_: &Path) -> Option<(u32, u32)> {
        None
    }

    fn laid(context: &LayoutContext<'_>, source: &str, width: f32) -> LaidBlock {
        let (blocks, _) = parse_document(source);
        layout_block(context, &blocks[0].kind, width).unwrap()
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

    #[test]
    fn headings_are_taller_than_paragraphs_with_the_same_text() {
        with_context(&no_images, None, |context| {
            let heading = laid(context, "# Same\n", 400.0);
            let paragraph = laid(context, "Same\n", 400.0);
            assert!(heading.height > paragraph.height);
            assert_eq!(heading.heading.as_deref(), Some("Same"));
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
            assert_eq!(block.links.len(), 1);
            assert_eq!(block.links[0].dest, "https://x.dev");
            assert_eq!(block.links[0].text, "here");
            let rect = block.links[0].rects[0];
            assert!(rect.left > 0.0 && rect.width() > 0.0 && rect.height() > 0.0);
            assert!(!block.links[0].scrolls);
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
                &format!(
                    "| {wide} | [far](https://far.dev) |
|---|---|
| a | b |
"
                ),
                200.0,
            );
            let link = &block.links[0];
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
    fn link_search_sees_links_in_nested_blocks_only_from_the_model() {
        let has = |source: &str| {
            let (blocks, _) = parse_document(source);
            block_has_link(&blocks[0].kind)
        };
        assert!(has("see [a](b)
"));
        assert!(has("# [a](b)
"));
        assert!(has("- item
  - [a](b)
"));
        assert!(has("> quote
>
> - [a](b)
"));
        assert!(has("| h |
|---|
| [a](b) |
"));
        assert!(!has("plain *text*
"));
        assert!(!has("```
[a](b)
```
"));
        assert!(!has("![alt](a.png)
"));
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
}
