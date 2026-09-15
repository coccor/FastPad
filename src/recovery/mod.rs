pub mod snapshot;

use crate::Result;
use crate::document::{Document, RecoveryId};
use crate::platform::{OwnedHandle, wide_null};
use std::path::{Path, PathBuf};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE,
};

pub use snapshot::{
    Snapshot, SnapshotCandidate, discover_snapshots, discover_snapshots_with, write_snapshot,
};

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

/// Files a close may delete: everything owned after an explicit Discard of a dirty document, or
/// the own snapshot of a clean non-recovered document. A Save decision keeps everything.
pub fn snapshots_removed_on_close(
    root: &Path,
    document: &Document,
    discarded: bool,
) -> Vec<PathBuf> {
    if document.dirty {
        if discarded {
            owned_snapshot_files(root, document)
        } else {
            Vec::new()
        }
    } else if document.recovery_origin.is_none() {
        vec![snapshot::snapshot_path(root, document.recovery_id)]
    } else {
        Vec::new()
    }
}

/// Every recovery ID a process composes shares this name, so a live owner is detectable.
pub fn owner_mutex_name(id: RecoveryId) -> String {
    format!(
        r"Local\FastPad-Recovery-{:016x}-{}",
        (id.0 >> 64) as u64,
        (id.0 >> 32) as u32
    )
}

pub fn create_owner_mutex(id: RecoveryId) -> Result<OwnedHandle> {
    let name = wide_null(&owner_mutex_name(id));
    let raw = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    unsafe { OwnedHandle::from_raw_owned(raw) }
}

pub fn owner_is_alive(id: RecoveryId) -> bool {
    let name = wide_null(&owner_mutex_name(id));
    let raw = unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, 0, name.as_ptr()) };
    unsafe { OwnedHandle::from_raw_owned(raw) }.is_ok()
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
    use super::{
        create_owner_mutex, input_idle, needs_snapshot, next_snapshot_document,
        owned_snapshot_files, owner_is_alive, owner_mutex_name, snapshots_removed_on_close,
    };
    use crate::document::{Document, DocumentId, RecoveryId, RecoveryOrigin};
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
    fn only_discard_or_a_clean_ordinary_close_removes_snapshots() {
        // Break caught: a Save decision (which does not write) or a clean recovered tab deleting
        // the only surviving copy of unsaved text.
        let root = Path::new(r"C:\Recovery");
        let origin = RecoveryOrigin {
            snapshot_path: PathBuf::from(r"C:\Recovery\old.fps"),
            original_path: None,
        };
        let mut dirty_recovered = Document::test_fixture(DocumentId(1), true);
        dirty_recovered.recovery_origin = Some(origin.clone());
        assert_eq!(
            snapshots_removed_on_close(root, &dirty_recovered, true).len(),
            2
        );
        assert!(snapshots_removed_on_close(root, &dirty_recovered, false).is_empty());

        let clean = Document::test_fixture(DocumentId(2), false);
        assert_eq!(snapshots_removed_on_close(root, &clean, false).len(), 1);

        let mut clean_recovered = Document::test_fixture(DocumentId(3), false);
        clean_recovered.recovery_origin = Some(origin);
        assert!(snapshots_removed_on_close(root, &clean_recovered, true).is_empty());
    }

    #[test]
    fn owner_mutex_names_are_shared_by_one_process_and_detect_liveness() {
        // Break caught: a second instance treating a running instance's snapshots as crashed.
        let process_start = 0x1122_3344_5566_7788;
        let first = RecoveryId::compose(process_start, 4242, 7);
        let second = RecoveryId::compose(process_start, 4242, 8);
        assert_eq!(
            owner_mutex_name(first),
            r"Local\FastPad-Recovery-1122334455667788-4242"
        );
        assert_eq!(owner_mutex_name(first), owner_mutex_name(second));
        assert_ne!(
            owner_mutex_name(first),
            owner_mutex_name(RecoveryId::compose(process_start, 4243, 7))
        );

        let live = RecoveryId::compose(0xC0FF_EE00_0000_0000, std::process::id(), 1);
        assert!(!owner_is_alive(live));
        let owner = create_owner_mutex(live).unwrap();
        assert!(owner_is_alive(live));
        drop(owner);
        assert!(!owner_is_alive(live));
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
