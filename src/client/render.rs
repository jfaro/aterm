//! Pure rendering: draws a [`View`] and never mutates it.

use std::time::Duration;
use std::time::SystemTime;

use ratatui::Frame;
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
use ratatui::widgets::Paragraph;
use tui_term::widget::Cursor;
use tui_term::widget::PseudoTerminal;

use super::view::Areas;
use super::view::Mode;
use super::view::View;
use super::view::areas;
use crate::detect::AgentState;
use crate::protocol::PaneInfo;
use crate::protocol::unix_millis;

const HINTS: &str =
    " ctrl-b  c shell · a command · n/p switch · J/K move · r rename · x close · d detach";

/// Draws `view` as of `now`, which dates the sidebar's status durations.
pub fn render(frame: &mut Frame, view: &View, now: SystemTime) {
    let Areas {
        sidebar,
        pane,
        status,
    } = areas(frame.area());
    render_sidebar(frame, sidebar, view, unix_millis(now));
    render_pane(frame, pane, view);
    render_status(frame, status, view);
}

fn render_sidebar(frame: &mut Frame, area: Rect, view: &View, now_ms: u64) {
    let block = Block::new()
        .borders(Borders::RIGHT)
        .border_style(Style::new().dark_gray())
        .title(" aterm ".bold());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines: Vec<Line> = view
        .panes
        .iter()
        .enumerate()
        .map(|(index, pane)| {
            sidebar_row(
                index,
                pane,
                Some(pane.id) == view.focused,
                inner.width,
                now_ms,
            )
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn sidebar_row(
    index: usize,
    pane: &PaneInfo,
    focused: bool,
    width: u16,
    now_ms: u64,
) -> Line<'static> {
    let (dot, color, state) = match pane.status {
        AgentState::Working => ("●", Color::Yellow, "working"),
        AgentState::Blocked => ("●", Color::Red, "blocked"),
        AgentState::Idle => ("●", Color::Green, "idle"),
        AgentState::Unknown => ("○", Color::DarkGray, ""),
    };
    let label = if state.is_empty() {
        String::new()
    } else {
        let elapsed = Duration::from_millis(now_ms.saturating_sub(pane.status_since));
        format!("{state} {}", format_elapsed(elapsed))
    };
    let marker = if focused { "▌" } else { " " };
    let name_width = (width as usize).saturating_sub(4 + 1 + label.len() + 1);
    let name: String = format!("{} {}", index + 1, pane.name)
        .chars()
        .take(name_width)
        .collect();
    let row_style = if focused {
        Style::new().add_modifier(Modifier::BOLD)
    } else {
        Style::new()
    };

    Line::from(vec![
        Span::styled(marker, Style::new().cyan()),
        Span::styled(format!("{dot} "), Style::new().fg(color)),
        Span::styled(format!("{name:<name_width$} "), row_style),
        Span::styled(label, Style::new().fg(color)),
    ])
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

    if matches!(view.mode, Mode::Normal) && !screen.hide_cursor() {
        let (row, col) = screen.cursor_position();
        if row < area.height && col < area.width {
            frame.set_cursor_position(Position::new(area.x + col, area.y + row));
        }
    }
}

fn render_status(frame: &mut Frame, area: Rect, view: &View) {
    let line = match &view.mode {
        Mode::Prompt { kind, input } => {
            let prefix = format!(" {}: ", kind.label());
            let cursor_x = area.x + (prefix.chars().count() + input.chars().count()) as u16;
            frame.set_cursor_position(Position::new(
                cursor_x.min(area.right().saturating_sub(1)),
                area.y,
            ));
            Line::from(vec![
                Span::styled(prefix, Style::new().cyan()),
                Span::raw(input.clone()),
            ])
        }
        Mode::Prefix => Line::from(Span::styled(HINTS, Style::new().black().on_cyan())),
        Mode::Normal => match &view.message {
            Some(message) => Line::from(Span::styled(format!(" {message}"), Style::new().red())),
            None => Line::from(Span::styled(HINTS, Style::new().dark_gray())),
        },
    };
    frame.render_widget(Paragraph::new(line), area);
}
