//! Mouse text selection over the mirrored pane screen, copied out through OSC 52.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ratatui::layout::Position;

/// A linear run of cells, in pane-relative coordinates, from where the drag
/// started to where the pointer is now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    /// Cell the drag started on.
    pub anchor: Position,

    /// Cell the pointer is on now.
    pub head: Position,

    /// Whether the button is still held.
    pub dragging: bool,
}

impl Selection {
    pub fn start(at: Position) -> Self {
        Self {
            anchor: at,
            head: at,
            dragging: true,
        }
    }

    /// Whether the selection covers no more than the cell it started on.
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// First and last selected cells in reading order, both inclusive.
    pub fn bounds(&self) -> (Position, Position) {
        let key = |p: Position| (p.y, p.x);
        if key(self.anchor) <= key(self.head) {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    pub fn contains(&self, cell: Position) -> bool {
        let (start, end) = self.bounds();
        let key = (cell.y, cell.x);
        (start.y, start.x) <= key && key <= (end.y, end.x)
    }

    /// The selected text, with soft-wrapped rows joined and trailing blanks dropped.
    pub fn text(&self, screen: &vt100::Screen) -> String {
        let (rows, cols) = screen.size();
        if rows == 0 || cols == 0 {
            return String::new();
        }
        // `contents_between` underflows on columns past the screen's width.
        let clamp = |p: Position| (p.y.min(rows - 1), p.x.min(cols - 1));
        let (start, end) = self.bounds();
        let ((start_row, start_col), (end_row, end_col)) = (clamp(start), clamp(end));
        let text = screen.contents_between(start_row, start_col, end_row, end_col + 1);
        // Soft-wrapped rows are already joined, so this only trims hard line ends.
        text.split('\n')
            .map(|line| line.trim_end_matches(' '))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Escape sequence asking the outer terminal to put `text` on the system clipboard.
pub fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", STANDARD.encode(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection(anchor: (u16, u16), head: (u16, u16)) -> Selection {
        Selection {
            anchor: Position::new(anchor.0, anchor.1),
            head: Position::new(head.0, head.1),
            dragging: false,
        }
    }

    #[test]
    fn backwards_drags_normalize_to_reading_order() {
        let backwards = selection((2, 3), (5, 1));
        assert_eq!(
            backwards.bounds(),
            (Position::new(5, 1), Position::new(2, 3))
        );
        assert_eq!(selection((5, 1), (2, 3)).bounds(), backwards.bounds());
        assert!(selection((1, 1), (1, 1)).is_empty());
    }

    #[test]
    fn contains_runs_linearly_across_rows() {
        let s = selection((5, 1), (2, 3));
        assert!(!s.contains(Position::new(4, 1)));
        assert!(s.contains(Position::new(5, 1)));
        assert!(s.contains(Position::new(79, 1)));
        assert!(s.contains(Position::new(0, 2)));
        assert!(s.contains(Position::new(2, 3)));
        assert!(!s.contains(Position::new(3, 3)));
        assert!(!s.contains(Position::new(0, 4)));
    }

    #[test]
    fn text_joins_wrapped_rows_and_trims_trailing_blanks() {
        // Rows: "abcde " soft-wraps into "fgh", then "xy   ", then "last".
        let mut parser = vt100::Parser::new(4, 6, 0);
        parser.process(b"abcde fgh\r\nxy   \r\nlast");
        let screen = parser.screen();
        assert_eq!(selection((0, 0), (5, 2)).text(screen), "abcde fgh\nxy");
        assert_eq!(selection((2, 3), (1, 0)).text(screen), "bcde fgh\nxy\nlas");
        assert_eq!(selection((1, 0), (2, 0)).text(screen), "bc");
        // Coordinates past the screen are clamped rather than panicking.
        assert_eq!(selection((0, 3), (99, 99)).text(screen), "last");
    }

    #[test]
    fn osc52_base64_encodes_for_the_clipboard() {
        assert_eq!(osc52("hi"), "\x1b]52;c;aGk=\x07");
    }
}
