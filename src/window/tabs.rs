use crate::document::{CloseCancelled, CloseDecision, Document, DocumentId};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

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

#[derive(Debug)]
pub struct Tabs {
    documents: Vec<Document>,
    selection: TabSelection,
}

impl Tabs {
    pub fn new() -> Self {
        Self {
            documents: Vec::new(),
            selection: TabSelection::new(0),
        }
    }

    pub fn with_document(document: Document) -> Self {
        Self::from_documents([document])
    }

    pub fn from_documents(documents: impl IntoIterator<Item = Document>) -> Self {
        Self {
            documents: documents.into_iter().collect(),
            selection: TabSelection::new(0),
        }
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

    pub fn active(&self) -> &Document {
        &self.documents[self.active_index()]
    }

    pub fn document(&self, id: DocumentId) -> Option<&Document> {
        self.documents.iter().find(|document| document.id == id)
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
            return Ok(closed);
        }
        let closed = self.documents.remove(index);
        self.selection
            .select(index.min(self.documents.len() - 1), self.documents.len());
        Ok(closed)
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
        true
    }

    pub fn clear_for_shutdown(&mut self) {
        self.documents.clear();
        self.selection.active.store(0, Ordering::Release);
    }

    pub(crate) fn active_handle(&self) -> &crate::editor::EditorDocument {
        &self.active().handle
    }

    pub(crate) fn dirty_ids(&self) -> impl Iterator<Item = DocumentId> + '_ {
        self.documents
            .iter()
            .filter(|document| document.dirty)
            .map(|document| document.id)
    }
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

impl Default for Tabs {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::Tabs;
    use crate::document::{Document, DocumentId};

    #[test]
    fn native_document_tab_is_selected() {
        let tabs = Tabs::with_document(Document::test_fixture(DocumentId(1), false));
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs.active_index(), 0);
        assert_eq!(tabs.titles().collect::<Vec<_>>(), ["Untitled"]);
    }

    #[test]
    fn shared_selection_updates_the_tab_model_and_rejects_out_of_range_indices() {
        // Break caught: accessibility can keep a provider-private selection that title painting
        // cannot observe, or accept a button/out-of-range index as a tab.
        let tabs = Tabs::from_documents([
            Document::test_fixture(DocumentId(1), false),
            Document::test_fixture(DocumentId(2), false),
        ]);
        let selection = tabs.selection();

        assert!(selection.select(1, tabs.len()));
        assert_eq!(tabs.active_index(), 1);
        assert!(!selection.select(2, tabs.len()));
        assert_eq!(tabs.active_index(), 1);
    }
}
