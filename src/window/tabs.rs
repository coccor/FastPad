use crate::document::{CloseCancelled, CloseDecision, Document, DocumentId};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

#[derive(Clone, Debug)]
pub(crate) struct TabSelection {
    active: Arc<AtomicUsize>,
}

impl TabSelection {
    pub(crate) fn new(active: usize) -> Self {
        Self {
            active: Arc::new(AtomicUsize::new(active)),
        }
    }

    pub(crate) fn active_index(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }

    pub(crate) fn select(&self, index: usize, tab_count: usize) -> bool {
        if index >= tab_count {
            return false;
        }
        self.active.store(index, Ordering::Release);
        true
    }
}

#[derive(Clone, Debug)]
pub(crate) struct TabView {
    state: Arc<RwLock<TabViewState>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TabViewSnapshot {
    pub(crate) revision: u64,
    pub(crate) tabs: Vec<TabViewTab>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TabViewTab {
    pub(crate) id: DocumentId,
    pub(crate) title: String,
}

#[derive(Debug)]
struct TabViewState {
    revision: u64,
    tabs: Vec<TabViewTab>,
}

impl TabView {
    fn new(documents: &[Document]) -> Self {
        Self {
            state: Arc::new(RwLock::new(TabViewState {
                revision: 0,
                tabs: view_tabs(documents),
            })),
        }
    }

    pub(crate) fn snapshot(&self) -> TabViewSnapshot {
        let state = self.state.read().unwrap_or_else(|error| error.into_inner());
        TabViewSnapshot {
            revision: state.revision,
            tabs: state.tabs.clone(),
        }
    }

    fn update(&self, documents: &[Document]) {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(|error| error.into_inner());
        state.revision = state.revision.saturating_add(1);
        state.tabs = view_tabs(documents);
    }
}

fn view_tabs(documents: &[Document]) -> Vec<TabViewTab> {
    documents
        .iter()
        .map(|document| TabViewTab {
            id: document.id,
            title: document.title(),
        })
        .collect()
}

#[derive(Debug)]
pub struct Tabs {
    documents: Vec<Document>,
    selection: TabSelection,
    view: TabView,
}

impl Tabs {
    pub fn new() -> Self {
        let documents = Vec::new();
        Self {
            view: TabView::new(&documents),
            documents,
            selection: TabSelection::new(0),
        }
    }

    pub fn with_document(document: Document) -> Self {
        let documents = vec![document];
        Self {
            view: TabView::new(&documents),
            documents,
            selection: TabSelection::new(0),
        }
    }

    pub fn from_documents(
        documents: impl IntoIterator<Item = Document>,
    ) -> Result<Self, DuplicateDocumentPath> {
        let documents = documents.into_iter().collect::<Vec<_>>();
        validate_unique_paths(&documents)?;
        Ok(Self {
            view: TabView::new(&documents),
            documents,
            selection: TabSelection::new(0),
        })
    }

    pub fn len(&self) -> usize {
        self.documents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.documents.is_empty()
    }

    pub fn active_index(&self) -> usize {
        self.selection.active_index()
    }

    pub(crate) fn selection(&self) -> TabSelection {
        self.selection.clone()
    }

    pub(crate) fn view(&self) -> TabView {
        self.view.clone()
    }

    pub fn active(&self) -> &Document {
        &self.documents[self.active_index()]
    }

    pub fn document(&self, id: DocumentId) -> Option<&Document> {
        self.documents.iter().find(|document| document.id == id)
    }

    pub(crate) fn find_path(&self, path: &Path) -> Option<DocumentId> {
        let key = canonical_key(path).ok()?;
        self.documents
            .iter()
            .find(|document| {
                document
                    .path
                    .as_deref()
                    .and_then(|path| canonical_key(path).ok())
                    .is_some_and(|path| path == key)
            })
            .map(|document| document.id)
    }

    pub(crate) fn replace_active_untitled(&mut self, document: Document) -> Document {
        let index = self.active_index();
        let old = std::mem::replace(&mut self.documents[index], document);
        self.view.update(&self.documents);
        old
    }

    #[cfg(test)]
    pub fn ids(&self) -> impl Iterator<Item = DocumentId> + '_ {
        self.documents.iter().map(|document| document.id)
    }

    pub fn titles(&self) -> impl Iterator<Item = String> + '_ {
        self.documents.iter().map(Document::title)
    }

