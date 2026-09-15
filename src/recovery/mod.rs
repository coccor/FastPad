pub mod snapshot;

use crate::Result;
use crate::document::Document;
use std::path::{Path, PathBuf};

pub use snapshot::{Snapshot, SnapshotCandidate, discover_snapshots, write_snapshot};

pub const RECOVERY_TIMER_ID: usize = 0x4650_5243;
pub const IDLE_THRESHOLD_MS: u32 = 2_000;

pub fn recovery_root() -> Result<PathBuf> {
    Ok(crate::platform::paths::fastpad_data_dir()?.join("Recovery"))
}

/// Tick counts wrap every ~49.7 days, so elapsed time is measured with wrapping subtraction.
pub fn input_idle(last_input_tick: u32, now_tick: u32) -> bool {
    now_tick.wrapping_sub(last_input_tick) >= IDLE_THRESHOLD_MS
}

pub fn timer_period_ms(interval_seconds: u32) -> u32 {
    interval_seconds.max(1).saturating_mul(1_000)
}

pub fn needs_snapshot(document: &Document) -> bool {
    document.dirty && document.recovery_generation != Some(document.generation)
}

pub fn next_snapshot_document<'a>(
    documents: impl IntoIterator<Item = &'a Document>,
) -> Option<&'a Document> {
    documents
        .into_iter()
        .find(|document| needs_snapshot(document))
}

/// Snapshot files owned by `document`: its own, plus the source it was recovered from.
pub fn owned_snapshot_files(root: &Path, document: &Document) -> Vec<PathBuf> {
    let mut files = vec![snapshot::snapshot_path(root, document.recovery_id)];
    files.extend(
        document
            .recovery_origin
            .as_ref()
            .map(|origin| origin.snapshot_path.clone()),
    );
    files
}

pub fn remove_snapshot_files(files: &[PathBuf]) {
    for file in files {
        let _ = std::fs::remove_file(file);
    }
}

pub fn recovered_notice(count: usize) -> String {
    if count == 1 {
        "Recovered 1 unsaved document from a previous session. Save it to keep its contents."
            .to_owned()
    } else {
        format!(
            "Recovered {count} unsaved documents from a previous session. Save them to keep \
             their contents."
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{input_idle, needs_snapshot, next_snapshot_document, owned_snapshot_files};
    use crate::document::{Document, DocumentId, RecoveryOrigin};
    use std::path::{Path, PathBuf};

    #[test]
    fn idle_requires_two_seconds_without_input_across_tick_wraparound() {
        assert!(!input_idle(10_000, 11_999));
        assert!(input_idle(10_000, 12_000));
        assert!(input_idle(u32::MAX - 500, 1_500));
        assert!(!input_idle(u32::MAX - 500, 1_000));
    }

    #[test]
    fn only_dirty_documents_with_an_unrecorded_generation_are_selected() {
        // Break caught: rewriting an unchanged snapshot every tick, or snapshotting clean tabs.
        let clean = Document::test_fixture(DocumentId(1), false);
        let mut recorded = Document::test_fixture(DocumentId(2), true);
        recorded.generation = 4;
        recorded.recovery_generation = Some(4);
        let mut changed = Document::test_fixture(DocumentId(3), true);
        changed.generation = 5;
        changed.recovery_generation = Some(4);
        let fresh = Document::test_fixture(DocumentId(4), true);

        assert!(!needs_snapshot(&clean));
        assert!(!needs_snapshot(&recorded));
        assert!(needs_snapshot(&changed));
        assert!(needs_snapshot(&fresh));
        let documents = [clean, recorded, changed, fresh];
        assert_eq!(
            next_snapshot_document(&documents).map(|document| document.id),
            Some(DocumentId(3))
        );
    }

    #[test]
    fn recovered_documents_own_their_source_snapshot_too() {
        let root = Path::new(r"C:\Recovery");
        let mut document = Document::test_fixture(DocumentId(1), true);
        assert_eq!(owned_snapshot_files(root, &document).len(), 1);
        document.recovery_origin = Some(RecoveryOrigin {
            snapshot_path: PathBuf::from(r"C:\Recovery\old.fps"),
            original_path: None,
        });
        let files = owned_snapshot_files(root, &document);
        assert_eq!(files.len(), 2);
        assert_eq!(files[1], PathBuf::from(r"C:\Recovery\old.fps"));
    }
}
