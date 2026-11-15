//! Pure rendering: draws a [`View`] and never mutates it.

use std::time::Duration;
use std::time::SystemTime;

use ratatui::Frame;
use ratatui::layout::Margin;
use ratatui::layout::Position;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Block;
use ratatui::widgets::Borders;
use ratatui::widgets::Clear;
use ratatui::widgets::Paragraph;
use tui_term::widget::Cursor;
use tui_term::widget::PseudoTerminal;

use super::view::Areas;
use super::view::Mode;
use super::view::Row;
use super::view::View;
use super::view::areas;
use crate::detect::AgentState;
use crate::protocol::FolderInfo;
use crate::protocol::PaneInfo;
use crate::protocol::unix_millis;

/// One-line summary for the status bar. `? help` comes first so it survives truncation on
/// narrow screens, where [`HELP`] is the full reference.
const HINTS: &str = " ctrl-b  ? help · c shell · a command · f folder · n/p switch · J/K move · \
                     r rename · x close · </> resize · b sidebar · d detach";
const RESIZE_HINTS: &str = " resize  ←/→ adjust · shift ±8 · = reset · esc done";

/// Sections of the help popup: a heading, then (keys, action) rows.
const HELP: &[(&str, &[(&str, &str)])] = &[
    (
        "ctrl-b, then",
        &[
            ("c", "new shell pane in the current folder"),
            ("a", "new pane running a command"),
            ("f", "new folder"),
            ("n / j / ↓", "next pane or folder"),
            ("p / k / ↑", "previous pane or folder"),
            ("1–9", "focus pane by position"),
            ("J / shift-↓", "move pane down, across folders"),
            ("K / shift-↑", "move pane up, across folders"),
            ("r", "rename pane or folder"),
            ("x", "close pane / delete folder (keeps panes)"),
            ("< / >", "resize sidebar"),
            ("b", "hide/show sidebar"),
            ("d / q", "detach"),
            ("ctrl-b", "send a literal ctrl-b"),
            ("?", "show this help"),
        ],
    ),
    (
        "resize mode",
        &[
            ("← / → / h / l", "adjust by 2"),
            ("shift / H / L", "adjust by 8"),
            ("=", "reset width"),
            ("any other key", "done"),
        ],
    ),
];

/// Draws `view` as of `now`, which dates the sidebar's status durations.
pub fn render(frame: &mut Frame, view: &View, now: SystemTime) {
    let Areas {
        sidebar,
        pane,
        status,
    } = areas(frame.area(), &view.sidebar);
    if sidebar.width > 0 {
        render_sidebar(frame, sidebar, view, unix_millis(now));
    }
    render_pane(frame, pane, view);
    render_status(frame, status, view);
    if sidebar.width > 0 {
        let junction = Position::new(sidebar.right() - 1, status.y);
        if let Some(cell) = frame.buffer_mut().cell_mut(junction) {
            cell.set_symbol("┴");
        }
    }
    if matches!(view.mode, Mode::Help) {
        render_help(frame, frame.area());
    }
}