    pub fn activate(&mut self, id: DocumentId) -> Result<(), UnknownDocument> {
        let index = self
            .documents
            .iter()
            .position(|document| document.id == id)
            .ok_or(UnknownDocument(id))?;
        self.selection.select(index, self.documents.len());
        Ok(())
    }

    pub fn activate_index(&mut self, index: usize) -> Result<(), UnknownDocument> {
        let id = self
            .documents
            .get(index)
            .map(|document| document.id)
            .ok_or(UnknownDocument(DocumentId(u64::MAX)))?;
        self.activate(id)
    }

    pub fn push(&mut self, document: Document) -> Result<(), DuplicateDocumentPath> {
        if let Some(path) = document.path.as_deref() {
            let candidate = canonical_key(path)?;
            if self.documents.iter().any(|existing| {
                existing
                    .path
                    .as_deref()
                    .and_then(|path| canonical_key(path).ok())
                    .is_some_and(|path| path == candidate)
            }) {
                return Err(DuplicateDocumentPath(candidate));
            }
        }
        self.documents.push(document);
        let index = self.documents.len() - 1;
        self.selection.select(index, self.documents.len());
        self.view.update(&self.documents);
        Ok(())
    }

    pub fn close_active<F>(
        &mut self,
        decision: CloseDecision,
        replacement: F,
    ) -> Result<Document, CloseCancelled>
    where
        F: FnOnce() -> Document,
    {
        if decision == CloseDecision::Cancel {
            return Err(CloseCancelled);
        }
        let index = self.active_index();
        if self.documents.len() == 1 {
            let closed = std::mem::replace(&mut self.documents[0], replacement());
            self.selection.select(0, 1);
            self.view.update(&self.documents);
            return Ok(closed);
        }
        let closed = self.documents.remove(index);
        self.selection
            .select(index.min(self.documents.len() - 1), self.documents.len());
        self.view.update(&self.documents);
        Ok(closed)
    }

    pub fn active_close_review(&self) -> Option<CloseReview> {
        let document = self.documents.get(self.active_index())?;
        Some(CloseReview {
            id: document.id,
            generation: document.generation,
        })
    }

    pub fn close_reviewed(
        &mut self,
        review: CloseReview,
        decision: CloseDecision,
        replacement: Option<Document>,
    ) -> Result<Document, CloseReviewError> {
        if decision == CloseDecision::Cancel {
            return Err(CloseReviewError::Cancelled);
        }
        let Some(index) = self
            .documents
            .iter()
            .position(|document| document.id == review.id)
        else {
            return Err(CloseReviewError::Stale);
        };
        if index != self.active_index() || self.documents[index].generation != review.generation {
            return Err(CloseReviewError::Stale);
        }
        let closed = if self.documents.len() == 1 {
            let replacement = replacement.ok_or(CloseReviewError::MissingReplacement)?;
            std::mem::replace(&mut self.documents[0], replacement)
        } else {
            self.documents.remove(index)
        };
        let active = index.min(self.documents.len() - 1);
        self.selection.select(active, self.documents.len());
        self.view.update(&self.documents);
        Ok(closed)
    }

    pub fn next_dirty_review(&self, reviewed: &[CloseReviewKey]) -> Option<CloseReview> {
        self.documents
            .iter()
            .find(|document| {
                document.dirty
                    && !reviewed.contains(&CloseReviewKey {
                        id: document.id,
                        generation: document.generation,
                    })
            })
            .map(|document| CloseReview {
                id: document.id,
                generation: document.generation,
            })
    }

    pub fn dirty_review_is_current(&self, review: CloseReview) -> bool {
        self.document(review.id)
            .is_some_and(|document| document.dirty && document.generation == review.generation)
    }

