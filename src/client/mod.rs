//! The TUI: one client of the server, showing a pane list and the focused pane.

mod input;
mod render;
mod view;

use std::io::stdout;
use std::sync::Arc;
use std::time::Duration;
use std::time::SystemTime;

use anyhow::Result;
use anyhow::bail;
use crossterm::event::DisableBracketedPaste;
use crossterm::event::EnableBracketedPaste;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use crossterm::event::{self};
use crossterm::execute;
use ratatui::DefaultTerminal;
use ratatui::layout::Rect;
use serde_json::Value;
use serde_json::json;
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;

use crate::protocol::Event;
use crate::protocol::PaneId;
use crate::protocol::PaneInfo;
use crate::protocol::events;
use crate::rpc::Connection;
use input::PaneInput;
use view::Mode;
use view::PromptKind;
use view::View;
use view::areas;

const CLOCK_TICK: Duration = Duration::from_secs(1);

pub async fn attach() -> Result<()> {
    let (conn, server_events) = Connection::connect_or_spawn().await?;
    conn.call("events.subscribe", json!({})).await?;

    let mut terminal = ratatui::init();
    let _ = execute!(stdout(), EnableBracketedPaste);
    let result = App::new(conn).run(&mut terminal, server_events).await;
    let _ = execute!(stdout(), DisableBracketedPaste);
    ratatui::restore();
    result
}

struct App {
    conn: Arc<Connection>,
    view: View,
    pane_area: Rect,
    list_dirty: bool,
    screen_dirty: bool,
    quit: bool,
}

impl App {
    fn new(conn: Arc<Connection>) -> Self {
        Self {
            conn,
            view: View::new(),
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
        self.pane_area = areas(Rect::new(0, 0, size.width, size.height)).pane;
        // Redraws periodically so the sidebar's status durations keep counting.
        let mut clock = tokio::time::interval(CLOCK_TICK);
        clock.set_missed_tick_behavior(MissedTickBehavior::Delay);

        while !self.quit {
            if self.list_dirty {
                self.refresh_panes().await?;
            }
            if self.screen_dirty {
                self.refresh_screen().await;
            }
            terminal.draw(|frame| render::render(frame, &self.view, SystemTime::now()))?;

            tokio::select! {
                _ = clock.tick() => {}
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
            event::Event::Resize(width, height) => {
                self.pane_area = areas(Rect::new(0, 0, width, height)).pane;
                self.list_dirty = true;
            }
            _ => {}
        }
    }

    async fn on_key(&mut self, key: KeyEvent) {
        self.view.message = None;
        match std::mem::replace(&mut self.view.mode, Mode::Normal) {
            Mode::Normal if input::is_prefix(&key) => self.view.mode = Mode::Prefix,
            Mode::Normal => {
                if let Some(pane_input) = input::translate(&key) {
                    self.send_to_focused(pane_input).await;
                }
            }
            Mode::Prefix => self.on_prefix_command(key).await,
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

    async fn on_prefix_command(&mut self, key: KeyEvent) {
        match key.code {
            _ if input::is_prefix(&key) => {
                self.send_to_focused(PaneInput::Key("ctrl-b".into())).await
            }
            KeyCode::Char('c') => self.create_pane(None).await,
            KeyCode::Char('a') => {
                self.view.mode = Mode::Prompt {
                    kind: PromptKind::NewPane,
                    input: String::new(),
                };
            }
            KeyCode::Char('r') => {
                if let Some(pane) = self.view.focused_pane() {
                    let input = pane.name.clone();
                    self.view.mode = Mode::Prompt {
                        kind: PromptKind::Rename,
                        input,
                    };
                }
            }
            KeyCode::Char('n' | 'j') | KeyCode::Down => {
                self.change_focus(|view| view.cycle_focus(1))
            }
            KeyCode::Char('p' | 'k') | KeyCode::Up => {
                self.change_focus(|view| view.cycle_focus(-1))
            }
            KeyCode::Char(digit @ '1'..='9') => {
                let index = digit as usize - '1' as usize;
                self.change_focus(|view| view.focus_index(index));
            }
            KeyCode::Char('x') => {
                if let Some(pane) = self.view.focused {
                    self.call_reporting_errors("pane.close", json!({ "pane": pane }))
                        .await;
                }
            }
            KeyCode::Char('d' | 'q') => self.quit = true,
            _ => {}
        }
    }

    async fn submit_prompt(&mut self, kind: PromptKind, input: String) {
        let input = input.trim().to_owned();
        match kind {
            PromptKind::NewPane if !input.is_empty() => self.create_pane(Some(input)).await,
            PromptKind::Rename if !input.is_empty() => {
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

    async fn create_pane(&mut self, command: Option<String>) {
        let params = json!({
            "command": command,
            "cwd": std::env::current_dir().ok(),
            "rows": self.pane_area.height,
            "cols": self.pane_area.width,
        });
        match self.conn.call_as::<PaneInfo>("pane.create", params).await {
            Ok(info) => {
                self.view.focused = Some(info.id);
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
        if self.view.set_panes(panes) {
            self.screen_dirty = true;
        }
        Ok(())
    }

    async fn refresh_screen(&mut self) {
        self.screen_dirty = false;
        let (rows, cols) = (self.pane_area.height.max(1), self.pane_area.width.max(1));
        let mut screen = vt100::Parser::new(rows, cols, 0);
        if let Some(pane) = self.view.focused {
            match self
                .conn
                .call("pane.read", json!({ "pane": pane, "format": "ansi" }))
                .await
            {
                Ok(result) => {
                    screen.process(result["ansi"].as_str().unwrap_or_default().as_bytes())
                }
                Err(_) => self.list_dirty = true,
            }
        }
        self.view.screen = screen;
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
