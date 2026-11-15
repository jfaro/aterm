//! Client presentation state: pure data, mutated only by the event loop.

use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;

use crate::protocol::FolderId;
use crate::protocol::FolderInfo;
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

    /// Showing the key reference; any key closes it without reaching the pane.
    Help,
}

#[derive(Clone, Copy)]
pub enum PromptKind {
    NewPane,
    Rename,
    NewFolder,
    RenameFolder(FolderId),
}

impl PromptKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::NewPane => "command",
            Self::Rename => "rename",
            Self::NewFolder => "folder name",
            Self::RenameFolder(_) => "rename folder",
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

    /// Folders in sidebar order, as last fetched from the server.
    pub folders: Vec<FolderInfo>,

    /// Pane shown and receiving keys.
    pub focused: Option<PaneId>,

    /// Folder whose sidebar row is selected; while `None`, the focused pane's row is.
    pub selected_folder: Option<FolderId>,

    /// Local mirror of the focused pane's screen.
    pub screen: vt100::Parser,

    /// Pane that `screen` mirrors, which may lag `focused` until the next read.
    pub screen_pane: Option<PaneId>,

    /// Lines `screen` is scrolled back into the pane's history; 0 when live.
    pub scrollback: usize,

    /// Whether the mirrored pane is on the alternate screen.
    pub alternate_screen: bool,

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
            folders: Vec::new(),
            focused: None,
            selected_folder: None,
            screen: vt100::Parser::new(1, 1, 0),
            screen_pane: None,
            scrollback: 0,
            alternate_screen: false,
            mode: Mode::Normal,
            message: None,
            sidebar,
        }
    }

    pub fn focused_pane(&self) -> Option<&PaneInfo> {
        self.panes.iter().find(|pane| Some(pane.id) == self.focused)
    }

    pub fn selected_folder(&self) -> Option<&FolderInfo> {
        self.folders
            .iter()
            .find(|folder| Some(folder.id) == self.selected_folder)
    }

    /// Replaces the pane and folder lists, keeping focus on the same pane and
    /// the selection on the same folder when they still exist. Returns true if
    /// focus moved.
    pub fn set_panes(&mut self, panes: Vec<PaneInfo>, folders: Vec<FolderInfo>) -> bool {
        let previous = self.focused;
        let still_there = previous.is_some_and(|id| panes.iter().any(|p| p.id == id));
        if !still_there {
            self.focused = panes.first().map(|p| p.id);
        }
        self.panes = panes;
        self.folders = folders;
        if self.selected_folder().is_none() {
            self.selected_folder = None;
        }
        self.focused != previous
    }

    /// The group `pane` is shown in: its folder, or the top level if that
    /// folder isn't in the folder list (yet).
    pub fn group_of(&self, pane: &PaneInfo) -> Option<FolderId> {
        pane.folder
            .filter(|&id| self.folders.iter().any(|folder| folder.id == id))
    }

    /// Ids of the panes shown in `group`, in order.
    fn group(&self, group: Option<FolderId>) -> Vec<PaneId> {
        self.panes
            .iter()
            .filter(|pane| self.group_of(pane) == group)
            .map(|pane| pane.id)
            .collect()
    }

    /// Sidebar rows in order: top-level panes, then each folder followed by its panes.
    pub fn rows(&self) -> Vec<Row> {
        let mut rows: Vec<Row> = self.group(None).into_iter().map(Row::Pane).collect();
        for folder in &self.folders {
            rows.push(Row::Folder(folder.id));
            rows.extend(self.group(Some(folder.id)).into_iter().map(Row::Pane));
        }
        rows
    }

    /// The highlighted sidebar row: the selected folder, else the focused pane.
    pub fn selected_row(&self) -> Option<Row> {
        self.selected_folder
            .map(Row::Folder)
            .or(self.focused.map(Row::Pane))
    }

    /// Moves the selection by `delta` rows, wrapping around. Landing on a pane
    /// focuses it; landing on a folder keeps the focused pane on screen.
    pub fn cycle_focus(&mut self, delta: isize) {
        let rows = self.rows();
        if rows.is_empty() {
            return;
        }
        let current = self
            .selected_row()
            .and_then(|row| rows.iter().position(|&other| other == row))
            .unwrap_or(0) as isize;
        let next = (current + delta).rem_euclid(rows.len() as isize) as usize;
        self.select(rows[next]);
    }

    /// Focuses the `index`th pane in sidebar order, skipping folder rows.
    pub fn focus_index(&mut self, index: usize) {
        let pane = self
            .rows()
            .into_iter()
            .filter(|row| matches!(row, Row::Pane(_)))
            .nth(index);
        if let Some(row) = pane {
            self.select(row);
        }
    }

    fn select(&mut self, row: Row) {
        match row {
            Row::Folder(id) => self.selected_folder = Some(id),
            Row::Pane(id) => {
                self.selected_folder = None;
                self.focused = Some(id);
            }
        }
    }

    /// Folder new panes join: the selected one, else the focused pane's.
    pub fn current_folder(&self) -> Option<FolderId> {
        self.selected_folder
            .or_else(|| self.focused_pane().and_then(|pane| self.group_of(pane)))
    }

    /// Where shifting the focused pane `delta` (±1) places puts it, as a group
    /// and an index within that group. Past either end of its group, the pane
    /// moves into the neighboring group: to the start of the next one going
    /// down, the end of the previous one going up. `None` at the very ends.
    pub fn move_target(&self, delta: isize) -> Option<(Option<FolderId>, usize)> {
        let pane = self.focused_pane()?;
        let group = self.group_of(pane);
        let members = self.group(group);
        let index = members.iter().position(|&id| id == pane.id)?;
        if let Some(target) = index.checked_add_signed(delta)
            && target < members.len()
        {
            return Some((group, target));
        }

        let groups: Vec<Option<FolderId>> = std::iter::once(None)
            .chain(self.folders.iter().map(|folder| Some(folder.id)))
            .collect();
        let position = groups.iter().position(|&other| other == group)?;
        let next = *groups.get(position.checked_add_signed(delta.signum())?)?;
        let index = if delta > 0 { 0 } else { self.group(next).len() };
        Some((next, index))
    }
}

