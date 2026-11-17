//! Translation from crossterm key and wheel events to what is sent to a pane.

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use vt100::MouseProtocolEncoding;
use vt100::MouseProtocolMode;

#[derive(Debug, PartialEq)]
pub enum PaneInput {
    /// A text literal.
    Text(String),

    /// A key name understood by [`crate::keys::encode`].
    Key(String),
}

pub fn is_prefix(key: &KeyEvent) -> bool {
    key.code == KeyCode::Char('b') && key.modifiers.contains(KeyModifiers::CONTROL)
}

pub fn translate(key: &KeyEvent) -> Option<PaneInput> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    // BackTab is crossterm's own name for Shift+Tab.
    let shift = key.modifiers.contains(KeyModifiers::SHIFT) || key.code == KeyCode::BackTab;

    if let KeyCode::Char(c) = key.code {
        return Some(match (ctrl, alt) {
            (true, true) => PaneInput::Key(format!("alt-ctrl-{c}")),
            (true, false) => PaneInput::Key(format!("ctrl-{c}")),
            (false, true) => PaneInput::Key(format!("alt-{c}")),
            (false, false) => PaneInput::Text(c.to_string()),
        });
    }

    let base = match key.code {
        KeyCode::Enter => "enter".to_owned(),
        KeyCode::Tab | KeyCode::BackTab => "tab".to_owned(),
        KeyCode::Backspace => "backspace".to_owned(),
        KeyCode::Esc => "esc".to_owned(),
        KeyCode::Up => "up".to_owned(),
        KeyCode::Down => "down".to_owned(),
        KeyCode::Left => "left".to_owned(),
        KeyCode::Right => "right".to_owned(),
        KeyCode::Home => "home".to_owned(),
        KeyCode::End => "end".to_owned(),
        KeyCode::PageUp => "pageup".to_owned(),
        KeyCode::PageDown => "pagedown".to_owned(),
        KeyCode::Delete => "delete".to_owned(),
        KeyCode::Insert => "insert".to_owned(),
        KeyCode::F(n) => format!("f{n}"),
        _ => return None,
    };

    let ctrl_prefix = if ctrl { "ctrl-" } else { "" };
    let alt_prefix = if alt { "alt-" } else { "" };
    let shift_prefix = if shift { "shift-" } else { "" };
    Some(PaneInput::Key(format!(
        "{ctrl_prefix}{alt_prefix}{shift_prefix}{base}"
    )))
}

/// Bytes reporting one wheel notch at the 0-based cell (`col`, `row`) to a
/// program that turned on mouse reporting, or `None` if it hasn't.
pub fn encode_wheel(up: bool, col: u16, row: u16, screen: &vt100::Screen) -> Option<String> {
    if screen.mouse_protocol_mode() == MouseProtocolMode::None {
        return None;
    }

    let button: u32 = if up { 64 } else { 65 };

    let (col, row) = (u32::from(col) + 1, u32::from(row) + 1);

    let legacy = |max: u32| -> String {
        [button, col, row]
            .iter()
            .filter_map(|v| char::from_u32((v + 32).min(max)))
            .collect()
    };

    Some(match screen.mouse_protocol_encoding() {
        MouseProtocolEncoding::Sgr => format!("\x1b[<{button};{col};{row}M"),
        MouseProtocolEncoding::Utf8 => format!("\x1b[M{}", legacy(2047)),

        // Values above 127 would be mangled into UTF-8 on the way to the pane.
        MouseProtocolEncoding::Default => format!("\x1b[M{}", legacy(127)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_reports_follow_the_programs_mouse_modes() {
        let mut parser = vt100::Parser::new(10, 10, 0);
        assert_eq!(encode_wheel(true, 0, 0, parser.screen()), None);

        parser.process(b"\x1b[?1000h");
        assert_eq!(
            encode_wheel(true, 2, 4, parser.screen()).as_deref(),
            Some("\x1b[M`#%")
        );

        parser.process(b"\x1b[?1006h");
        assert_eq!(
            encode_wheel(false, 2, 4, parser.screen()).as_deref(),
            Some("\x1b[<65;3;5M")
        );
    }

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn translates_keys() {
        assert_eq!(
            translate(&key(KeyCode::Char('x'), KeyModifiers::NONE)),
            Some(PaneInput::Text("x".into()))
        );
        assert_eq!(
            translate(&key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(PaneInput::Key("ctrl-c".into()))
        );
        assert_eq!(
            translate(&key(KeyCode::Left, KeyModifiers::ALT)),
            Some(PaneInput::Key("alt-left".into()))
        );
        assert_eq!(
            translate(&key(KeyCode::F(5), KeyModifiers::NONE)),
            Some(PaneInput::Key("f5".into()))
        );
        assert_eq!(
            translate(&key(KeyCode::Enter, KeyModifiers::SHIFT)),
            Some(PaneInput::Key("shift-enter".into()))
        );
        assert_eq!(
            translate(&key(KeyCode::Left, KeyModifiers::CONTROL)),
            Some(PaneInput::Key("ctrl-left".into()))
        );
    }

    #[test]
    fn every_translated_key_is_encodable() {
        let codes = [
            KeyCode::Enter,
            KeyCode::BackTab,
            KeyCode::Up,
            KeyCode::Insert,
            KeyCode::F(12),
            KeyCode::Char('q'),
        ];
        for code in codes {
            for modifiers in [
                KeyModifiers::NONE,
                KeyModifiers::ALT,
                KeyModifiers::CONTROL,
                KeyModifiers::SHIFT,
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ] {
                if let Some(PaneInput::Key(name)) = translate(&key(code, modifiers)) {
                    assert!(
                        crate::keys::encode(&name, false).is_some(),
                        "{name} is not encodable"
                    );
                }
            }
        }
    }
}
