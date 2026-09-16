//! Block heights for the virtualized preview: estimates until a block is laid out, exact heights
//! afterwards, with running sums rebuilt lazily from the first changed block.

use crate::preview::model::Block;
use std::ops::Range;

#[derive(Debug, Default)]
pub struct HeightIndex {
    heights: Vec<f32>,
    measured: Vec<bool>,
    /// `prefix[i]` is the sum of `heights[..i]`; entries `0..=valid` are current.
    prefix: Vec<f32>,
    valid: usize,
}

impl HeightIndex {
    pub fn reset(&mut self, estimates: impl IntoIterator<Item = f32>) {
        self.heights = estimates.into_iter().collect();
        self.measured = vec![false; self.heights.len()];
        self.prefix = vec![0.0; self.heights.len() + 1];
        self.valid = 0;
    }

    pub fn splice(&mut self, old: Range<usize>, estimates: &[f32]) {
        self.heights.splice(old.clone(), estimates.iter().copied());
        self.measured
            .splice(old.clone(), std::iter::repeat_n(false, estimates.len()));
        self.prefix.resize(self.heights.len() + 1, 0.0);
        self.valid = self.valid.min(old.start);
    }

    pub fn len(&self) -> usize {
        self.heights.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heights.is_empty()
    }

    pub fn height(&self, index: usize) -> f32 {
        self.heights.get(index).copied().unwrap_or(0.0)
    }

    pub fn is_measured(&self, index: usize) -> bool {
        self.measured.get(index).copied().unwrap_or(false)
    }

    pub fn set_measured(&mut self, index: usize, height: f32) {
        if index < self.heights.len() {
            self.heights[index] = height;
            self.measured[index] = true;
            self.valid = self.valid.min(index);
        }
    }

    fn ensure_prefix(&mut self, upto: usize) {
        if self.prefix.len() != self.heights.len() + 1 {
            self.prefix.resize(self.heights.len() + 1, 0.0);
        }
        while self.valid < upto {
            self.prefix[self.valid + 1] = self.prefix[self.valid] + self.heights[self.valid];
            self.valid += 1;
        }
    }

    pub fn top(&mut self, index: usize) -> f32 {
        let index = index.min(self.heights.len());
        self.ensure_prefix(index);
        self.prefix[index]
    }

    pub fn total(&mut self) -> f32 {
        self.top(self.heights.len())
    }

    /// The block containing `y`, clamped to the last block.
    pub fn index_at(&mut self, y: f32) -> usize {
        let count = self.heights.len();
        if count == 0 {
            return 0;
        }
        self.ensure_prefix(count);
        self.prefix[..count]
            .partition_point(|top| *top <= y)
            .saturating_sub(1)
    }

    pub fn anchor(&mut self, scroll_y: f32) -> (usize, f32) {
        let index = self.index_at(scroll_y);
        (index, scroll_y - self.top(index))
    }

    pub fn scroll_for_anchor(&mut self, (index, within): (usize, f32)) -> f32 {
        self.top(index) + within.min(self.height(index))
    }
}

pub fn estimate_height(line_count: usize, line_height: f32, gap: f32) -> f32 {
    line_count.max(1) as f32 * line_height + gap
}

pub fn offset_for_line(blocks: &[Block], heights: &mut HeightIndex, line: usize) -> f32 {
    let index = blocks.partition_point(|block| block.lines.end <= line);
    let Some(block) = blocks.get(index) else {
        return heights.total();
    };
    let top = heights.top(index);
    if line < block.lines.start {
        return top;
    }
    let span = block.lines.len().max(1) as f32;
    top + (line - block.lines.start) as f32 / span * heights.height(index)
}

pub fn line_for_offset(blocks: &[Block], heights: &mut HeightIndex, y: f32) -> usize {
    if blocks.is_empty() {
        return 0;
    }
    let index = heights.index_at(y.max(0.0)).min(blocks.len() - 1);
    let block = &blocks[index];
    let height = heights.height(index).max(1.0);
    let fraction = ((y - heights.top(index)) / height).clamp(0.0, 1.0);
    let span = block.lines.len().max(1);
    let offset = ((fraction * span as f32) + 1e-3).floor() as usize;
    block.lines.start + offset.min(span - 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::model::parse_document;

    #[test]
    fn tops_are_running_sums_and_update_after_measurement() {
        let mut heights = HeightIndex::default();
        heights.reset([10.0, 20.0, 30.0]);
        assert_eq!(heights.top(0), 0.0);
        assert_eq!(heights.top(2), 30.0);
        assert_eq!(heights.total(), 60.0);
        heights.set_measured(0, 15.0);
        assert!(heights.is_measured(0));
        assert_eq!(heights.top(2), 35.0);
        assert_eq!(heights.index_at(34.9), 1);
        assert_eq!(heights.index_at(35.0), 2);
        assert_eq!(heights.index_at(1_000.0), 2);
    }

    #[test]
    fn splice_replaces_estimates_and_forgets_measurements() {
        let mut heights = HeightIndex::default();
        heights.reset([10.0, 10.0, 10.0]);
        heights.set_measured(1, 50.0);
        heights.splice(1..2, &[5.0, 5.0]);
        assert_eq!(heights.len(), 4);
        assert!(!heights.is_measured(1));
        assert_eq!(heights.total(), 30.0);
    }

    #[test]
    fn anchors_keep_the_first_visible_block_in_place() {
        let mut heights = HeightIndex::default();
        heights.reset([10.0, 10.0, 10.0]);
        let anchor = heights.anchor(15.0);
        assert_eq!(anchor, (1, 5.0));
        heights.set_measured(0, 30.0);
        assert_eq!(heights.scroll_for_anchor(anchor), 35.0);
    }

    #[test]
    fn lines_and_offsets_round_trip_inside_blocks() {
        let source = "# A\n\none\ntwo\nthree\n\n- x\n- y\n";
        let (blocks, _) = parse_document(source);
        let mut heights = HeightIndex::default();
        heights.reset(
            blocks
                .iter()
                .map(|block| estimate_height(block.lines.len(), 20.0, 16.0)),
        );
        for block in &blocks {
            for line in block.lines.clone() {
                let offset = offset_for_line(&blocks, &mut heights, line);
                assert_eq!(
                    line_for_offset(&blocks, &mut heights, offset),
                    line,
                    "line {line}"
                );
            }
        }
    }

    #[test]
    fn offsets_grow_with_source_lines() {
        let (blocks, _) = parse_document("a\n\nb\n\nc\n");
        let mut heights = HeightIndex::default();
        heights.reset(blocks.iter().map(|_| 36.0));
        assert!(
            offset_for_line(&blocks, &mut heights, 0) < offset_for_line(&blocks, &mut heights, 2)
        );
        assert!(
            offset_for_line(&blocks, &mut heights, 2) < offset_for_line(&blocks, &mut heights, 4)
        );
        assert_eq!(offset_for_line(&blocks, &mut heights, 99), heights.total());
    }
}
