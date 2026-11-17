//! The TUI: one client of the server, showing a pane list and the focused pane.

mod input;
mod keybindings;
mod render;
mod selection;
mod view;

use std::io::Write;
use std::io::stdout;
use std::sync::Arc;
use std::time::Duration;
use std::time::SystemTime;

use anyhow::Result;
use anyhow::bail;
use crossterm::Command;
use crossterm::event::DisableBracketedPaste;
use crossterm::event::EnableBracketedPaste;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use crossterm::event::KeyboardEnhancementFlags;
use crossterm::event::MouseButton;
use crossterm::event::MouseEvent;
use crossterm::event::MouseEventKind;
use crossterm::event::PopKeyboardEnhancementFlags;
use crossterm::event::PushKeyboardEnhancementFlags;
use crossterm::event::{self};
use crossterm::execute;
use ratatui::DefaultTerminal;
use ratatui::layout::Position;
use ratatui::layout::Rect;
use serde_json::Value;
use serde_json::json;
use tokio::sync::mpsc;

use crate::protocol::Event;
use crate::protocol::FolderId;
use crate::protocol::FolderInfo;
use crate::protocol::PaneId;
use crate::protocol::PaneInfo;
use crate::protocol::events;
use crate::rpc::Connection;
use input::PaneInput;
use keybindings::PrefixAction;
use keybindings::ResizeAction;
use selection::Selection;
pub use view::DEFAULT_SIDEBAR_WIDTH;
pub use view::MIN_SIDEBAR_WIDTH;
use view::Mode;
use view::PromptKind;
use view::Sidebar;
use view::SidebarPosition;
use view::Toast;
use view::View;
use view::areas;

const CLOCK_TICK: Duration = Duration::from_secs(1);

/// How long a toast stays in the pane's corner.
const TOAST_DURATION: Duration = Duration::from_millis(1500);

/// Lines scrolled per wheel notch.
const WHEEL_LINES: usize = 3;

pub async fn attach(sidebar_width: u16) -> Result<()> {
    let (conn, server_events) = Connection::connect_or_spawn().await?;
    conn.call("events.subscribe", json!({})).await?;

    let mut terminal = ratatui::init();
    let _ = execute!(stdout(), EnableBracketedPaste, MouseReporting(true));
    // Lets the outer terminal (if it supports it) report modifiers on keys
    // like Enter that otherwise collapse to the same byte regardless of
    // Shift, so e.g. Shift+Enter can be told apart from plain Enter.
    let enhanced_keyboard = crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false);
    if enhanced_keyboard {
        let _ = execute!(
            stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
    }
    let result = App::new(conn, Sidebar::new(sidebar_width))
        .run(&mut terminal, server_events)
        .await;
    if enhanced_keyboard {
        let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
    }
    let _ = execute!(stdout(), MouseReporting(false), DisableBracketedPaste);
    ratatui::restore();
    result
}

/// Turns SGR reporting of mouse buttons, and of motion while one is held, on or
/// off. Unlike crossterm's mouse capture it leaves bare motion unreported, so
/// moving the mouse costs no redraws. Without it, the outer terminal turns the
/// wheel into arrow keys.
struct MouseReporting(bool);

impl Command for MouseReporting {
    fn write_ansi(&self, f: &mut impl std::fmt::Write) -> std::fmt::Result {
        f.write_str(if self.0 {
            "\x1b[?1000h\x1b[?1002h\x1b[?1006h"
        } else {
            "\x1b[?1006l\x1b[?1002l\x1b[?1000l"
        })
    }
}

struct App {
    conn: Arc<Connection>,
    view: View,
    screen_area: Rect,
    pane_area: Rect,
    list_dirty: bool,
    screen_dirty: bool,
    quit: bool,
}

impl App {
    fn new(conn: Arc<Connection>, sidebar: Sidebar) -> Self {
        Self {
            conn,
            view: View::new(sidebar),
            screen_area: Rect::default(),
            pane_area: Rect::default(),
            list_dirty: true,
            screen_dirty: true,
            quit: false,
        }
    }

