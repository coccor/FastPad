use crate::editor::EditorDocument;
use crate::file::encoding::Encoding;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DocumentId(pub u64);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RecoveryId(pub u128);

impl RecoveryId {
    pub const fn from_u128(value: u128) -> Self {
        Self(value)
    }

    /// High 64 bits: process-start counter. Low 64 bits: PID, then the low 32 counter bits.
    pub const fn compose(process_start: u64, pid: u32, counter: u64) -> Self {
        let low = ((pid as u64) << 32) | (counter & 0xffff_ffff);
        Self(((process_start as u128) << 64) | low as u128)
    }

    pub const fn is_from_process(self, process_start: u64, pid: u32) -> bool {
        (self.0 >> 64) as u64 == process_start && (self.0 >> 32) as u32 == pid
    }
}

/// Where a recovered tab came from: its source snapshot file and original document path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryOrigin {
    pub snapshot_path: PathBuf,
    pub original_path: Option<PathBuf>,
    /// Restored from the last session rather than after a crash; titled like any other tab.
    pub from_session: bool,
}

impl RecoveryOrigin {
    pub fn display_name(&self) -> String {
        file_name_or_untitled(self.original_path.as_deref())
    }
}

fn file_name_or_untitled(path: Option<&std::path::Path>) -> String {
    path.and_then(std::path::Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".to_owned())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Language {
    PlainText,
    Json,
    Markdown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CloseDecision {
    Save,
    Discard,
    Cancel,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CloseCancelled;

#[derive(Debug)]
pub struct Document {
    pub id: DocumentId,
    pub handle: EditorDocument,
    pub path: Option<PathBuf>,
    pub language: Language,
    pub encoding: Encoding,
    pub dirty: bool,
    pub recovery_id: RecoveryId,
    pub generation: u64,
    pub recovery_generation: Option<u64>,
    pub recovery_origin: Option<RecoveryOrigin>,
}

impl PartialEq for Document {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.path == other.path
            && self.language == other.language
            && self.encoding == other.encoding
            && self.dirty == other.dirty
            && self.recovery_id == other.recovery_id
            && self.generation == other.generation
            && self.recovery_generation == other.recovery_generation
            && self.recovery_origin == other.recovery_origin
    }
}

impl Eq for Document {}

impl Document {
    pub fn untitled(id: DocumentId, recovery_id: RecoveryId, handle: EditorDocument) -> Self {
        Self {
            id,
            handle,
            path: None,
            language: Language::PlainText,
            encoding: Encoding::Utf8,
            dirty: false,
            recovery_id,
            generation: 0,
            recovery_generation: None,
            recovery_origin: None,
        }
    }

    pub fn title(&self) -> String {
        let base = match (&self.path, &self.recovery_origin) {
            (None, Some(origin)) if !origin.from_session => {
                format!("Recovered: {}", origin.display_name())
            }
            // A session tab reopened unbound, because its file was already open elsewhere,
            // still names that file.
            (None, Some(origin)) => origin.display_name(),
            (path, _) => file_name_or_untitled(path.as_deref()),
        };
        if self.dirty {
            format!("{base} *")
        } else {
            base
        }
    }

    #[cfg(test)]
    pub fn test_fixture(id: DocumentId, dirty: bool) -> Self {
        let mut document = Self::untitled(
            id,
            RecoveryId(u128::from(id.0)),
            EditorDocument::test_fixture(),
        );
        document.dirty = dirty;
        document
    }
}

#[cfg(test)]
mod tests {
    use super::{CloseCancelled, CloseDecision, Document, DocumentId};
    use crate::window::tabs::{CloseReviewError, Tabs};
    use std::fs;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn document(id: u64) -> Document {
        Document::test_fixture(DocumentId(id), false)
    }

    fn dirty_document(id: u64) -> Document {
        Document::test_fixture(DocumentId(id), true)
    }

    #[test]
    fn recovery_ids_pack_process_start_pid_and_counter() {
        // Break caught: IDs from two processes or two documents colliding on one snapshot file.
        let id = super::RecoveryId::compose(0x1122_3344_5566_7788, 0xAABB_CCDD, 0x1_0000_0005);
        assert_eq!(id.0, 0x1122_3344_5566_7788_AABB_CCDD_0000_0005);
        assert!(id.is_from_process(0x1122_3344_5566_7788, 0xAABB_CCDD));
        assert!(!id.is_from_process(0x1122_3344_5566_7788, 0xAABB_CCDE));
        assert!(!id.is_from_process(0x1122_3344_5566_7789, 0xAABB_CCDD));
        assert_ne!(
            super::RecoveryId::compose(1, 2, 3),
            super::RecoveryId::compose(1, 2, 4)
        );
    }

    #[test]
    fn recovered_documents_are_titled_by_their_origin_until_saved() {
        // Break caught: a recovered tab looking like an ordinary untitled or original-file tab.
        let mut recovered = dirty_document(1);
        recovered.recovery_origin = Some(super::RecoveryOrigin {
            snapshot_path: std::path::PathBuf::from("x.fps"),
            original_path: Some(std::path::PathBuf::from(r"C:\docs\notes.md")),
            from_session: false,
        });
        assert_eq!(recovered.title(), "Recovered: notes.md *");
        recovered.recovery_origin.as_mut().unwrap().original_path = None;
        assert_eq!(recovered.title(), "Recovered: Untitled *");
        recovered.path = Some(std::path::PathBuf::from(r"C:\docs\saved.txt"));
        assert_eq!(recovered.title(), "saved.txt *");
    }

    #[test]
    fn session_restored_untitled_tabs_are_not_called_recovered() {
        // Break caught: every unsaved tab from a normal close reappearing as "Recovered: ...",
        // as if FastPad had crashed.
        let mut document = Document::test_fixture(DocumentId(1), true);
        document.recovery_origin = Some(super::RecoveryOrigin {
            snapshot_path: std::path::PathBuf::from(r"C:\Recovery\a.fps"),
            original_path: None,
            from_session: true,
        });
        assert_eq!(document.title(), "Untitled *");
        document.recovery_origin.as_mut().unwrap().from_session = false;
        assert_eq!(document.title(), "Recovered: Untitled *");
    }

    #[test]
    fn session_tabs_reopened_without_their_path_keep_the_file_name() {
        // Break caught: an unsaved session tab whose file was already open in another tab coming
        // back as a bare "Untitled", so nothing says which file its text belongs to.
        let mut document = Document::test_fixture(DocumentId(1), true);
        document.recovery_origin = Some(super::RecoveryOrigin {
            snapshot_path: std::path::PathBuf::from(r"C:\Recovery\a.fps"),
            original_path: Some(std::path::PathBuf::from(r"C:\docs\notes.md")),
            from_session: true,
        });
        assert_eq!(document.title(), "notes.md *");
    }

    #[test]
    fn closing_the_last_tab_leaves_no_tabs_open() {
        // Break caught: the final tab being silently replaced, so its close button looks inert.
        let mut tabs = Tabs::with_document(document(1));
        let closed = tabs.close_active(CloseDecision::Discard).unwrap();
        assert_eq!(closed.id, DocumentId(1));
        assert!(tabs.is_empty());
        assert!(tabs.active().is_none());
        assert_eq!(
            tabs.close_active(CloseDecision::Discard),
            Err(CloseCancelled)
        );
    }

    #[test]
    fn cancel_preserves_dirty_tab_and_order() {
        // Break caught: a cancelled dirty close can still remove or reorder a document.
        let mut tabs = Tabs::from_documents([dirty_document(1), document(2)]).unwrap();
        tabs.activate(DocumentId(1)).unwrap();
        assert_eq!(
            tabs.close_active(CloseDecision::Cancel),
            Err(CloseCancelled)
        );
        assert_eq!(
            tabs.ids().collect::<Vec<_>>(),
            [DocumentId(1), DocumentId(2)]
        );
    }

    #[test]
    fn closing_an_active_middle_tab_selects_its_successor() {
        // Break caught: closing a middle document can select the previous tab or leave a stale
        // active index.
        let mut tabs = Tabs::from_documents([document(1), document(2), document(3)]).unwrap();
        tabs.activate(DocumentId(2)).unwrap();

        let closed = tabs.close_active(CloseDecision::Discard).unwrap();

        assert_eq!(closed.id, DocumentId(2));
        assert_eq!(tabs.active().unwrap().id, DocumentId(3));
        assert_eq!(
            tabs.ids().collect::<Vec<_>>(),
            [DocumentId(1), DocumentId(3)]
        );
    }

    #[test]
    fn save_decision_closes_only_a_document_that_is_no_longer_dirty() {
        // Break caught: a Save answer closing a tab whose save never happened or failed, which
        // discards the only copy of its unsaved text.
        let mut tabs = Tabs::from_documents([dirty_document(1), document(2)]).unwrap();
        let unsaved = tabs.active_close_review().unwrap();

        assert_eq!(
            tabs.close_reviewed(unsaved, CloseDecision::Save),
            Err(CloseReviewError::Unsaved)
        );
        assert_eq!(
            tabs.ids().collect::<Vec<_>>(),
            [DocumentId(1), DocumentId(2)]
        );

        assert!(tabs.set_active_dirty(false));
        let saved = tabs.active_close_review().unwrap();
        let closed = tabs.close_reviewed(saved, CloseDecision::Save).unwrap();

        assert_eq!(closed.id, DocumentId(1));
        assert_eq!(tabs.active().unwrap().id, DocumentId(2));
    }

    #[test]
    fn save_point_notifications_change_only_the_active_document() {
        // Break caught: Scintilla save-point notifications can mark every tab, or a stale tab,
        // instead of the document installed in the editor.
        let mut tabs = Tabs::from_documents([document(1), document(2)]).unwrap();
        tabs.activate(DocumentId(2)).unwrap();

        assert!(tabs.set_active_dirty(true));
        assert!(!tabs.set_active_dirty(true));
        assert!(!tabs.document(DocumentId(1)).unwrap().dirty);
        assert!(tabs.document(DocumentId(2)).unwrap().dirty);
        assert!(tabs.set_active_dirty(false));
    }

    #[test]
    fn duplicate_canonical_paths_are_rejected() {
        // Break caught: alternate spellings of one path can open the same file into two native
        // documents and create competing save ownership.
        let root = std::env::temp_dir().join(format!(
            "fastpad-task9-path-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("same.txt");
        fs::write(&path, b"same").unwrap();
        let alternate = root.join(".").join("same.txt");

        let mut first = document(1);
        first.path = Some(path);
        let mut duplicate = document(2);
        duplicate.path = Some(alternate);
        let mut tabs = Tabs::with_document(first);

        assert!(tabs.push(duplicate).is_err());
        assert_eq!(tabs.ids().collect::<Vec<_>>(), [DocumentId(1)]);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bulk_construction_rejects_duplicate_canonical_paths() {
        // Break caught: constructing Tabs from an iterator can bypass the canonical-path
        // ownership invariant enforced by push.
        let root = std::env::temp_dir().join(format!(
            "fastpad-task9-bulk-path-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("same.txt");
        fs::write(&path, b"same").unwrap();

        let mut first = document(1);
        first.path = Some(path.clone());
        let mut duplicate = document(2);
        duplicate.path = Some(root.join(".").join("same.txt"));

        assert!(Tabs::from_documents([first, duplicate]).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reentrant_active_change_invalidates_a_close_review() {
        // Break caught: a modal prompt can reenter, activate a different tab, and make the old
        // decision close whichever document happens to be active afterward.
        let mut tabs = Tabs::from_documents([dirty_document(1), document(2)]).unwrap();
        let review = tabs.active_close_review().unwrap();
        tabs.activate(DocumentId(2)).unwrap();

        assert_eq!(
            tabs.close_reviewed(review, CloseDecision::Discard),
            Err(CloseReviewError::Stale)
        );
        assert_eq!(
            tabs.ids().collect::<Vec<_>>(),
            [DocumentId(1), DocumentId(2)]
        );
    }

    #[test]
    fn reentrant_dirty_change_is_included_in_window_close_review() {
        // Break caught: snapshotting dirty IDs once before modal prompts can silently omit a tab
        // that becomes dirty while an earlier document is being reviewed.
        let mut tabs = Tabs::from_documents([dirty_document(1), document(2)]).unwrap();
        let first = tabs.next_dirty_review(&[]).unwrap();

        tabs.activate(DocumentId(2)).unwrap();
        tabs.set_active_dirty(true);
        let reviewed = [first.key()];

        assert_eq!(tabs.next_dirty_review(&reviewed).unwrap().id, DocumentId(2));
    }

    #[test]
    fn dirty_titles_expose_the_save_point_state() {
        // Break caught: title-strip and accessibility snapshots cannot show which document has
        // left its save point.
        let tabs = Tabs::from_documents([document(1), dirty_document(2)]).unwrap();
        assert_eq!(
            tabs.titles().collect::<Vec<_>>(),
            ["Untitled", "Untitled *"]
        );
    }

    #[test]
    fn a_retained_live_view_is_empty_after_its_tab_owner_is_dropped() {
        // Break caught: retained accessibility providers expose documents after App teardown.
        let tabs = Tabs::with_document(document(1));
        let view = tabs.view();
        drop(tabs);
        assert!(view.snapshot().tabs.is_empty());
    }

    #[test]
    fn closing_a_tab_releases_its_single_owned_native_reference() {
        // Break caught: keeping a second mutable handle beside document metadata leaks a native
        // Scintilla document reference when the tab is closed.
        let releases = Arc::new(AtomicUsize::new(0));
        let handle =
            crate::editor::EditorDocument::test_fixture_with_release_counter(Arc::clone(&releases));
        let first = Document::untitled(DocumentId(1), super::RecoveryId(1), handle);
        let mut tabs = Tabs::from_documents([first, document(2)]).unwrap();

        let closed = tabs.close_active(CloseDecision::Discard).unwrap();
        assert_eq!(releases.load(Ordering::SeqCst), 0);
        drop(closed);
        assert_eq!(releases.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn shutdown_clear_releases_every_owned_document_reference() {
        // Break caught: dropping the editor HWND before Tabs drains its native document owners
        // makes SCI_RELEASEDOCUMENT target an invalid endpoint during shutdown.
        let releases = Arc::new(AtomicUsize::new(0));
        let first = Document::untitled(
            DocumentId(1),
            super::RecoveryId(1),
            crate::editor::EditorDocument::test_fixture_with_release_counter(Arc::clone(&releases)),
        );
        let second = Document::untitled(
            DocumentId(2),
            super::RecoveryId(2),
            crate::editor::EditorDocument::test_fixture_with_release_counter(Arc::clone(&releases)),
        );
        let mut tabs = Tabs::from_documents([first, second]).unwrap();

        tabs.clear_for_shutdown();

        assert_eq!(releases.load(Ordering::SeqCst), 2);
        assert_eq!(tabs.len(), 0);
    }
}
