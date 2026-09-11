use crate::editor::EditorDocument;
use crate::file::encoding::Encoding;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DocumentId(pub u64);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RecoveryId(pub u128);

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
        }
    }

    pub fn title(&self) -> String {
        let base = self
            .path
            .as_deref()
            .and_then(std::path::Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".to_owned());
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
    use crate::window::tabs::Tabs;
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
    fn closing_the_last_tab_replaces_it_with_a_new_document() {
        // Break caught: removing the final tab can leave the editor without a live document.
        let mut tabs = Tabs::with_document(document(1));
        let closed = tabs
            .close_active(CloseDecision::Discard, || document(2))
            .unwrap();
        assert_eq!(closed.id, DocumentId(1));
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs.active().id, DocumentId(2));
    }

    #[test]
    fn cancel_preserves_dirty_tab_and_order() {
        // Break caught: a cancelled dirty close can still remove or reorder a document.
        let mut tabs = Tabs::from_documents([dirty_document(1), document(2)]);
        tabs.activate(DocumentId(1)).unwrap();
        assert_eq!(
            tabs.close_active(CloseDecision::Cancel, || document(3)),
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
        let mut tabs = Tabs::from_documents([document(1), document(2), document(3)]);
        tabs.activate(DocumentId(2)).unwrap();

        let closed = tabs
            .close_active(CloseDecision::Discard, || document(4))
            .unwrap();

        assert_eq!(closed.id, DocumentId(2));
        assert_eq!(tabs.active().id, DocumentId(3));
        assert_eq!(
            tabs.ids().collect::<Vec<_>>(),
            [DocumentId(1), DocumentId(3)]
        );
    }

    #[test]
    fn confirmed_save_allows_a_dirty_document_to_close() {
        // Break caught: the tab model can ignore a caller's successful save decision and leave
        // the already-saved document open.
        let mut tabs = Tabs::from_documents([dirty_document(1), document(2)]);

        let closed = tabs
            .close_active(CloseDecision::Save, || document(3))
            .unwrap();

        assert_eq!(closed.id, DocumentId(1));
        assert_eq!(tabs.active().id, DocumentId(2));
    }

    #[test]
    fn save_point_notifications_change_only_the_active_document() {
        // Break caught: Scintilla save-point notifications can mark every tab, or a stale tab,
        // instead of the document installed in the editor.
        let mut tabs = Tabs::from_documents([document(1), document(2)]);
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
    fn dirty_titles_expose_the_save_point_state() {
        // Break caught: title-strip and accessibility snapshots cannot show which document has
        // left its save point.
        let tabs = Tabs::from_documents([document(1), dirty_document(2)]);
        assert_eq!(
            tabs.titles().collect::<Vec<_>>(),
            ["Untitled", "Untitled *"]
        );
    }

    #[test]
    fn closing_a_tab_releases_its_single_owned_native_reference() {
        // Break caught: keeping a second mutable handle beside document metadata leaks a native
        // Scintilla document reference when the tab is closed.
        let releases = Arc::new(AtomicUsize::new(0));
        let handle =
            crate::editor::EditorDocument::test_fixture_with_release_counter(Arc::clone(&releases));
        let first = Document::untitled(DocumentId(1), super::RecoveryId(1), handle);
        let mut tabs = Tabs::from_documents([first, document(2)]);

        let closed = tabs
            .close_active(CloseDecision::Discard, || document(3))
            .unwrap();
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
        let mut tabs = Tabs::from_documents([first, second]);

        tabs.clear_for_shutdown();

        assert_eq!(releases.load(Ordering::SeqCst), 2);
        assert_eq!(tabs.len(), 0);
    }
}