    async fn run(
        mut self,
        terminal: &mut DefaultTerminal,
        mut server_events: mpsc::UnboundedReceiver<Event>,
    ) -> Result<()> {
        let mut terminal_events = spawn_terminal_reader();
        let size = terminal.size()?;
        self.screen_area = Rect::new(0, 0, size.width, size.height);
        self.relayout();

        while !self.quit {
            if self.list_dirty {
                self.refresh_panes().await?;
            }
            if self.screen_dirty {
                self.refresh_screen().await;
            }
            terminal.draw(|frame| render::render(frame, &self.view, SystemTime::now()))?;

            // Redraws periodically so status icons animate and durations keep
            // counting.
            let animating = self
                .view
                .panes
                .iter()
                .any(|p| render::is_animated(p.status));
            let mut tick = if animating {
                render::ANIMATION_FRAME
            } else {
                CLOCK_TICK
            };
            // Wakes when the toast expires so it disappears on time.
            if let Some(toast) = &self.view.toast {
                match toast.until.duration_since(SystemTime::now()) {
                    Ok(remaining) => tick = tick.min(remaining),
                    Err(_) => {
                        self.view.toast = None;
                        continue;
                    }
                }
            }

            tokio::select! {
                _ = tokio::time::sleep(tick) => {}
                Some(event) = terminal_events.recv() => self.on_terminal_event(event).await,
                event = server_events.recv() => match event {
                    Some(event) => self.on_server_event(&event),
                    None => bail!("lost connection to server"),
                },
            }
            // Coalesce bursts so a flood of output costs one redraw.
            while let Ok(event) = server_events.try_recv() {
                self.on_server_event(&event);
            }
        }
        Ok(())
    }

    fn on_server_event(&mut self, event: &Event) {
        let pane = event.data["pane"].as_u64().map(|id| id as PaneId);
        match event.event.as_str() {
            events::PANE_OUTPUT_CHANGED => {
                if pane.is_some() && pane == self.view.focused {
                    self.screen_dirty = true;
                }
            }
            _ => self.list_dirty = true,
        }
    }

    async fn on_terminal_event(&mut self, event: event::Event) {
        match event {
            event::Event::Key(key) if key.kind != KeyEventKind::Release => self.on_key(key).await,
            event::Event::Paste(text) => match &mut self.view.mode {
                Mode::Prompt { input, .. } => input.push_str(&text),
                _ => self.send_to_focused(PaneInput::Text(text)).await,
            },
            event::Event::Mouse(mouse) => self.on_mouse(mouse).await,
            event::Event::Resize(width, height) => {
                self.screen_area = Rect::new(0, 0, width, height);
                self.relayout();
            }
            _ => {}
        }
    }

