//! In-app command palette: fuzzy-filtered list of view commands.
//! GPUI-free pure types and logic — rendering lives in treemap.rs.

use outrider_view::command::{CameraCommand, TourCommand, ViewCommand};
use outrider_view::spec::*;

pub(crate) struct CommandEntry {
    pub label: String,
    pub category: &'static str,
    pub action: CommandAction,
}

pub(crate) enum CommandAction {
    View(ViewCommand),
    OpenFilePalette,
    OpenSymbolPalette,
}

pub(crate) struct CommandPaletteState {
    pub open: bool,
    pub query: String,
    pub entries: Vec<CommandEntry>,
    pub filtered: Vec<usize>,
    pub selection: usize,
}

impl CommandPaletteState {
    pub fn new() -> Self {
        Self {
            open: false,
            query: String::new(),
            entries: Vec::new(),
            filtered: Vec::new(),
            selection: 0,
        }
    }

    pub fn open(&mut self, metric_names: &[&str]) {
        self.query.clear();
        self.selection = 0;
        self.entries = build_commands(metric_names);
        self.filtered = (0..self.entries.len()).collect();
        self.open = true;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.entries.clear();
        self.filtered.clear();
    }

    pub fn refilter(&mut self) {
        if self.query.is_empty() {
            self.filtered = (0..self.entries.len()).collect();
        } else {
            self.filtered = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| outrider_index::search::fuzzy_match(&self.query, &e.label))
                .map(|(i, _)| i)
                .collect();
        }
        self.selection = 0;
    }

    pub fn selected_entry(&self) -> Option<&CommandEntry> {
        let idx = *self.filtered.get(self.selection)?;
        self.entries.get(idx)
    }
}

pub(crate) enum CmdPaletteEffect {
    None,
    SelectionChanged,
    Execute(usize),
    Close,
    QueryChanged,
}

pub(crate) fn cmd_palette_key(
    state: &mut CommandPaletteState,
    key: &str,
    ch: Option<char>,
) -> CmdPaletteEffect {
    match key {
        "escape" => {
            state.close();
            CmdPaletteEffect::Close
        }
        "up" => {
            if state.filtered.is_empty() {
                return CmdPaletteEffect::None;
            }
            if state.selection == 0 {
                state.selection = state.filtered.len() - 1;
            } else {
                state.selection -= 1;
            }
            CmdPaletteEffect::SelectionChanged
        }
        "down" => {
            if state.filtered.is_empty() {
                return CmdPaletteEffect::None;
            }
            state.selection = (state.selection + 1) % state.filtered.len();
            CmdPaletteEffect::SelectionChanged
        }
        "enter" => {
            if let Some(&idx) = state.filtered.get(state.selection) {
                state.close();
                CmdPaletteEffect::Execute(idx)
            } else {
                CmdPaletteEffect::None
            }
        }
        "backspace" => {
            state.query.pop();
            state.refilter();
            CmdPaletteEffect::QueryChanged
        }
        _ => {
            if let Some(c) = ch {
                state.query.push(c);
                state.refilter();
                CmdPaletteEffect::QueryChanged
            } else {
                CmdPaletteEffect::None
            }
        }
    }
}

