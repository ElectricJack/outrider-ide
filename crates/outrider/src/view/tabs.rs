//! View tabs: a workspace of switchable view specs. Tab 0 is always the
//! session's base treemap view; every `.outrider/views/*.json` file gets a
//! tab. Switching tabs swaps the active spec while the focused symbol is
//! preserved, so a class selected in one view stays selected in the next.

use std::path::{Path, PathBuf};

use outrider_view::ViewSpec;

/// One switchable view.
#[derive(Debug, Clone)]
pub(crate) struct ViewTab {
    /// Display label: `meta.title` if set, else the file stem.
    pub(crate) label: String,
    /// Source file for file-backed tabs; None for the base treemap tab.
    pub(crate) path: Option<PathBuf>,
    /// The spec to apply when this tab becomes active.
    pub(crate) spec: ViewSpec,
}

#[derive(Debug)]
pub(crate) struct ViewTabs {
    tabs: Vec<ViewTab>,
    active: usize,
}

/// Index of the base treemap tab.
pub(crate) const BASE_TAB: usize = 0;

impl ViewTabs {
    /// Start with just the base tab holding `base_spec`.
    pub(crate) fn new(base_spec: ViewSpec) -> Self {
        ViewTabs {
            tabs: vec![ViewTab {
                label: "Treemap".into(),
                path: None,
                spec: base_spec,
            }],
            active: BASE_TAB,
        }
    }

    pub(crate) fn tabs(&self) -> &[ViewTab] {
        &self.tabs
    }

    pub(crate) fn active_index(&self) -> usize {
        self.active
    }

    pub(crate) fn active(&self) -> &ViewTab {
        &self.tabs[self.active]
    }

    pub(crate) fn len(&self) -> usize {
        self.tabs.len()
    }

    /// Keep the base tab's spec in sync with session changes (e.g. the
    /// churn toggle rewriting the default view).
    pub(crate) fn set_base_spec(&mut self, spec: ViewSpec) {
        self.tabs[BASE_TAB].spec = spec;
    }

    /// Record the spec currently live in the session against the active
    /// tab, so in-app edits (palette commands, layer pushes) survive a
    /// round trip through another tab.
    pub(crate) fn store_active_spec(&mut self, spec: ViewSpec) {
        self.tabs[self.active].spec = spec;
    }

    /// Insert or update the tab backed by `path`. Returns the tab index.
    pub(crate) fn upsert_file(&mut self, path: &Path, spec: ViewSpec) -> usize {
        let label = spec
            .meta
            .title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| {
                path.file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "view".into())
            });
        if let Some(i) = self.index_of_path(path) {
            self.tabs[i].label = label;
            self.tabs[i].spec = spec;
            i
        } else {
            self.tabs.push(ViewTab {
                label,
                path: Some(path.to_path_buf()),
                spec,
            });
            self.tabs.len() - 1
        }
    }

    /// Remove the tab backed by `path`. Returns true if the active tab was
    /// removed (the caller should then re-apply the new active spec).
    pub(crate) fn remove_file(&mut self, path: &Path) -> bool {
        let Some(i) = self.index_of_path(path) else {
            return false;
        };
        self.tabs.remove(i);
        if self.active == i {
            self.active = BASE_TAB;
            true
        } else {
            if self.active > i {
                self.active -= 1;
            }
            false
        }
    }

    pub(crate) fn index_of_path(&self, path: &Path) -> Option<usize> {
        self.tabs.iter().position(|t| t.path.as_deref() == Some(path))
    }

    /// Activate tab `i`; returns false when out of range or already active.
    pub(crate) fn activate(&mut self, i: usize) -> bool {
        if i >= self.tabs.len() || i == self.active {
            return false;
        }
        self.active = i;
        true
    }

    /// Activate the next tab, wrapping around.
    pub(crate) fn cycle(&mut self, forward: bool) -> bool {
        if self.tabs.len() < 2 {
            return false;
        }
        let n = self.tabs.len();
        let next = if forward {
            (self.active + 1) % n
        } else {
            (self.active + n - 1) % n
        };
        self.activate(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(title: Option<&str>) -> ViewSpec {
        let mut s = ViewSpec::default();
        s.meta.title = title.map(String::from);
        s
    }

    #[test]
    fn base_tab_always_first() {
        let t = ViewTabs::new(spec(None));
        assert_eq!(t.len(), 1);
        assert_eq!(t.active_index(), BASE_TAB);
        assert_eq!(t.active().label, "Treemap");
    }

    #[test]
    fn upsert_uses_title_then_stem() {
        let mut t = ViewTabs::new(spec(None));
        let i = t.upsert_file(Path::new("x/inheritance.json"), spec(Some("Class Inheritance")));
        assert_eq!(t.tabs()[i].label, "Class Inheritance");
        let j = t.upsert_file(Path::new("x/calls.json"), spec(None));
        assert_eq!(t.tabs()[j].label, "calls");
        // Updating the same path reuses its tab.
        let k = t.upsert_file(Path::new("x/inheritance.json"), spec(Some("Renamed")));
        assert_eq!(k, i);
        assert_eq!(t.len(), 3);
        assert_eq!(t.tabs()[i].label, "Renamed");
    }

    #[test]
    fn remove_active_falls_back_to_base() {
        let mut t = ViewTabs::new(spec(None));
        let i = t.upsert_file(Path::new("a.json"), spec(None));
        t.activate(i);
        assert!(t.remove_file(Path::new("a.json")));
        assert_eq!(t.active_index(), BASE_TAB);
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn remove_before_active_shifts_index() {
        let mut t = ViewTabs::new(spec(None));
        t.upsert_file(Path::new("a.json"), spec(None));
        let b = t.upsert_file(Path::new("b.json"), spec(None));
        t.activate(b);
        assert!(!t.remove_file(Path::new("a.json")));
        assert_eq!(t.active().path.as_deref(), Some(Path::new("b.json")));
    }

    #[test]
    fn cycle_wraps() {
        let mut t = ViewTabs::new(spec(None));
        t.upsert_file(Path::new("a.json"), spec(None));
        t.upsert_file(Path::new("b.json"), spec(None));
        assert!(t.cycle(true));
        assert_eq!(t.active_index(), 1);
        assert!(t.cycle(true));
        assert_eq!(t.active_index(), 2);
        assert!(t.cycle(true));
        assert_eq!(t.active_index(), 0);
        assert!(t.cycle(false));
        assert_eq!(t.active_index(), 2);
    }

    #[test]
    fn activate_same_is_noop() {
        let mut t = ViewTabs::new(spec(None));
        assert!(!t.activate(0));
        assert!(!t.activate(7));
    }
}