    pub fn set_active_dirty(&mut self, dirty: bool) -> bool {
        let active = self.active_index();
        let Some(document) = self.documents.get_mut(active) else {
            return false;
        };
        if document.dirty == dirty {
            return false;
        }
        document.dirty = dirty;
        document.generation = document.generation.saturating_add(1);
        self.view.update(&self.documents);
        true
    }

    /// Records the active document's language after an explicit `LanguageX` command or automatic
    /// detection, independent of applying the actual lexer to the live editor. Returns `false`
    /// (no-op) when the active document already has this language.
    pub(crate) fn set_active_language(&mut self, language: crate::document::Language) -> bool {
        let active = self.active_index();
        let Some(document) = self.documents.get_mut(active) else {
            return false;
        };
        if document.language == language {
            return false;
        }
        document.language = language;
        true
    }

    pub(crate) fn note_active_text_change(&mut self) {
        let active = self.active_index();
        if let Some(document) = self.documents.get_mut(active) {
            document.generation = document.generation.saturating_add(1);
        }
    }

    pub fn clear_for_shutdown(&mut self) {
        self.documents.clear();
        self.selection.active.store(0, Ordering::Release);
        self.view.update(&self.documents);
    }

    pub(crate) fn active_handle(&self) -> &crate::editor::EditorDocument {
        &self.active().handle
    }

    /// Renames the active document's path, e.g. after a successful Save As write. Rejects the
    /// rename if `path` canonicalizes to the same file another open tab already owns, preserving
    /// Task 9's one-native-document-per-canonical-path invariant. A `path` that does not yet exist
    /// on disk (the common brand-new Save As destination) cannot collide with any already-open
    /// document, so it is accepted without a canonicalization check.
    pub(crate) fn set_active_path(&mut self, path: PathBuf) -> Result<(), DuplicateDocumentPath> {
        let active = self.active_index();
        if let Ok(candidate) = canonical_key(&path) {
            let collides = self.documents.iter().enumerate().any(|(index, existing)| {
                index != active
                    && existing
                        .path
                        .as_deref()
                        .and_then(|existing_path| canonical_key(existing_path).ok())
                        .is_some_and(|existing_candidate| existing_candidate == candidate)
            });
            if collides {
                return Err(DuplicateDocumentPath(candidate));
            }
        }
        self.documents[active].path = Some(path);
        self.view.update(&self.documents);
        Ok(())
    }

