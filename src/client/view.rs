//! Client presentation state: pure data, mutated only by the event loop.

use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;

use crate::protocol::PaneId;
use crate::protocol::PaneInfo;

pub const SIDEBAR_WIDTH: u16 = 28;

pub enum Mode {
    /// Keys go to the focused pane.
    Normal,

    /// The prefix key was pressed; the next key is a client command.
    Prefix,

    /// Typing into the status-bar prompt.
    Prompt { kind: PromptKind, input: String },
}

#[derive(Clone, Copy)]
pub enum PromptKind {
    NewPane,
    Rename,
}

impl PromptKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::NewPane => "command",
            Self::Rename => "rename",
        }
    }
}

pub struct View {
    /// Panes in sidebar order, as last fetched from the server.
    pub panes: Vec<PaneInfo>,

    /// Pane shown and receiving keys.
    pub focused: Option<PaneId>,

    /// Local mirror of the focused pane's screen.
    pub screen: vt100::Parser,

    /// How the next key is interpreted.
    pub mode: Mode,

    /// Error shown in the status bar until the next key press.
    pub message: Option<String>,
}

impl View {
    pub fn new() -> Self {
        Self {
            panes: Vec::new(),
            focused: None,
            screen: vt100::Parser::new(1, 1, 0),
            mode: Mode::Normal,
            message: None,
        }
    }

    pub fn focused_pane(&self) -> Option<&PaneInfo> {
        self.panes.iter().find(|pane| Some(pane.id) == self.focused)
    }

    /// Replaces the pane list, keeping focus on the same pane when it still
    /// exists. Returns true if focus moved.
    pub fn set_panes(&mut self, panes: Vec<PaneInfo>) -> bool {
        let previous = self.focused;
        let still_there = previous.is_some_and(|id| panes.iter().any(|p| p.id == id));
        if !still_there {
            self.focused = panes.first().map(|p| p.id);
        }
        self.panes = panes;
        self.focused != previous
    }

    /// Moves focus by `delta` positions, wrapping around.
    pub fn cycle_focus(&mut self, delta: isize) {
        if self.panes.is_empty() {
            return;
        }
        let len = self.panes.len() as isize;
        let current = self
            .panes
            .iter()
            .position(|p| Some(p.id) == self.focused)
            .unwrap_or(0) as isize;
        let next = (current + delta).rem_euclid(len) as usize;
        self.focused = Some(self.panes[next].id);
    }

    pub fn focus_index(&mut self, index: usize) {
        if let Some(pane) = self.panes.get(index) {
            self.focused = Some(pane.id);
        }
    }
}

pub struct Areas {
    pub sidebar: Rect,
    pub pane: Rect,
    pub status: Rect,
}

pub fn areas(screen: Rect) -> Areas {
    let [main, status] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(screen);
    let [sidebar, pane] =
        Layout::horizontal([Constraint::Length(SIDEBAR_WIDTH), Constraint::Min(1)]).areas(main);
    Areas {
        sidebar,
        pane,
        status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::AgentState;

    fn pane(id: PaneId) -> PaneInfo {
        PaneInfo {
            id,
            name: format!("p{id}"),
            command: "sh".into(),
            cwd: "/".into(),
            rows: 1,
            cols: 1,
            agent: None,
            status: AgentState::Unknown,
            rule: None,
        }
    }

    #[test]
    fn focus_survives_list_refresh_and_falls_back() {
        let mut view = View::new();
        assert!(view.set_panes(vec![pane(1), pane(2)]));
        assert_eq!(view.focused, Some(1));

        view.focused = Some(2);
        assert!(!view.set_panes(vec![pane(1), pane(2), pane(3)]));
        assert_eq!(view.focused, Some(2));

        assert!(view.set_panes(vec![pane(3)]));
        assert_eq!(view.focused, Some(3));
        assert!(view.set_panes(vec![]));
        assert_eq!(view.focused, None);
    }

    #[test]
    fn focus_cycles_with_wraparound() {
        let mut view = View::new();
        view.set_panes(vec![pane(1), pane(2), pane(3)]);
        view.cycle_focus(-1);
        assert_eq!(view.focused, Some(3));
        view.cycle_focus(2);
        assert_eq!(view.focused, Some(2));
    }
}