/// One line of the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Folder(FolderId),
    Pane(PaneId),
}

pub struct Areas {
    pub sidebar: Rect,
    pub pane: Rect,
    pub status: Rect,
}

/// Height of the status bar, including its top border.
const STATUS_HEIGHT: u16 = 2;

pub fn areas(screen: Rect, sidebar: &Sidebar) -> Areas {
    let [main, status] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(STATUS_HEIGHT)]).areas(screen);
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
            folder: None,
        }
    }

    #[test]
    fn focus_survives_list_refresh_and_falls_back() {
        let mut view = View::new(Sidebar::default());
        assert!(view.set_panes(vec![pane(1), pane(2)], vec![]));
        assert_eq!(view.focused, Some(1));

        view.focused = Some(2);
        assert!(!view.set_panes(vec![pane(1), pane(2), pane(3)], vec![]));
        assert_eq!(view.focused, Some(2));

        assert!(view.set_panes(vec![pane(3)], vec![]));
        assert_eq!(view.focused, Some(3));
        assert!(view.set_panes(vec![], vec![]));
        assert_eq!(view.focused, None);
    }

    #[test]
    fn focus_cycles_with_wraparound() {
        let mut view = View::new(Sidebar::default());
        view.set_panes(vec![pane(1), pane(2), pane(3)], vec![]);
        view.cycle_focus(-1);
        assert_eq!(view.focused, Some(3));
        view.cycle_focus(2);
        assert_eq!(view.focused, Some(2));
    }

    fn folder(id: FolderId) -> FolderInfo {
        FolderInfo {
            id,
            name: format!("f{id}"),
        }
    }

    fn in_folder(id: PaneId, folder: FolderId) -> PaneInfo {
        PaneInfo {
            folder: Some(folder),
            ..pane(id)
        }
    }

    /// Top-level pane 1, folder 10 holding panes 2 and 3, and empty folder 20.
    fn foldered_view() -> View {
        let mut view = View::new(Sidebar::default());
        view.set_panes(
            vec![pane(1), in_folder(2, 10), in_folder(3, 10)],
            vec![folder(10), folder(20)],
        );
        view
    }

    #[test]
    fn rows_put_folders_after_top_level_panes() {
        let mut view = foldered_view();
        assert_eq!(
            view.rows(),
            [
                Row::Pane(1),
                Row::Folder(10),
                Row::Pane(2),
                Row::Pane(3),
                Row::Folder(20)
            ]
        );

        // A pane naming a folder not listed yet shows at the top level.
        view.set_panes(vec![pane(1), in_folder(2, 99)], vec![]);
        assert_eq!(view.rows(), [Row::Pane(1), Row::Pane(2)]);
    }

    #[test]
    fn selection_stops_on_folders_and_keeps_the_pane_shown() {
        let mut view = foldered_view();
        view.cycle_focus(1);
        assert_eq!(view.selected_row(), Some(Row::Folder(10)));
        assert_eq!(view.focused, Some(1));
        assert_eq!(view.current_folder(), Some(10));

        view.cycle_focus(1);
        assert_eq!(view.selected_row(), Some(Row::Pane(2)));
        assert_eq!(view.current_folder(), Some(10));

        view.cycle_focus(-3);
        assert_eq!(view.selected_row(), Some(Row::Folder(20)));
        assert_eq!(view.focused, Some(2));

        // Pane numbers skip folder rows.
        view.focus_index(2);
        assert_eq!(view.selected_row(), Some(Row::Pane(3)));

        // A deleted folder drops its selection.
        view.cycle_focus(1);
        assert_eq!(view.selected_row(), Some(Row::Folder(20)));
        view.set_panes(view.panes.clone(), vec![folder(10)]);
        assert_eq!(view.selected_row(), Some(Row::Pane(3)));
    }

    #[test]
    fn moves_cross_into_neighboring_folders() {
        let mut view = foldered_view();
        view.focused = Some(1);
        assert_eq!(view.move_target(-1), None);
        assert_eq!(view.move_target(1), Some((Some(10), 0)));

        view.focused = Some(2);
        assert_eq!(view.move_target(-1), Some((None, 1)));
        assert_eq!(view.move_target(1), Some((Some(10), 1)));

        view.focused = Some(3);
        assert_eq!(view.move_target(1), Some((Some(20), 0)));

        view.set_panes(
            vec![pane(1), in_folder(3, 20)],
            vec![folder(10), folder(20)],
        );
        assert_eq!(view.move_target(-1), Some((Some(10), 0)));
        assert_eq!(view.move_target(1), None);
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
