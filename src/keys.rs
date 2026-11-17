//! Named keys (`enter`, `ctrl-c`, `alt-left`, …) and their terminal byte encodings.

/// Encodes a key name into the bytes a terminal would send. `application_cursor`
/// is the pane's DECCKM mode, which changes how an unmodified arrow key is
/// encoded. Names may carry any combination of `ctrl-`, `alt-`, `shift-`
/// prefixes (in any order); a bare single character encodes as itself, so
/// `alt-X` and `X` keep their case.
pub fn encode(name: &str, application_cursor: bool) -> Option<Vec<u8>> {
    let (rest, ctrl, alt, shift) = strip_modifiers(name);
    let lower = rest.to_ascii_lowercase();

    // A lone character: ctrl turns it into a control byte, alt prefixes ESC.
    // Shift is already reflected in the character's own case.
    if let Some(c) = single_char(rest) {
        let mut bytes = if ctrl {
            vec![control_byte(c[0].to_ascii_lowercase())?]
        } else {
            c
        };
        if alt {
            bytes.insert(0, 0x1b);
        }
        return Some(bytes);
    }

    // Enter has no standard modifier-aware encoding, so pick bytes distinct
    // from plain `\r` for Shift+Enter: a raw `\n`, the convention programs
    // that read raw input (e.g. a chat REPL) use for "insert newline" rather
    // than "submit".
    if lower == "enter" || lower == "return" {
        let mut bytes = if shift { b"\n".to_vec() } else { b"\r".to_vec() };
        if alt {
            bytes.insert(0, 0x1b);
        }
        return Some(bytes);
    }

    if lower == "backtab" || (shift && lower == "tab") {
        let mut bytes = b"\x1b[Z".to_vec();
        if alt {
            bytes.insert(0, 0x1b);
        }
        return Some(bytes);
    }

    if let Some(bytes) = navigation(&lower, ctrl, alt, shift, application_cursor) {
        return Some(bytes);
    }

    let mut bytes = match lower.as_str() {
        "tab" => b"\t".to_vec(),
        "backspace" => b"\x7f".to_vec(),
        "esc" | "escape" => b"\x1b".to_vec(),
        "space" => b" ".to_vec(),
        _ => return None,
    };
    if alt {
        bytes.insert(0, 0x1b);
    }
    Some(bytes)
}

/// Strips any combination of `ctrl-`/`alt-`/`shift-` prefixes (in any order,
/// case-insensitively) off the front of `name`, returning the remainder with
/// its original case intact.
fn strip_modifiers(name: &str) -> (&str, bool, bool, bool) {
    let mut rest = name;
    let (mut ctrl, mut alt, mut shift) = (false, false, false);
    loop {
        let lower = rest.to_ascii_lowercase();
        if lower.starts_with("ctrl-") {
            ctrl = true;
            rest = &rest[5..];
        } else if lower.starts_with("alt-") {
            alt = true;
            rest = &rest[4..];
        } else if lower.starts_with("shift-") {
            shift = true;
            rest = &rest[6..];
        } else {
            break;
        }
    }
    (rest, ctrl, alt, shift)
}

/// Encodes a navigation or function key with modifiers using the standard
/// xterm `CSI <num>;<mod>~` / `CSI 1;<mod><letter>` forms, or `None` if
/// `lower` isn't one of those keys.
fn navigation(
    lower: &str,
    ctrl: bool,
    alt: bool,
    shift: bool,
    application_cursor: bool,
) -> Option<Vec<u8>> {
    let modcode = 1 + shift as u8 + 2 * alt as u8 + 4 * ctrl as u8;

    let cursor_letter = match lower {
        "up" => 'A',
        "down" => 'B',
        "right" => 'C',
        "left" => 'D',
        "home" => 'H',
        "end" => 'F',
        _ => return numbered_or_f1_to_f4(lower, modcode),
    };
    Some(if modcode == 1 {
        let prefix = if application_cursor { "\x1bO" } else { "\x1b[" };
        format!("{prefix}{cursor_letter}").into_bytes()
    } else {
        format!("\x1b[1;{modcode}{cursor_letter}").into_bytes()
    })
}

fn numbered_or_f1_to_f4(lower: &str, modcode: u8) -> Option<Vec<u8>> {
    let numbered = match lower {
        "insert" => 2,
        "delete" => 3,
        "pageup" => 5,
        "pagedown" => 6,
        "f5" => 15,
        "f6" => 17,
        "f7" => 18,
        "f8" => 19,
        "f9" => 20,
        "f10" => 21,
        "f11" => 23,
        "f12" => 24,
        _ => 0,
    };
    if numbered != 0 {
        return Some(if modcode == 1 {
            format!("\x1b[{numbered}~").into_bytes()
        } else {
            format!("\x1b[{numbered};{modcode}~").into_bytes()
        });
    }

    let f_letter = match lower {
        "f1" => 'P',
        "f2" => 'Q',
        "f3" => 'R',
        "f4" => 'S',
        _ => return None,
    };
    Some(if modcode == 1 {
        format!("\x1bO{f_letter}").into_bytes()
    } else {
        format!("\x1b[1;{modcode}{f_letter}").into_bytes()
    })
}

fn single_char(s: &str) -> Option<Vec<u8>> {
    let mut chars = s.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(c.to_string().into_bytes()),
        _ if s == "space" => Some(b" ".to_vec()),
        _ => None,
    }
}

fn control_byte(c: u8) -> Option<u8> {
    match c {
        b'a'..=b'z' => Some(c - b'a' + 1),
        b'@' | b' ' | b'2' => Some(0),
        b'[' | b'3' => Some(0x1b),
        b'\\' | b'4' => Some(0x1c),
        b']' | b'5' => Some(0x1d),
        b'^' | b'6' => Some(0x1e),
        b'_' | b'7' | b'/' => Some(0x1f),
        b'?' | b'8' => Some(0x7f),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::encode;

    #[test]
    fn encodes_common_keys() {
        assert_eq!(encode("enter", false).unwrap(), b"\r");
        assert_eq!(encode("ctrl-c", false).unwrap(), vec![3]);
        assert_eq!(encode("CTRL-A", false).unwrap(), vec![1]);
        assert_eq!(encode("alt-x", false).unwrap(), b"\x1bx");
        assert_eq!(encode("alt-X", false).unwrap(), b"\x1bX");
        assert_eq!(encode("q", false).unwrap(), b"q");
        assert_eq!(encode("alt-left", false).unwrap(), b"\x1b[1;3D");
        assert_eq!(encode("ctrl-space", false).unwrap(), vec![0]);
    }

    #[test]
    fn arrows_follow_application_cursor_mode() {
        assert_eq!(encode("up", false).unwrap(), b"\x1b[A");
        assert_eq!(encode("up", true).unwrap(), b"\x1bOA");
    }

    #[test]
    fn shift_enter_is_distinct_from_enter() {
        assert_eq!(encode("shift-enter", false).unwrap(), b"\n");
        assert_eq!(encode("enter", false).unwrap(), b"\r");
    }

    #[test]
    fn ctrl_arrows_carry_the_modifier() {
        assert_eq!(encode("ctrl-left", false).unwrap(), b"\x1b[1;5D");
        assert_eq!(encode("ctrl-right", false).unwrap(), b"\x1b[1;5C");
        // Order of prefixes shouldn't matter.
        assert_eq!(
            encode("alt-ctrl-left", false).unwrap(),
            encode("ctrl-alt-left", false).unwrap()
        );
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(encode("hyper-q", false).is_none());
        assert!(encode("ctrl-ab", false).is_none());
    }
}