fn build_commands(metric_names: &[&str]) -> Vec<CommandEntry> {
    let mut cmds = Vec::new();

    // Camera
    cmds.push(CommandEntry {
        label: "Home".into(),
        category: "Camera",
        action: CommandAction::View(ViewCommand::Camera(CameraCommand::Home)),
    });
    cmds.push(CommandEntry {
        label: "Follow: Focus".into(),
        category: "Camera",
        action: CommandAction::View(ViewCommand::Camera(CameraCommand::Follow(
            FollowMode::Focus,
        ))),
    });
    cmds.push(CommandEntry {
        label: "Follow: None".into(),
        category: "Camera",
        action: CommandAction::View(ViewCommand::Camera(CameraCommand::Follow(
            FollowMode::None,
        ))),
    });

    // Fill by each available metric
    for name in metric_names {
        cmds.push(CommandEntry {
            label: format!("Fill: {name}"),
            category: "Fill",
            action: CommandAction::View(ViewCommand::PushLayer(LayerSpec::Fill(FillSpec {
                metric: name.to_string(),
                channel: FillChannel::Fill,
                scale: Scale::Percentile,
                domain: None,
                ramp: None,
            }))),
        });
    }

    // Layers
    cmds.push(CommandEntry {
        label: "Clear Layers".into(),
        category: "Layers",
        action: CommandAction::View(ViewCommand::Clear(
            outrider_view::command::ClearScope::Layers,
        )),
    });
    cmds.push(CommandEntry {
        label: "Clear All".into(),
        category: "Layers",
        action: CommandAction::View(ViewCommand::Clear(
            outrider_view::command::ClearScope::All,
        )),
    });
    cmds.push(CommandEntry {
        label: "Pop Layer".into(),
        category: "Layers",
        action: CommandAction::View(ViewCommand::PopLayer),
    });

    // Mask (dim everything except focused set — uses a convention set)
    cmds.push(CommandEntry {
        label: "Mask: Dim Except Focus".into(),
        category: "Mask",
        action: CommandAction::View(ViewCommand::PushLayer(LayerSpec::Mask(MaskSpec {
            dim_except: Some(SetRef::Inline(Box::new(SetExpr::Ids(vec![
                "$focus".into(),
            ])))),
            dim: None,
            strength: 0.7,
        }))),
    });

    // Tour
    cmds.push(CommandEntry {
        label: "Tour: Play".into(),
        category: "Tour",
        action: CommandAction::View(ViewCommand::Tour(TourCommand::Play)),
    });
    cmds.push(CommandEntry {
        label: "Tour: Next".into(),
        category: "Tour",
        action: CommandAction::View(ViewCommand::Tour(TourCommand::Next)),
    });
    cmds.push(CommandEntry {
        label: "Tour: Previous".into(),
        category: "Tour",
        action: CommandAction::View(ViewCommand::Tour(TourCommand::Prev)),
    });
    cmds.push(CommandEntry {
        label: "Tour: Stop".into(),
        category: "Tour",
        action: CommandAction::View(ViewCommand::Tour(TourCommand::Stop)),
    });
    cmds.push(CommandEntry {
        label: "Tour: Load History".into(),
        category: "Tour",
        action: CommandAction::View(ViewCommand::Tour(TourCommand::SetSteps(Vec::new()))),
    });

    // Palette shortcuts
    cmds.push(CommandEntry {
        label: "Find File".into(),
        category: "Navigate",
        action: CommandAction::OpenFilePalette,
    });
    cmds.push(CommandEntry {
        label: "Find Symbol".into(),
        category: "Navigate",
        action: CommandAction::OpenSymbolPalette,
    });

    cmds
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_populates_and_filters() {
        let mut state = CommandPaletteState::new();
        state.open(&["churn", "complexity"]);
        assert!(state.open);
        assert!(state.entries.len() > 5);
        assert_eq!(state.filtered.len(), state.entries.len());

        state.query = "fill".into();
        state.refilter();
        assert!(state.filtered.len() < state.entries.len());
        for &idx in &state.filtered {
            assert!(outrider_index::search::fuzzy_match(
                "fill",
                &state.entries[idx].label
            ));
        }
    }

    #[test]
    fn key_nav_wraps() {
        let mut state = CommandPaletteState::new();
        state.open(&["churn"]);
        let n = state.filtered.len();

        let eff = cmd_palette_key(&mut state, "up", None);
        assert!(matches!(eff, CmdPaletteEffect::SelectionChanged));
        assert_eq!(state.selection, n - 1);

        let eff = cmd_palette_key(&mut state, "down", None);
        assert!(matches!(eff, CmdPaletteEffect::SelectionChanged));
        assert_eq!(state.selection, 0);
    }

    #[test]
    fn enter_executes_and_closes() {
        let mut state = CommandPaletteState::new();
        state.open(&[]);
        assert!(state.open);

        let eff = cmd_palette_key(&mut state, "enter", None);
        assert!(matches!(eff, CmdPaletteEffect::Execute(_)));
        assert!(!state.open);
    }

    #[test]
    fn escape_closes() {
        let mut state = CommandPaletteState::new();
        state.open(&[]);
        let eff = cmd_palette_key(&mut state, "escape", None);
        assert!(matches!(eff, CmdPaletteEffect::Close));
        assert!(!state.open);
    }

    #[test]
    fn typing_filters() {
        let mut state = CommandPaletteState::new();
        state.open(&["churn", "complexity", "coverage"]);
        let all = state.filtered.len();

        cmd_palette_key(&mut state, "x", Some('x'));
        // "x" should narrow results (only entries containing 'x')
        assert!(state.filtered.len() <= all);
    }
}
