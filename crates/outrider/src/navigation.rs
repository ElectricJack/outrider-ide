use outrider_index::SymbolId;

/// Bounded browser-style history for explicit focus navigation.
pub(crate) struct NavigationHistory {
    entries: Vec<SymbolId>,
    cursor: usize,
    capacity: usize,
}

impl NavigationHistory {
    pub(crate) fn new(initial: SymbolId, capacity: usize) -> Self {
        assert!(capacity > 0, "navigation history capacity must be non-zero");
        Self {
            entries: vec![initial],
            cursor: 0,
            capacity,
        }
    }

    pub(crate) fn push(&mut self, id: SymbolId) {
        self.entries.truncate(self.cursor + 1);
        self.entries.push(id);
        if self.entries.len() > self.capacity {
            let excess = self.entries.len() - self.capacity;
            self.entries.drain(..excess);
        }
        self.cursor = self.entries.len() - 1;
    }

    pub(crate) fn back(&mut self) -> Option<&SymbolId> {
        self.cursor = self.cursor.checked_sub(1)?;
        self.entries.get(self.cursor)
    }

    pub(crate) fn forward(&mut self) -> Option<&SymbolId> {
        let next = self.cursor + 1;
        if next >= self.entries.len() {
            return None;
        }
        self.cursor = next;
        self.entries.get(self.cursor)
    }

    #[cfg(test)]
    pub(crate) fn current(&self) -> &SymbolId {
        &self.entries[self.cursor]
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::NavigationHistory;
    use outrider_index::{SymbolId, SymbolKind};

    fn id(path: &str) -> SymbolId {
        SymbolId {
            kind: SymbolKind::File,
            qualified_path: path.into(),
            ordinal: 0,
        }
    }

    #[test]
    fn history_moves_back_and_forward() {
        let root = id("root");
        let child = id("child");
        let mut history = NavigationHistory::new(root.clone(), 64);
        history.push(child.clone());
        assert_eq!(history.back(), Some(&root));
        assert_eq!(history.forward(), Some(&child));
    }

    #[test]
    fn push_after_back_discards_forward_entries() {
        let root = id("root");
        let child = id("child");
        let replacement = id("replacement");
        let mut history = NavigationHistory::new(root.clone(), 64);
        history.push(child);
        assert_eq!(history.back(), Some(&root));
        history.push(replacement.clone());
        assert_eq!(history.current(), &replacement);
        assert!(history.forward().is_none());
    }

    #[test]
    fn history_respects_its_capacity() {
        let mut history = NavigationHistory::new(id("0"), 3);
        for path in ["1", "2", "3"] {
            history.push(id(path));
        }
        assert_eq!(history.len(), 3);
        assert_eq!(history.current(), &id("3"));
        assert_eq!(history.back(), Some(&id("2")));
        assert_eq!(history.back(), Some(&id("1")));
        assert!(history.back().is_none());
    }
}
