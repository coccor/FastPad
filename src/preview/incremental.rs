//! Keeps a `PreviewDocument` in sync with edits by reparsing only the blocks around them. A slice
//! is accepted only when the unchanged sentinel blocks at both ends parse back identically, which
//! proves the parser state at the slice boundaries did not change; otherwise the slice widens and
//! finally falls back to a full parse. The property test in this file is the authority: a
//! divergence means a missing fallback rule, never a rendering special case.

use crate::preview::model::{Block, RefDef, parse_blocks, parse_document};
use std::borrow::Cow;
use std::ops::Range;

pub const MAX_PENDING_EDITS: usize = 64;
const SENTINELS: usize = 2;
const MAX_WIDENINGS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edit {
    pub position: usize,
    pub removed: usize,
    pub inserted: usize,
    pub lines_delta: isize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Pending {
    Nothing,
    Full,
    Edits(Vec<Edit>),
}

#[derive(Debug, Default)]
pub struct EditLog {
    edits: Vec<Edit>,
    full: bool,
}

impl EditLog {
    pub fn record(&mut self, edit: Edit) {
        if self.full {
            return;
        }
        if self.edits.len() == MAX_PENDING_EDITS {
            self.request_full();
        } else {
            self.edits.push(edit);
        }
    }

    pub fn request_full(&mut self) {
        self.full = true;
        self.edits.clear();
    }

    pub fn is_empty(&self) -> bool {
        !self.full && self.edits.is_empty()
    }

    pub fn take(&mut self) -> Pending {
        if std::mem::take(&mut self.full) {
            self.edits.clear();
            Pending::Full
        } else if self.edits.is_empty() {
            Pending::Nothing
        } else {
            Pending::Edits(std::mem::take(&mut self.edits))
        }
    }

    /// Puts back work taken by `take` that was not processed; later edits stay after it.
    pub fn restore(&mut self, pending: Pending) {
        match pending {
            Pending::Nothing => {}
            Pending::Full => self.request_full(),
            Pending::Edits(mut edits) => {
                if self.full {
                    return;
                }
                edits.append(&mut self.edits);
                if edits.len() > MAX_PENDING_EDITS {
                    self.request_full();
                } else {
                    self.edits = edits;
                }
            }
        }
    }
}

/// Read access to the current document text; implemented over Scintilla's buffer by the host.
pub trait SourceText {
    fn len(&self) -> usize;
    fn slice(&self, range: Range<usize>) -> Cow<'_, str>;
    fn line_of(&self, byte: usize) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl SourceText for str {
    fn len(&self) -> usize {
        str::len(self)
    }

    fn slice(&self, range: Range<usize>) -> Cow<'_, str> {
        Cow::Borrowed(&self[range])
    }

    fn line_of(&self, byte: usize) -> usize {
        self.as_bytes()[..byte.min(str::len(self))]
            .iter()
            .filter(|value| **value == b'\n')
            .count()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Update {
    Unchanged,
    /// Blocks `old` (pre-update indices) were replaced by blocks `new` (post-update indices).
    Replaced {
        old: Range<usize>,
        new: Range<usize>,
    },
    Full,
}

#[derive(Debug, Default)]
pub struct PreviewDocument {
    pub blocks: Vec<Block>,
    pub revision: u64,
    refdefs: Vec<RefDef>,
}

impl PreviewDocument {
    pub fn parse(source: &str) -> Self {
        let (blocks, refdefs) = parse_document(source);
        Self {
            blocks,
            revision: 1,
            refdefs,
        }
    }

    pub fn reparse(&mut self, source: &(impl SourceText + ?Sized)) -> Update {
        let text = source.slice(0..source.len());
        let (blocks, refdefs) = parse_document(&text);
        self.blocks = blocks;
        self.refdefs = refdefs;
        self.revision += 1;
        Update::Full
    }

    pub fn apply(&mut self, source: &(impl SourceText + ?Sized), edits: &[Edit]) -> Update {
        match self.try_apply(source, edits) {
            Some(update) => update,
            None => self.reparse(source),
        }
    }

    /// Like `apply`, but returns `None` where `apply` would parse the whole document. `None` does
    /// not leave the model untouched: block and definition ranges may already be shifted for the
    /// edits without the blocks being reparsed, so the caller must parse the whole document before
    /// any further incremental apply.
    pub fn try_apply(
        &mut self,
        source: &(impl SourceText + ?Sized),
        edits: &[Edit],
    ) -> Option<Update> {
        if edits.is_empty() {
            return Some(Update::Unchanged);
        }
        let dirty = self.shift_for_edits(edits)?;
        let count = self.blocks.len();
        let first = self
            .blocks
            .partition_point(|block| block.bytes.end < dirty.start);
        let last = self
            .blocks
            .partition_point(|block| block.bytes.start <= dirty.end);
        let mut low = first.saturating_sub(SENTINELS);
        let mut high = (last.max(first) + SENTINELS).min(count);
        for _ in 0..=MAX_WIDENINGS {
            if low == 0 && high == count {
                break;
            }
            let start = if low == 0 {
                0
            } else {
                self.blocks[low].bytes.start
            };
            let end = if high == count {
                source.len()
            } else {
                self.blocks[high - 1].bytes.end
            };
            let text = source.slice(start..end);
            if text.contains("]:") {
                return None;
            }
            let mut parsed = parse_blocks(&text, start, source.line_of(start), &self.refdefs);
            let start_ok = low == 0 || parsed.first() == Some(&self.blocks[low]);
            let end_ok = high == count || parsed.last() == Some(&self.blocks[high - 1]);
            if start_ok && end_ok {
                // Report only the blocks that differ: the sentinels (and any other block the edit
                // left identical) keep their layouts in the view.
                let previous = &self.blocks[low..high];
                let same_start = previous
                    .iter()
                    .zip(&parsed)
                    .take_while(|(old, new)| old == new)
                    .count();
                let same_end = previous[same_start..]
                    .iter()
                    .rev()
                    .zip(parsed[same_start..].iter().rev())
                    .take_while(|(old, new)| old == new)
                    .count();
                let changed = same_start..parsed.len() - same_end;
                let old = low + same_start..high - same_end;
                let new = low + same_start..low + changed.end;
                self.blocks.splice(old.clone(), parsed.drain(changed));
                self.revision += 1;
                return Some(Update::Replaced { old, new });
            }
            if !start_ok {
                low = low.saturating_sub(1);
            }
            if !end_ok {
                high = (high + 1).min(count);
            }
        }
        None
    }

    /// Moves block and definition ranges through `edits` in order and returns the edited byte
    /// range in final coordinates, or `None` when an edit touches a reference definition (which
    /// needs a full parse).
    fn shift_for_edits(&mut self, edits: &[Edit]) -> Option<Range<usize>> {
        let mut dirty: Option<Range<usize>> = None;
        for edit in edits {
            let removed_end = edit.position + edit.removed;
            if self.refdefs.iter().any(|definition| {
                definition.span.start <= removed_end && edit.position <= definition.span.end
            }) {
                return None;
            }
            // Blocks ending before the edit keep their bytes and lines; skipping them by binary
            // search halves the work for an edit in the middle of a large document.
            let first = self
                .blocks
                .partition_point(|block| block.bytes.end < edit.position);
            for block in &mut self.blocks[first..] {
                block.bytes = shift_range(block.bytes.clone(), edit);
                if block.bytes.start >= edit.position + edit.inserted {
                    block.lines = shift_lines(block.lines.clone(), edit.lines_delta);
                }
            }
            for definition in &mut self.refdefs {
                definition.span = shift_range(definition.span.clone(), edit);
            }
            let touched = edit.position..edit.position + edit.inserted;
            dirty = Some(match dirty {
                None => touched,
                Some(previous) => {
                    let previous = shift_range(previous, edit);
                    previous.start.min(touched.start)..previous.end.max(touched.end)
                }
            });
        }
        dirty
    }
}

fn shift_position(position: usize, edit: &Edit, is_end: bool) -> usize {
    let removed_end = edit.position + edit.removed;
    if position < edit.position || (position == edit.position && !is_end) {
        position
    } else if position >= removed_end {
        position - edit.removed + edit.inserted
    } else {
        edit.position + edit.inserted
    }
}

fn shift_range(range: Range<usize>, edit: &Edit) -> Range<usize> {
    let start = shift_position(range.start, edit, false);
    let end = shift_position(range.end, edit, true).max(start);
    start..end
}

fn shift_lines(lines: Range<usize>, delta: isize) -> Range<usize> {
    let move_line = |line: usize| (line as isize + delta).max(0) as usize;
    move_line(lines.start)..move_line(lines.end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::model::parse_document;

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

    /// xorshift64*: deterministic and dependency-free.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }

        fn below(&mut self, bound: usize) -> usize {
            (self.next() % bound.max(1) as u64) as usize
        }
    }

    fn boundary(text: &str, mut position: usize) -> usize {
        position = position.min(text.len());
        while !text.is_char_boundary(position) {
            position -= 1;
        }
        position
    }

    fn random_edit(rng: &mut Rng, text: &mut String) -> Edit {
        let position = boundary(text, rng.below(text.len() + 1));
        if rng.below(3) == 0 && position < text.len() {
            let end = boundary(text, position + 1 + rng.below(8));
            let end = if end == position {
                boundary(text, text.len())
            } else {
                end
            };
            let removed_text = text[position..end].to_owned();
            text.replace_range(position..end, "");
            Edit {
                position,
                removed: removed_text.len(),
                inserted: 0,
                lines_delta: -(removed_text.matches('\n').count() as isize),
            }
        } else {
            let insert = INSERTS[rng.below(INSERTS.len())];
            text.insert_str(position, insert);
            Edit {
                position,
                removed: 0,
                inserted: insert.len(),
                lines_delta: insert.matches('\n').count() as isize,
            }
        }
    }

    #[test]
    fn incremental_updates_always_equal_a_full_parse() {
        for (fixture_index, fixture) in FIXTURES.iter().enumerate() {
            for seed in 1..=40_u64 {
                let mut rng = Rng(seed * 7919 + fixture_index as u64);
                let mut text = (*fixture).to_owned();
                let mut document = PreviewDocument::parse(&text);
                for step in 0..60 {
                    let batch = 1 + rng.below(4);
                    let edits = (0..batch)
                        .map(|_| random_edit(&mut rng, &mut text))
                        .collect::<Vec<_>>();
                    document.apply(text.as_str(), &edits);
                    let (expected, _) = parse_document(&text);
                    assert_eq!(
                        document.blocks, expected,
                        "fixture {fixture_index}, seed {seed}, step {step}, text {text:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_one_paragraph_edit_replaces_only_nearby_blocks() {
        let mut text = String::from("a\n\nb\n\nc\n\nd\n\ne\n\nf\n\ng\n");
        let mut document = PreviewDocument::parse(&text);
        // Byte 12 is the start of paragraph "e", the fifth of seven blocks.
        text.insert(12, 'x');
        let update = document.apply(
            text.as_str(),
            &[Edit {
                position: 12,
                removed: 0,
                inserted: 1,
                lines_delta: 0,
            }],
        );
        let Update::Replaced { old, new } = update else {
            panic!("expected a partial update, got {update:?}");
        };
        assert_eq!(
            (old, new),
            (4..5, 4..5),
            "only the edited paragraph is reported"
        );
        assert_eq!(document.blocks, parse_document(&text).0);
    }

    #[test]
    fn slices_containing_reference_definitions_force_a_full_parse() {
        let mut text = String::from("[a]\n\n[a]: /one\n");
        let mut document = PreviewDocument::parse(&text);
        text.replace_range(10..13, "two");
        let update = document.apply(
            text.as_str(),
            &[Edit {
                position: 10,
                removed: 3,
                inserted: 3,
                lines_delta: 0,
            }],
        );
        assert_eq!(update, Update::Full);
        assert_eq!(document.blocks, parse_document(&text).0);
    }

    #[test]
    fn the_edit_log_caps_pending_edits_and_then_requests_a_full_parse() {
        let mut log = EditLog::default();
        let edit = Edit {
            position: 0,
            removed: 0,
            inserted: 1,
            lines_delta: 0,
        };
        for _ in 0..MAX_PENDING_EDITS {
            log.record(edit);
        }
        assert!(matches!(log.take(), Pending::Edits(edits) if edits.len() == MAX_PENDING_EDITS));
        for _ in 0..=MAX_PENDING_EDITS {
            log.record(edit);
        }
        assert_eq!(log.take(), Pending::Full);
        assert!(log.is_empty());
    }

    #[test]
    fn restored_pending_edits_are_not_lost() {
        let mut log = EditLog::default();
        let edit = Edit {
            position: 3,
            removed: 1,
            inserted: 0,
            lines_delta: 0,
        };
        log.record(edit);
        let pending = log.take();
        log.restore(pending);
        assert_eq!(log.take(), Pending::Edits(vec![edit]));
    }

    #[test]
    fn try_apply_declines_instead_of_parsing_everything() {
        let mut text = String::from("[a]\n\n[a]: /one\n");
        let mut document = PreviewDocument::parse(&text);
        text.replace_range(10..13, "two");
        let edit = Edit {
            position: 10,
            removed: 3,
            inserted: 3,
            lines_delta: 0,
        };
        assert_eq!(document.try_apply(text.as_str(), &[edit]), None);
        assert_eq!(
            document.try_apply(text.as_str(), &[]),
            Some(Update::Unchanged)
        );
    }

    #[test]
    fn revisions_increase_on_every_change() {
        let mut text = String::from("a\n");
        let mut document = PreviewDocument::parse(&text);
        let first = document.revision;
        text.push('b');
        document.apply(
            text.as_str(),
            &[Edit {
                position: 2,
                removed: 0,
                inserted: 1,
                lines_delta: 0,
            }],
        );
        assert!(document.revision > first);
    }

    #[test]
    fn an_edit_inside_a_div_replaces_only_the_div() {
        let mut text =
            String::from("a\n\nb\n\nc\n\n<div>\n\none\n\ntwo\n\n</div>\n\nd\n\ne\n\nf\n");
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
        assert_eq!(
            update,
            Update::Replaced {
                old: 3..4,
                new: 3..4
            }
        );
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
}
