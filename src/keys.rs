//! Named keys (`enter`, `ctrl-c`, `alt-left`, …) and their terminal byte encodings.

/// Encodes a key name into the bytes a terminal would send. `application_cursor`
/// is the pane's DECCKM mode, which changes how arrow/home/end keys are encoded.
/// A bare single character encodes as itself, so `alt-X` and `X` keep their case.
pub fn encode(name: &str, application_cursor: bool) -> Option<Vec<u8>> {
    let lower = name.to_ascii_lowercase();

    if lower.starts_with("alt-") {
        let mut bytes = vec![0x1b];
        bytes.extend(encode(&name[4..], application_cursor)?);
        return Some(bytes);
    }
    if let Some(rest) = lower.strip_prefix("ctrl-") {
        return single_char(rest)
            .and_then(|c| control_byte(c[0]))
            .map(|b| vec![b]);
    }

    let cursor = |c: char| -> Vec<u8> {
        let prefix = if application_cursor { "\x1bO" } else { "\x1b[" };
        format!("{prefix}{c}").into_bytes()
    };
    let bytes = match lower.as_str() {
        "enter" | "return" => b"\r".to_vec(),
        "tab" => b"\t".to_vec(),
        "shift-tab" | "backtab" => b"\x1b[Z".to_vec(),
        "backspace" => b"\x7f".to_vec(),
        "esc" | "escape" => b"\x1b".to_vec(),
        "space" => b" ".to_vec(),
        "up" => cursor('A'),
        "down" => cursor('B'),
        "right" => cursor('C'),
        "left" => cursor('D'),
        "home" => cursor('H'),
        "end" => cursor('F'),
        "insert" => b"\x1b[2~".to_vec(),
        "delete" => b"\x1b[3~".to_vec(),
        "pageup" => b"\x1b[5~".to_vec(),
        "pagedown" => b"\x1b[6~".to_vec(),
        "f1" => b"\x1bOP".to_vec(),
        "f2" => b"\x1bOQ".to_vec(),
        "f3" => b"\x1bOR".to_vec(),
        "f4" => b"\x1bOS".to_vec(),
        "f5" => b"\x1b[15~".to_vec(),
        "f6" => b"\x1b[17~".to_vec(),
        "f7" => b"\x1b[18~".to_vec(),
        "f8" => b"\x1b[19~".to_vec(),
        "f9" => b"\x1b[20~".to_vec(),
        "f10" => b"\x1b[21~".to_vec(),
        "f11" => b"\x1b[23~".to_vec(),
        "f12" => b"\x1b[24~".to_vec(),
        _ => return single_char(name),
    };
    Some(bytes)
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
        assert_eq!(encode("alt-left", false).unwrap(), b"\x1b\x1b[D");
        assert_eq!(encode("ctrl-space", false).unwrap(), vec![0]);
    }

    #[test]
    fn arrows_follow_application_cursor_mode() {
        assert_eq!(encode("up", false).unwrap(), b"\x1b[A");
        assert_eq!(encode("up", true).unwrap(), b"\x1bOA");
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(encode("hyper-q", false).is_none());
        assert!(encode("ctrl-ab", false).is_none());
    }
}
