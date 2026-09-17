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
/// The tallest an image box may be, in DIPs.
const MAX_IMAGE_HEIGHT: f32 = 16_384.0;

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
    Disclosure {
        key: DetailsKey,
        expanded: bool,
    },
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
        // An absurd `height` must not produce a block taller than scrolling can address.
        box_height = box_height.min(MAX_IMAGE_HEIGHT);
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
    unsafe {
        layout.HitTestTextPosition(image.position, false, &mut point_x, &mut point_y, &mut hit)
    }
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
            let rich =
                context.rich_layout(cell, context.fonts.body_size, weight, 100_000.0, width)?;
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
                rich.layout
                    .SetMaxWidth(inner.max(1.0))
                    .map_err(hresult_error)?;
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
            let tops = wide
                .images
                .iter()
                .map(|slot| slot.rect.top)
                .collect::<Vec<_>>();
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
            let block = laid(
                context,
                "![build status](https://x.dev/badge.svg) text\n",
                400.0,
            );
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
            assert_eq!(
                sized(Some(Length::Percent(50)), None),
                (200.0, 100.0, false)
            );
            assert_eq!(
                sized(None, Some(Length::Pixels(100))),
                (200.0, 100.0, false)
            );
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
                first_glyph_x(&laid(
                    context,
                    "<div align=\"center\">\n\nHi\n\n</div>\n",
                    400.0
                )) > 150.0
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
        let source =
            "<details>\n<summary>More</summary>\n\n# Inside\n\n[x](https://x.dev)\n\n</details>\n";
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
                block
                    .headings
                    .iter()
                    .map(|(index, _)| *index)
                    .collect::<Vec<_>>(),
                vec![1]
            );
        });
    }
}
