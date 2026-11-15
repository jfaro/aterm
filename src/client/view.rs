//! Client presentation state: pure data, mutated only by the event loop.

use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;

use crate::protocol::PaneId;
use crate::protocol::PaneInfo;

pub const DEFAULT_SIDEBAR_WIDTH: u16 = 28;
pub const MIN_SIDEBAR_WIDTH: u16 = 16;

/// Columns the pane keeps however wide the sidebar is asked to be.
const MIN_PANE_WIDTH: u16 = 20;

pub enum Mode {
    /// Keys go to the focused pane.
    Normal,

    /// The prefix key was pressed; the next key is a client command.
    Prefix,

    /// Typing into the status-bar prompt.
    Prompt { kind: PromptKind, input: String },

    /// Adjusting the sidebar width; keys keep resizing until one ends the mode.
    Resize,
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

pub struct Sidebar {
    /// Width asked for, kept even while the screen is too narrow to honor it.
    pub width: u16,

    /// Whether the sidebar is drawn at all.
    pub visible: bool,
}

impl Sidebar {
    pub fn new(width: u16) -> Self {
        Self {
            width: width.max(MIN_SIDEBAR_WIDTH),
            visible: true,
        }
    }

    /// Columns the sidebar occupies on a screen `screen_width` columns wide.
    pub fn width_on(&self, screen_width: u16) -> u16 {
        if !self.visible {
            return 0;
        }
        self.width.min(screen_width.saturating_sub(MIN_PANE_WIDTH))
    }

    /// Changes the width by `delta` columns, starting from what a screen `screen_width` columns
    /// wide can show and staying within its bounds, and shows the sidebar.
    pub fn resize(&mut self, delta: i16, screen_width: u16) {
        let max = screen_width
            .saturating_sub(MIN_PANE_WIDTH)
            .max(MIN_SIDEBAR_WIDTH);
        self.width = self
            .width
            .min(max)
            .saturating_add_signed(delta)
            .clamp(MIN_SIDEBAR_WIDTH, max);
        self.visible = true;
    }

    pub fn reset(&mut self) {
        self.width = DEFAULT_SIDEBAR_WIDTH;
    }
}

impl Default for Sidebar {
    fn default() -> Self {
        Self::new(DEFAULT_SIDEBAR_WIDTH)
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

    /// Width and visibility of the pane list.
    pub sidebar: Sidebar,
}

impl View {
    pub fn new(sidebar: Sidebar) -> Self {
        Self {
            panes: Vec::new(),
            focused: None,
            screen: vt100::Parser::new(1, 1, 0),
            mode: Mode::Normal,
            message: None,
            sidebar,
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

    pub fn focused_index(&self) -> Option<usize> {
        self.panes.iter().position(|p| Some(p.id) == self.focused)
    }

    /// Moves focus by `delta` positions, wrapping around.
    pub fn cycle_focus(&mut self, delta: isize) {
        if self.panes.is_empty() {
            return;
        }
        let len = self.panes.len() as isize;
        let current = self.focused_index().unwrap_or(0) as isize;
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

pub fn areas(screen: Rect, sidebar: &Sidebar) -> Areas {
    let [main, status] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(screen);
    let sidebar_width = sidebar.width_on(screen.width);
    let [sidebar, pane] =
        Layout::horizontal([Constraint::Length(sidebar_width), Constraint::Min(1)]).areas(main);
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
            auto_named: true,
            command: "sh".into(),
            cwd: "/".into(),
            rows: 1,
            cols: 1,
            agent: None,
            status: AgentState::Unknown,
            status_since: 0,
            rule: None,
        }
    }

    #[test]
    fn focus_survives_list_refresh_and_falls_back() {
        let mut view = View::new(Sidebar::default());
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
        let mut view = View::new(Sidebar::default());
        view.set_panes(vec![pane(1), pane(2), pane(3)]);
        view.cycle_focus(-1);
        assert_eq!(view.focused, Some(3));
        view.cycle_focus(2);
        assert_eq!(view.focused, Some(2));
    }

    #[test]
    fn sidebar_resizes_within_bounds() {
        let mut sidebar = Sidebar::default();
        sidebar.resize(2, 100);
        assert_eq!(sidebar.width, 30);
        sidebar.resize(-100, 100);
        assert_eq!(sidebar.width, MIN_SIDEBAR_WIDTH);
        sidebar.resize(100, 100);
        assert_eq!(sidebar.width, 100 - MIN_PANE_WIDTH);
        sidebar.reset();
        assert_eq!(sidebar.width, DEFAULT_SIDEBAR_WIDTH);
    }

    #[test]
    fn sidebar_narrows_on_small_screens_but_keeps_its_width() {
        let mut sidebar = Sidebar::new(40);
        assert_eq!(sidebar.width_on(50), 30);
        assert_eq!(sidebar.width_on(200), 40);

        // Resizing starts from what is on screen, not the remembered width.
        sidebar.resize(-2, 50);
        assert_eq!(sidebar.width, 28);
    }

    #[test]
    fn hidden_sidebar_takes_no_space_until_resized() {
        let mut sidebar = Sidebar {
            visible: false,
            ..Sidebar::default()
        };
        let screen = Rect::new(0, 0, 100, 10);
        assert_eq!(areas(screen, &sidebar).pane.width, 100);

        sidebar.resize(2, 100);
        assert!(sidebar.visible);
        assert_eq!(areas(screen, &sidebar).sidebar.width, 30);
    }
}
