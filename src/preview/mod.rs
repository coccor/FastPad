//! Markdown preview: a block model, incremental reparsing, and a lazily created Direct2D view.
//! Nothing here runs until the user opens a preview.

pub mod incremental;
pub mod model;

/// How the preview shares the content area with the editor. Owned by the window, not the tab.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PreviewMode {
    #[default]
    Off,
    Split,
    Full,
}

/// Documents larger than this reparse fully on a worker thread.
pub const WORKER_PARSE_THRESHOLD: usize = 1_048_576;
/// Documents larger than this pause live updates behind a "Refresh preview" bar.
pub const LIVE_UPDATE_LIMIT: usize = 10_485_760;
/// Idle time after the last edit before the preview updates.
pub const PREVIEW_UPDATE_DELAY_MS: u32 = 120;
