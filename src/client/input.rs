//! Translation of crossterm key events into what gets sent to a pane.

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;

#[derive(Debug, PartialEq)]
pub enum PaneInput {
    /// Literal text, typed as-is.
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
    let alt_prefix = if alt { "alt-" } else { "" };

    let named = match key.code {
        KeyCode::Char(c) if ctrl => return Some(PaneInput::Key(format!("{alt_prefix}ctrl-{c}"))),
        KeyCode::Char(c) if alt => return Some(PaneInput::Key(format!("alt-{c}"))),
        KeyCode::Char(c) => return Some(PaneInput::Text(c.to_string())),
        KeyCode::Enter => "enter".to_owned(),
        KeyCode::Tab => "tab".to_owned(),
        KeyCode::BackTab => "shift-tab".to_owned(),
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
    Some(PaneInput::Key(format!("{alt_prefix}{named}")))
}

#[cfg(test)]
mod tests {
    use super::*;

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
            for modifiers in [KeyModifiers::NONE, KeyModifiers::ALT, KeyModifiers::CONTROL] {
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