    /// Reverts the active document's path back to a value it previously, legitimately held (or
    /// `None`, if it had not yet claimed a path), e.g. after a Save As write that renamed the
    /// path via `set_active_path` but then failed before anything was actually written to the
    /// new location. No collision check is needed: `original` was already valid for this
    /// document, so restoring it cannot newly collide with any other tab.
    pub(crate) fn revert_active_path(&mut self, original: Option<PathBuf>) {
        let active = self.active_index();
        self.documents[active].path = original;
        self.view.update(&self.documents);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CloseReview {
    pub id: DocumentId,
    pub generation: u64,
}

impl CloseReview {
    pub fn key(self) -> CloseReviewKey {
        CloseReviewKey {
            id: self.id,
            generation: self.generation,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CloseReviewKey {
    id: DocumentId,
    generation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CloseReviewError {
    Cancelled,
    Stale,
    MissingReplacement,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnknownDocument(pub DocumentId);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DuplicateDocumentPath(pub PathBuf);

fn canonical_key(path: &Path) -> Result<PathBuf, DuplicateDocumentPath> {
    let canonical = path
        .canonicalize()
        .map_err(|_| DuplicateDocumentPath(path.to_path_buf()))?;
    #[cfg(windows)]
    {
        Ok(PathBuf::from(canonical.to_string_lossy().to_lowercase()))
    }
    #[cfg(not(windows))]
    {
        Ok(canonical)
    }
}

fn validate_unique_paths(documents: &[Document]) -> Result<(), DuplicateDocumentPath> {
    let mut paths = Vec::new();
    for document in documents {
        let Some(path) = document.path.as_deref() else {
            continue;
        };
        let canonical = canonical_key(path)?;
        if paths.contains(&canonical) {
            return Err(DuplicateDocumentPath(canonical));
        }
        paths.push(canonical);
    }
    Ok(())
}

impl Default for Tabs {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Tabs {
    fn drop(&mut self) {
        // Normal shutdown drains before HWND destruction; emergency teardown must still retire
        // metadata so retained accessibility providers cannot target a recycled window.
        self.clear_for_shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::Tabs;
    use crate::document::{Document, DocumentId};
    use std::fs;

    fn document(id: u64) -> Document {
        Document::test_fixture(DocumentId(id), false)
    }

    #[test]
    fn native_document_tab_is_selected() {
        let tabs = Tabs::with_document(Document::test_fixture(DocumentId(1), false));
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs.active_index(), 0);
        assert_eq!(tabs.titles().collect::<Vec<_>>(), ["Untitled"]);
    }

    #[test]
    fn set_active_language_updates_only_the_active_document() {
        // Break caught: explicit language selection or detection-on-open applying to the wrong
        // tab, or leaving Document::language stale after a real lexer switch.
        use crate::document::Language;
        let mut tabs = Tabs::from_documents([document(1), document(2)]).unwrap();
        tabs.activate(DocumentId(2)).unwrap();

        assert!(tabs.set_active_language(Language::Json));
        assert!(!tabs.set_active_language(Language::Json));
        assert_eq!(
            tabs.document(DocumentId(1)).unwrap().language,
            Language::PlainText
        );
        assert_eq!(
            tabs.document(DocumentId(2)).unwrap().language,
            Language::Json
        );
    }

    #[test]
    fn shared_selection_updates_the_tab_model_and_rejects_out_of_range_indices() {
        // Break caught: accessibility can keep a provider-private selection that title painting
        // cannot observe, or accept a button/out-of-range index as a tab.
        let tabs = Tabs::from_documents([
            Document::test_fixture(DocumentId(1), false),
            Document::test_fixture(DocumentId(2), false),
        ])
        .unwrap();
        let selection = tabs.selection();

        assert!(selection.select(1, tabs.len()));
        assert_eq!(tabs.active_index(), 1);
        assert!(!selection.select(2, tabs.len()));
        assert_eq!(tabs.active_index(), 1);
    }

    #[test]
    fn renaming_the_active_document_updates_its_path_and_the_tab_view() {
        // Break caught: Save As writing the path field directly instead of going through a
        // validated method can desync the tab view from the document model.
        let root = std::env::temp_dir().join(format!(
            "fastpad-task11-rename-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let target = root.join("saved-as.txt");
        fs::write(&target, b"saved").unwrap();
        let mut tabs = Tabs::with_document(document(1));
        let view = tabs.view();
        let before = view.snapshot().revision;

        tabs.set_active_path(target.clone()).unwrap();

        assert_eq!(tabs.active().path.as_deref(), Some(target.as_path()));
        assert!(view.snapshot().revision > before);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn renaming_to_a_path_that_does_not_yet_exist_succeeds() {
        // Break caught: reusing canonical_key's existence requirement verbatim can reject every
        // brand-new Save As destination, since a not-yet-written file cannot be canonicalized.
        let root = std::env::temp_dir().join(format!(
            "fastpad-task11-rename-new-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let target = root.join("not-written-yet.txt");
        let mut tabs = Tabs::with_document(document(1));

        assert!(tabs.set_active_path(target.clone()).is_ok());
        assert_eq!(tabs.active().path.as_deref(), Some(target.as_path()));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn renaming_the_active_document_onto_another_open_tabs_canonical_path_is_rejected() {
        // Break caught: Save As can create two tabs that own the same canonical path, breaking
        // Task 9's one-native-document-per-path invariant.
        let root = std::env::temp_dir().join(format!(
            "fastpad-task11-rename-collision-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let other_path = root.join("other.txt");
        fs::write(&other_path, b"other").unwrap();
        let mut other = document(2);
        other.path = Some(other_path.clone());
        let mut tabs = Tabs::from_documents([document(1), other]).unwrap();
        tabs.activate(DocumentId(1)).unwrap();

        let alternate = root.join(".").join("other.txt");
        assert!(tabs.set_active_path(alternate).is_err());
        assert_eq!(tabs.active().path, None);
        fs::remove_dir_all(root).unwrap();
    }
}
