#[derive(Debug)]
pub struct Tabs {
    titles: Vec<String>,
    active: usize,
}

impl Tabs {
    pub fn new() -> Self {
        Self {
            titles: vec!["Untitled".to_owned()],
            active: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.titles.len()
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

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
}
