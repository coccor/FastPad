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
    titles: Vec<String>,
    selection: TabSelection,
}

impl Tabs {
    pub fn new() -> Self {
        Self {
            titles: vec!["Untitled".to_owned()],
            selection: TabSelection::new(0),
        }
    }

    pub fn len(&self) -> usize {
        self.titles.len()
    }

    pub fn active_index(&self) -> usize {
        self.selection.active_index()
    }

    pub(crate) fn selection(&self) -> TabSelection {
        self.selection.clone()
    }

    #[cfg(test)]
    pub fn title(&self, index: usize) -> &str {
        &self.titles[index]
    }

    pub fn titles(&self) -> impl Iterator<Item = &str> {
        self.titles.iter().map(String::as_str)
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

    #[test]
    fn startup_tab_is_plain_text_and_selected() {
        let tabs = Tabs::new();
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs.active_index(), 0);
        assert_eq!(tabs.title(0), "Untitled");
    }

    #[test]
    fn shared_selection_updates_the_tab_model_and_rejects_out_of_range_indices() {
        // Break caught: accessibility can keep a provider-private selection that title painting
        // cannot observe, or accept a button/out-of-range index as a tab.
        let mut tabs = Tabs::new();
        tabs.titles.push("Two".to_owned());
        let selection = tabs.selection();

        assert!(selection.select(1, tabs.len()));
        assert_eq!(tabs.active_index(), 1);
        assert!(!selection.select(2, tabs.len()));
        assert_eq!(tabs.active_index(), 1);
    }
}
