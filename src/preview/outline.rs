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