/// Draws [`HELP`] in a bordered popup centered on `area`, clipped if `area` is too small.
fn render_help(frame: &mut Frame, area: Rect) {
    let key_width = HELP
        .iter()
        .flat_map(|(_, rows)| rows.iter())
        .map(|(keys, _)| keys.chars().count())
        .max()
        .unwrap_or(0);

    let mut lines = Vec::new();
    for (index, (heading, rows)) in HELP.iter().enumerate() {
        if index > 0 {
            lines.push(Line::default());
        }
        lines.push(Line::from(Span::styled(*heading, Style::new().bold())));
        for (keys, action) in rows.iter() {
            lines.push(Line::from(vec![
                Span::styled(format!("  {keys:<key_width$}  "), Style::new().cyan()),
                Span::raw(*action),
            ]));
        }
    }

    let content_width = lines.iter().map(Line::width).max().unwrap_or(0) as u16;
    let block = Block::bordered()
        .border_style(Style::new().dark_gray())
        .title(" help ".bold())
        .title_bottom(" any key to close ".dark_gray());
    let width = (content_width + 4).min(area.width);
    let height = (lines.len() as u16 + 2).min(area.height);
    let popup = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };

    let inner = block.inner(popup).inner(Margin::new(1, 0));
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_sidebar(frame: &mut Frame, area: Rect, view: &View, now_ms: u64) {
    let block = Block::new()
        .borders(Borders::RIGHT)
        .border_style(Style::new().dark_gray());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let selected = view.selected_row();
    // Pane numbers skip folder rows, matching `ctrl-b 1`–`9`.
    let mut pane_number = 0;
    let lines: Vec<Line> = view
        .rows()
        .into_iter()
        .filter_map(|row| match row {
            Row::Folder(id) => {
                let folder = view.folders.iter().find(|folder| folder.id == id)?;
                let count = view
                    .panes
                    .iter()
                    .filter(|pane| view.group_of(pane) == Some(id))
                    .count();
                Some(folder_row(
                    folder,
                    count,
                    selected == Some(row),
                    inner.width,
                ))
            }
            Row::Pane(id) => {
                let pane = view.panes.iter().find(|pane| pane.id == id)?;
                pane_number += 1;
                Some(sidebar_row(
                    pane_number,
                    pane,
                    PaneRowStyle {
                        selected: selected == Some(row),
                        focused: view.focused == Some(id),
                        indented: view.group_of(pane).is_some(),
                    },
                    inner.width,
                    now_ms,
                ))
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn folder_row(folder: &FolderInfo, pane_count: usize, selected: bool, width: u16) -> Line<'static> {
    let marker = if selected { "▌" } else { " " };
    let count = pane_count.to_string();
    let name_width = (width as usize).saturating_sub(1 + 2 + 1 + count.len());
    let name: String = folder.name.chars().take(name_width).collect();

    Line::from(vec![
        Span::styled(marker, Style::new().cyan()),
        Span::styled("▾ ", Style::new().dark_gray()),
        Span::styled(format!("{name:<name_width$} "), Style::new().bold()),
        Span::styled(count, Style::new().dark_gray()),
    ])
}

struct PaneRowStyle {
    /// The row has the sidebar's selection marker.
    selected: bool,

    /// The pane is the one on screen.
    focused: bool,

    /// The pane is inside a folder.
    indented: bool,
}

fn sidebar_row(
    number: usize,
    pane: &PaneInfo,
    style: PaneRowStyle,
    width: u16,
    now_ms: u64,
) -> Line<'static> {
    let (icon, color) = status_icon(pane.status, now_ms);
    let label = match pane.status {
        AgentState::Unknown => String::new(),
        _ => format_elapsed(Duration::from_millis(
            now_ms.saturating_sub(pane.status_since),
        )),
    };
    let marker = if style.selected { "▌" } else { " " };
    let indent = if style.indented { " " } else { "" };
    let name_width = (width as usize).saturating_sub(4 + indent.len() + 1 + label.len() + 1);
    let name: String = format!("{number} {}", pane.name)
        .chars()
        .take(name_width)
        .collect();
    let row_style = if style.focused {
        Style::new().add_modifier(Modifier::BOLD)
    } else {
        Style::new()
    };

    Line::from(vec![
        Span::styled(marker, Style::new().cyan()),
        Span::raw(indent),
        Span::styled(format!("{icon} "), Style::new().fg(color)),
        Span::styled(format!("{name:<name_width$} "), row_style),
        Span::styled(label, Style::new().fg(color)),
    ])
}

/// Spinner frames for a working agent, advanced every [`ANIMATION_FRAME`].
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// How often animated status icons advance.
pub const ANIMATION_FRAME: Duration = Duration::from_millis(100);

/// Blocked agents blink, toggling once per this many animation frames.
const BLINK_FRAMES: u64 = 5;

/// Whether `state` is drawn with an animated icon, so the sidebar needs
/// redrawing every [`ANIMATION_FRAME`].
pub fn is_animated(state: AgentState) -> bool {
    matches!(state, AgentState::Working | AgentState::Blocked)
}

/// One-column icon for `state`, animated off the wall clock so every pane in
/// the same state moves in step.
fn status_icon(state: AgentState, now_ms: u64) -> (&'static str, Color) {
    let frame = now_ms / ANIMATION_FRAME.as_millis() as u64;
    match state {
        AgentState::Working => (
            SPINNER[(frame % SPINNER.len() as u64) as usize],
            Color::Yellow,
        ),
        AgentState::Blocked if (frame / BLINK_FRAMES).is_multiple_of(2) => ("●", Color::Red),
        AgentState::Blocked => ("○", Color::Red),
        AgentState::Idle => ("●", Color::Green),
        AgentState::Unknown => ("○", Color::DarkGray),
    }
}

/// Compact elapsed time in the largest whole unit: `42s`, `5m`, `3h`, `2d`.
fn format_elapsed(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    match secs {
        0..60 => format!("{secs}s"),
        60..3_600 => format!("{}m", secs / 60),
        3_600..86_400 => format!("{}h", secs / 3_600),
        _ => format!("{}d", secs / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_uses_largest_whole_unit() {
        let format = |secs| format_elapsed(Duration::from_secs(secs));
        assert_eq!(format(0), "0s");
        assert_eq!(format(59), "59s");
        assert_eq!(format(60), "1m");
        assert_eq!(format(3_599), "59m");
        assert_eq!(format(3_600), "1h");
        assert_eq!(format(86_399), "23h");
        assert_eq!(format(86_400 * 3), "3d");
    }

    fn draw_help(width: u16, height: u16) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| render_help(frame, frame.area()))
            .unwrap();
        terminal.backend().to_string()
    }

    #[test]
    fn help_lists_every_action_and_fits_tiny_screens() {
        let screen = draw_help(80, 30);
        for (_, rows) in HELP {
            for (_, action) in rows.iter() {
                assert!(screen.contains(action), "{action} missing from:\n{screen}");
            }
        }
        draw_help(3, 2);
    }

    #[test]
    fn sidebar_shows_folders_after_top_level_panes() {
        let pane = |id, folder| PaneInfo {
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
            folder,
        };
        let folder = |id, name: &str| FolderInfo {
            id,
            name: name.into(),
        };
        let mut view = View::new(super::super::view::Sidebar::default());
        view.set_panes(
            vec![pane(1, None), pane(2, Some(10)), pane(3, Some(10))],
            vec![folder(10, "backend"), folder(20, "frontend")],
        );
        view.cycle_focus(1);

        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(16, 6)).unwrap();
        terminal
            .draw(|frame| render_sidebar(frame, frame.area(), &view, 0))
            .unwrap();
        let lines: Vec<String> = terminal
            .backend()
            .buffer()
            .content()
            .chunks(16)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect())
            .collect();
        assert_eq!(
            lines,
            [
                " ○ 1 p1        │",
                "▌▾ backend    2│",
                "  ○ 2 p2       │",
                "  ○ 3 p3       │",
                " ▾ frontend   0│",
                "               │",
            ]
        );
    }
}

