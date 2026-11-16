//! Rewrites HVP (`CSI row;col f`) into the equivalent CUP (`CSI row;col H`).
//!
//! vt100 doesn't implement HVP, but full-screen programs such as btop position
//! every draw with it, so without this their output piles up wherever the
//! cursor happened to be.

const ESC: u8 = 0x1b;

/// CSI sequences longer than this are passed through untouched rather than
/// buffered without bound.
const MAX_CSI_LEN: usize = 64;

/// Streaming rewriter; keeps a sequence split across reads intact.
#[derive(Default)]
pub struct HvpRewriter {
    state: State,
}

#[derive(Default)]
enum State {
    #[default]
    Ground,

    /// Saw ESC; a following `[` starts a CSI sequence.
    Escape,

    /// Inside a CSI sequence; holds the bytes after `ESC [` seen so far.
    Csi(Vec<u8>),
}

impl HvpRewriter {
    /// Returns `input` with every complete HVP rewritten. Bytes of a sequence
    /// still incomplete at the end of `input` are held back for the next call.
    pub fn rewrite(&mut self, input: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(input.len());
        for &byte in input {
            self.state = match std::mem::take(&mut self.state) {
                State::Ground if byte == ESC => State::Escape,
                State::Ground => {
                    out.push(byte);
                    State::Ground
                }
                State::Escape if byte == b'[' => State::Csi(Vec::new()),
                State::Escape if byte == ESC => {
                    out.push(ESC);
                    State::Escape
                }
                State::Escape => {
                    out.extend([ESC, byte]);
                    State::Ground
                }
                State::Csi(mut body) => match byte {
                    // Parameter and intermediate bytes.
                    0x20..=0x3f if body.len() < MAX_CSI_LEN => {
                        body.push(byte);
                        State::Csi(body)
                    }
                    // Final byte.
                    0x40..=0x7e => {
                        let is_hvp =
                            byte == b'f' && body.iter().all(|b| b.is_ascii_digit() || *b == b';');
                        out.extend([ESC, b'[']);
                        out.extend(&body);
                        out.push(if is_hvp { b'H' } else { byte });
                        State::Ground
                    }
                    // Anything else ends our interest; the parser sees it as sent.
                    _ => {
                        out.extend([ESC, b'[']);
                        out.extend(&body);
                        if byte == ESC {
                            State::Escape
                        } else {
                            out.push(byte);
                            State::Ground
                        }
                    }
                },
            };
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::HvpRewriter;

    #[test]
    fn rewrites_hvp_and_leaves_everything_else() {
        let mut rewriter = HvpRewriter::default();
        assert_eq!(
            rewriter.rewrite(b"a\x1b[3;5fb\x1b[f\x1b[1;31mc\x1b[?25l\x1b7"),
            b"a\x1b[3;5Hb\x1b[H\x1b[1;31mc\x1b[?25l\x1b7"
        );
        // Private or intermediate forms aren't HVP.
        assert_eq!(rewriter.rewrite(b"\x1b[?1f\x1b[1 f"), b"\x1b[?1f\x1b[1 f");
    }

    #[test]
    fn keeps_sequences_split_across_reads() {
        let mut rewriter = HvpRewriter::default();
        let mut out = rewriter.rewrite(b"x\x1b");
        out.extend(rewriter.rewrite(b"[12;"));
        out.extend(rewriter.rewrite(b"4fy"));
        assert_eq!(out, b"x\x1b[12;4Hy");
    }

    #[test]
    fn passes_through_interrupted_and_overlong_sequences() {
        let mut rewriter = HvpRewriter::default();
        assert_eq!(rewriter.rewrite(b"\x1b[1\x1b[2;2f"), b"\x1b[1\x1b[2;2H");

        let long = [b"\x1b[".as_slice(), &[b'1'; 70], b"f"].concat();
        assert_eq!(rewriter.rewrite(&long), long);
    }
}