    async fn on_key(&mut self, key: KeyEvent) {
        self.view.message = None;
        self.view.selection = None;
        match std::mem::replace(&mut self.view.mode, Mode::Normal) {
            Mode::Normal if input::is_prefix(&key) => self.view.mode = Mode::Prefix,
            Mode::Normal => {
                if let Some(pane_input) = input::translate(&key) {
                    self.send_to_focused(pane_input).await;
                }
            }
            Mode::Prefix => self.on_prefix_command(key).await,
            Mode::Resize => self.on_resize_key(key),
            Mode::Help => {}
            Mode::Prompt { kind, mut input } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => self.submit_prompt(kind, input).await,
                KeyCode::Backspace => {
                    input.pop();
                    self.view.mode = Mode::Prompt { kind, input };
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    input.push(c);
                    self.view.mode = Mode::Prompt { kind, input };
                }
                _ => self.view.mode = Mode::Prompt { kind, input },
            },
        }
    }

    async fn on_mouse(&mut self, mouse: MouseEvent) {
        if !matches!(self.view.mode, Mode::Normal) {
            return;
        }
        let position = Position::new(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::ScrollUp => self.on_wheel(true, position).await,
            MouseEventKind::ScrollDown => self.on_wheel(false, position).await,
            MouseEventKind::Down(MouseButton::Left) => {
                self.view.selection = self
                    .view
                    .focused
                    .and(self.pane_cell(position))
                    .map(Selection::start);
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let head = self.clamped_pane_cell(position);
                if let Some(selection) = &mut self.view.selection
                    && selection.dragging
                {
                    selection.head = head;
                }
            }
            MouseEventKind::Up(MouseButton::Left) => self.finish_selection(),
            _ => {}
        }
    }

    /// The pane-relative cell at screen `position`, if it is over the pane.
    fn pane_cell(&self, position: Position) -> Option<Position> {
        self.pane_area
            .contains(position)
            .then(|| Position::new(position.x - self.pane_area.x, position.y - self.pane_area.y))
    }

    /// The pane-relative cell nearest screen `position`, so drags past an edge stop at it.
    fn clamped_pane_cell(&self, position: Position) -> Position {
        let area = self.pane_area;
        Position::new(
            position
                .x
                .saturating_sub(area.x)
                .min(area.width.saturating_sub(1)),
            position
                .y
                .saturating_sub(area.y)
                .min(area.height.saturating_sub(1)),
        )
    }

    /// Ends a drag, copying what it covered; a click without a drag selects nothing.
    fn finish_selection(&mut self) {
        let Some(selection) = &mut self.view.selection else {
            return;
        };
        if !selection.dragging {
            return;
        }
        selection.dragging = false;
        if selection.is_empty() {
            self.view.selection = None;
            return;
        }

        let text = selection.text(self.view.screen.screen());
        if text.is_empty() {
            return;
        }
        let mut out = stdout();
        let copied = out
            .write_all(selection::osc52(&text).as_bytes())
            .and_then(|()| out.flush());
        match copied {
            Ok(()) => {
                self.view.toast = Some(Toast {
                    text: "copied to clipboard".into(),
                    until: SystemTime::now() + TOAST_DURATION,
                });
            }
            Err(err) => self.view.message = Some(format!("copy failed: {err}")),
        }
    }

    /// Routes the wheel over the pane the way a terminal would: to the program
    /// if it asked for mouse reports, as arrow keys on the alternate screen
    /// (which has no history), and otherwise through the pane's scrollback.
    async fn on_wheel(&mut self, up: bool, position: Position) {
        if !self.pane_area.contains(position) {
            return;
        }
        self.view.selection = None;

        if self.view.scrollback == 0 {
            let (col, row) = (position.x - self.pane_area.x, position.y - self.pane_area.y);
            if let Some(report) = input::encode_wheel(up, col, row, self.view.screen.screen()) {
                self.send_to_focused(PaneInput::Text(report)).await;
                return;
            }
            if self.view.alternate_screen {
                let key = if up { "up" } else { "down" };
                for _ in 0..WHEEL_LINES {
                    self.send_to_focused(PaneInput::Key(key.into())).await;
                }
                return;
            }
        }

        // The server clamps how far back this can go.
        self.view.scrollback = if up {
            self.view.scrollback.saturating_add(WHEEL_LINES)
        } else {
            self.view.scrollback.saturating_sub(WHEEL_LINES)
        };
        self.screen_dirty = true;
    }

    async fn on_prefix_command(&mut self, key: KeyEvent) {
        let Some(binding) = keybindings::PREFIX_BINDINGS
            .iter()
            .find(|b| (b.matches)(&key))
        else {
            return;
        };
        self.execute_prefix_action(&binding.action, &key).await;
    }

    async fn execute_prefix_action(&mut self, action: &PrefixAction, key: &KeyEvent) {
        match action {
            PrefixAction::Literal => self.send_to_focused(PaneInput::Key("ctrl-b".into())).await,
            PrefixAction::NewShell => self.create_pane(None).await,
            PrefixAction::NewCommand => {
                self.view.mode = Mode::Prompt {
                    kind: PromptKind::NewPane,
                    input: String::new(),
                };
            }
            PrefixAction::NewFolder => {
                self.view.mode = Mode::Prompt {
                    kind: PromptKind::NewFolder,
                    input: String::new(),
                };
            }
            PrefixAction::Rename => {
                let prompt = match (self.view.selected_folder(), self.view.focused_pane()) {
                    (Some(folder), _) => Some((PromptKind::RenameFolder(folder.id), &folder.name)),
                    (None, Some(pane)) => Some((PromptKind::Rename, &pane.name)),
                    (None, None) => None,
                };
                if let Some((kind, name)) = prompt {
                    let input = name.clone();
                    self.view.mode = Mode::Prompt { kind, input };
                }
            }
            PrefixAction::MoveDown => self.move_selected(1).await,
            PrefixAction::MoveUp => self.move_selected(-1).await,
            PrefixAction::FocusNext => self.change_focus(|view| view.cycle_focus(1)),
            PrefixAction::FocusPrev => self.change_focus(|view| view.cycle_focus(-1)),
            PrefixAction::FocusIndex => {
                let KeyCode::Char(digit @ '1'..='9') = key.code else {
                    return;
                };
                let index = digit as usize - '1' as usize;
                self.change_focus(|view| view.focus_index(index));
            }
            // Deleting a folder moves its panes to the top level; it never closes them.
            PrefixAction::CloseOrDelete => match (self.view.selected_folder, self.view.focused) {
                (Some(folder), _) => {
                    self.call_reporting_errors("folder.delete", json!({ "folder": folder }))
                        .await
                }
                (None, Some(pane)) => {
                    self.call_reporting_errors("pane.close", json!({ "pane": pane }))
                        .await
                }
                (None, None) => {}
            },
            PrefixAction::ResizeLeft => self.start_resizing(-keybindings::RESIZE_STEP_SML),
            PrefixAction::ResizeRight => self.start_resizing(keybindings::RESIZE_STEP_SML),
            PrefixAction::ToggleSidebar => {
                self.view.sidebar.visible = !self.view.sidebar.visible;
                self.relayout();
            }
            PrefixAction::ToggleSidebarPosition => {
                self.view.sidebar.position = self.view.sidebar.position.toggle();
                self.relayout();
            }
            PrefixAction::Detach => self.quit = true,
            PrefixAction::ShowHelp => self.view.mode = Mode::Help,
        }
    }

    fn start_resizing(&mut self, delta: i16) {
        self.resize_sidebar(delta);
        self.view.mode = Mode::Resize;
    }

    /// Resizes on resize keys and stays in resize mode; any other key leaves it unsent.
    fn on_resize_key(&mut self, key: KeyEvent) {
        let Some(binding) = keybindings::RESIZE_BINDINGS
            .iter()
            .find(|b| (b.matches)(&key))
        else {
            return;
        };
        match binding.action {
            ResizeAction::Adjust(delta) => self.start_resizing(delta),
            ResizeAction::Reset => {
                self.view.sidebar.reset();
                self.relayout();
                self.view.mode = Mode::Resize;
            }
        }
    }

    fn resize_sidebar(&mut self, delta: i16) {
        let extent = match self.view.sidebar.position {
            SidebarPosition::Left => self.screen_area.width,
            SidebarPosition::Bottom => self.screen_area.height,
        };
        self.view.sidebar.resize(delta, extent);
        self.relayout();
    }

    /// Recomputes the pane area after the screen or sidebar changed, resizing every pane to fit.
    fn relayout(&mut self) {
        let pane_area = areas(self.screen_area, &self.view.sidebar).pane;
        if pane_area != self.pane_area {
            self.pane_area = pane_area;
            self.view.selection = None;
            self.list_dirty = true;
            self.screen_dirty = true;
        }
    }

    async fn submit_prompt(&mut self, kind: PromptKind, input: String) {
        let input = input.trim().to_owned();
        match kind {
            PromptKind::NewPane if !input.is_empty() => self.create_pane(Some(input)).await,
            // Folders are always named, so an empty name keeps the prompt open.
            PromptKind::NewFolder | PromptKind::RenameFolder(_) if input.is_empty() => {
                self.view.message = Some("a folder needs a name".into());
                self.view.mode = Mode::Prompt { kind, input };
            }
            PromptKind::NewFolder => {
                self.call_reporting_errors("folder.create", json!({ "name": input }))
                    .await
            }
            PromptKind::RenameFolder(folder) => {
                self.call_reporting_errors(
                    "folder.rename",
                    json!({ "folder": folder, "name": input }),
                )
                .await
            }
            // An empty name hands naming back to the server's auto-labeling.
            PromptKind::Rename => {
                if let Some(pane) = self.view.focused {
                    self.call_reporting_errors(
                        "pane.rename",
                        json!({ "pane": pane, "name": input }),
                    )
                    .await;
                }
            }
            _ => {}
        }
    }

    fn change_focus(&mut self, change: impl FnOnce(&mut View)) {
        let before = self.view.focused;
        change(&mut self.view);
        if self.view.focused != before {
            self.screen_dirty = true;
        }
    }

    /// Moves the selected folder, or else the focused pane, one place up
    /// (`delta` -1) or down (+1).
    async fn move_selected(&mut self, delta: isize) {
        match self.view.selected_folder {
            Some(folder) => self.move_folder(folder, delta).await,
            None => self.move_focused(delta).await,
        }
    }

    /// Shifts the focused pane one place up (`delta` -1) or down (+1), crossing into the
    /// neighboring folder at either end of its own and stopping at either end of the sidebar.
    async fn move_focused(&mut self, delta: isize) {
        let (Some(pane), Some((folder, index))) = (self.view.focused, self.view.move_target(delta))
        else {
            return;
        };

        // The server's `pane.moved` event triggers the list refresh.
        self.call_reporting_errors(
            "pane.move",
            json!({ "pane": pane, "folder": folder, "index": index }),
        )
        .await;
    }

    /// Swaps the selected folder with its neighbor one place up (`delta` -1)
    /// or down (+1); a no-op at either end of the sidebar.
    async fn move_folder(&mut self, folder: FolderId, delta: isize) {
        // The server's `folder.moved` event triggers the list refresh.
        self.call_reporting_errors("folder.move", json!({ "folder": folder, "delta": delta }))
            .await;
    }

    async fn create_pane(&mut self, command: Option<String>) {
        let params = json!({
            "command": command,
            "cwd": std::env::current_dir().ok(),
            "rows": self.pane_area.height,
            "cols": self.pane_area.width,
            "folder": self.view.current_folder(),
        });
        match self.conn.call_as::<PaneInfo>("pane.create", params).await {
            Ok(info) => {
                self.view.focused = Some(info.id);
                self.view.selected_folder = None;
                self.list_dirty = true;
                self.screen_dirty = true;
            }
            Err(err) => self.view.message = Some(format!("{err:#}")),
        }
    }

    async fn send_to_focused(&mut self, pane_input: PaneInput) {
        let Some(pane) = self.view.focused else {
            return;
        };
        // Input goes to the pane on screen, so the sidebar follows it there.
        self.view.selected_folder = None;
        // Typing returns a scrolled-back view to the live screen.
        if self.view.scrollback > 0 {
            self.view.scrollback = 0;
            self.screen_dirty = true;
        }
        let (method, params) = match pane_input {
            PaneInput::Text(text) => ("pane.send_text", json!({ "pane": pane, "text": text })),
            PaneInput::Key(key) => ("pane.send_keys", json!({ "pane": pane, "keys": [key] })),
        };
        self.call_reporting_errors(method, params).await;
    }

    async fn call_reporting_errors(&mut self, method: &str, params: Value) {
        if let Err(err) = self.conn.call(method, params).await {
            self.view.message = Some(format!("{err:#}"));
        }
    }

    /// Fetches the pane list and makes every pane match the client's pane area.
    async fn refresh_panes(&mut self) -> Result<()> {
        self.list_dirty = false;
        let panes: Vec<PaneInfo> = self.conn.call_as("pane.list", json!({})).await?;
        let (rows, cols) = (self.pane_area.height.max(1), self.pane_area.width.max(1));
        for pane in panes.iter().filter(|p| (p.rows, p.cols) != (rows, cols)) {
            // Failure means the pane just went away; the next list refresh drops it.
            let _ = self
                .conn
                .call(
                    "pane.resize",
                    json!({ "pane": pane.id, "rows": rows, "cols": cols }),
                )
                .await;
        }
        let folders: Vec<FolderInfo> = self.conn.call_as("folder.list", json!({})).await?;
        if self.view.set_panes(panes, folders) {
            self.screen_dirty = true;
        }
        Ok(())
    }

    async fn refresh_screen(&mut self) {
        self.screen_dirty = false;
        let (rows, cols) = (self.pane_area.height.max(1), self.pane_area.width.max(1));
        let mut screen = vt100::Parser::new(rows, cols, 0);
        let pane = self.view.focused;
        // A newly focused pane starts out live.
        let requested = if pane == self.view.screen_pane {
            self.view.scrollback
        } else {
            0
        };
        let (mut scrollback, mut alternate_screen) = (0, false);
        if let Some(pane) = pane {
            let params = json!({ "pane": pane, "format": "ansi", "scrollback": requested });
            match self.conn.call("pane.read", params).await {
                Ok(result) => {
                    screen.process(result["ansi"].as_str().unwrap_or_default().as_bytes());
                    scrollback = result["scrollback"].as_u64().unwrap_or(0) as usize;
                    alternate_screen = result["alternate_screen"].as_bool().unwrap_or(false);
                }
                Err(_) => self.list_dirty = true,
            }
        }
        // A finished selection would now highlight whatever moved under it, but
        // one still being dragged survives output so a busy pane can't break it.
        let dragging = self.view.selection.is_some_and(|s| s.dragging);
        if !dragging || pane != self.view.screen_pane {
            self.view.selection = None;
        }
        self.view.screen = screen;
        self.view.screen_pane = pane;
        self.view.scrollback = scrollback;
        self.view.alternate_screen = alternate_screen;
    }
}

/// Reads terminal events on a dedicated thread, which exits once the receiver is dropped.
fn spawn_terminal_reader() -> mpsc::UnboundedReceiver<event::Event> {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        while !tx.is_closed() {
            match event::poll(Duration::from_millis(100)) {
                Ok(true) => match event::read() {
                    Ok(event) => {
                        if tx.send(event).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                },
                Ok(false) => {}
                Err(_) => break,
            }
        }
    });
    rx
}