fn render_pane(frame: &mut Frame, area: Rect, view: &View) {
    if view.focused.is_none() {
        let hint =
            Paragraph::new("no panes — press ctrl-b c for a shell, ctrl-b a to run a command")
                .dark_gray()
                .centered();
        let middle = Rect {
            y: area.y + area.height / 2,
            height: 1,
            ..area
        };
        frame.render_widget(hint, middle);
        return;
    }

    let screen = view.screen.screen();
    let mut cursor = Cursor::default();
    cursor.hide();
    frame.render_widget(PseudoTerminal::new(screen).cursor(cursor), area);

    if matches!(view.mode, Mode::Normal)
        && view.scrollback == 0
        && view.selected_folder.is_none()
        && !screen.hide_cursor()
    {
        let (row, col) = screen.cursor_position();
        if row < area.height && col < area.width {
            frame.set_cursor_position(Position::new(area.x + col, area.y + row));
        }
    }
}

fn render_status(frame: &mut Frame, area: Rect, view: &View) {
    let block = Block::new()
        .borders(Borders::TOP)
        .border_style(Style::new().dark_gray());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    render_status_line(frame, inner, view);
}

fn render_status_line(frame: &mut Frame, area: Rect, view: &View) {
    let line = match &view.mode {
        Mode::Prompt { kind, input } => {
            let prefix = format!(" {}: ", kind.label());
            let cursor_x = area.x + (prefix.chars().count() + input.chars().count()) as u16;
            frame.set_cursor_position(Position::new(
                cursor_x.min(area.right().saturating_sub(1)),
                area.y,
            ));
            let mut spans = vec![
                Span::styled(prefix, Style::new().cyan()),
                Span::raw(input.clone()),
            ];
            if let Some(message) = &view.message {
                spans.push(Span::styled(format!("  {message}"), Style::new().red()));
            }
            Line::from(spans)
        }
        Mode::Prefix => Line::from(Span::styled(HINTS, Style::new().black().on_cyan())),
        Mode::Help => Line::from(Span::styled(HINTS, Style::new().dark_gray())),
        Mode::Resize => {
            let width = view.sidebar.width_on(area.width);
            Line::from(vec![
                Span::styled(RESIZE_HINTS, Style::new().black().on_cyan()),
                Span::styled(format!("  width {width}"), Style::new().cyan()),
            ])
        }
        Mode::Normal => match &view.message {
            Some(message) => Line::from(Span::styled(format!(" {message}"), Style::new().red())),
            None if let Some(folder) = view.selected_folder() => Line::from(vec![
                Span::styled(
                    format!(" folder {} ", folder.name),
                    Style::new().black().on_cyan(),
                ),
                Span::styled(
                    "  ctrl-b r rename · x delete (keeps its panes) · c new pane in it",
                    Style::new().cyan(),
                ),
            ]),
            None if view.scrollback > 0 => Line::from(Span::styled(
                format!(
                    " scrolled back {} lines · scroll down or type to return",
                    view.scrollback
                ),
                Style::new().black().on_yellow(),
            )),
            None => Line::from(Span::styled(HINTS, Style::new().dark_gray())),
        },
    };
    frame.render_widget(Paragraph::new(line), area);
}
