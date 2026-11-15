//! Pane runtime: the PTY, the child process and the terminal emulator fed by it.

use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use anyhow::Context;
use anyhow::Result;
use portable_pty::Child;
use portable_pty::CommandBuilder;
use portable_pty::MasterPty;
use portable_pty::PtySize;
use portable_pty::native_pty_system;

use crate::protocol::PaneId;

const SCROLLBACK_LINES: usize = 0;

pub type PtyWriter = Arc<Mutex<Box<dyn Write + Send>>>;

/// Emulated terminal state of one pane. Shared between the PTY reader thread
/// (which feeds it) and the server (which reads snapshots from it).
pub struct Terminal {
    /// Emulator state; locked briefly for every write and read.
    parser: Mutex<vt100::Parser<TerminalCallbacks>>,

    /// Bumped after every chunk of output; observers compare it to detect change.
    output_seq: AtomicU64,
}

/// Captures what vt100 doesn't track itself: the window title, and replies to
/// terminal queries that programs block on at startup.
#[derive(Default)]
struct TerminalCallbacks {
    title: String,
    replies: Vec<u8>,
}

impl vt100::Callbacks for TerminalCallbacks {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = String::from_utf8_lossy(title).into_owned();
    }

    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        _i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        let first_param = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        match (i1, c, first_param) {
            // Device status report: cursor position (1-based).
            (None, 'n', 6) => {
                let (row, col) = screen.cursor_position();
                self.replies
                    .extend(format!("\x1b[{};{}R", row + 1, col + 1).into_bytes());
            }
            // Device status report: terminal OK.
            (None, 'n', 5) => self.replies.extend(b"\x1b[0n"),
            // Primary device attributes: claim a VT220-class terminal.
            (None, 'c', 0) => self.replies.extend(b"\x1b[?62;22c"),
            _ => {}
        }
    }
}

impl Terminal {
    fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: Mutex::new(vt100::Parser::new_with_callbacks(
                rows,
                cols,
                SCROLLBACK_LINES,
                TerminalCallbacks::default(),
            )),
            output_seq: AtomicU64::new(0),
        }
    }

    /// Feeds PTY output in and returns any replies owed to the program.
    fn process(&self, bytes: &[u8]) -> Vec<u8> {
        let mut parser = self.parser.lock().unwrap();
        parser.process(bytes);
        let replies = std::mem::take(&mut parser.callbacks_mut().replies);
        drop(parser);
        self.output_seq.fetch_add(1, Ordering::Release);
        replies
    }

    pub fn output_seq(&self) -> u64 {
        self.output_seq.load(Ordering::Acquire)
    }

    pub fn text(&self) -> String {
        self.parser.lock().unwrap().screen().contents()
    }

    /// Bytes that recreate the full screen (contents, cursor, input modes) on
    /// a blank terminal of the same size.
    pub fn ansi(&self) -> Vec<u8> {
        self.parser.lock().unwrap().screen().state_formatted()
    }

    /// Window title and visible rows, as consumed by detection.
    pub fn detection_snapshot(&self) -> (String, Vec<String>) {
        let parser = self.parser.lock().unwrap();
        let screen = parser.screen();
        let (_, cols) = screen.size();
        let lines = screen
            .rows(0, cols)
            .map(|row| row.trim_end().to_owned())
            .collect();
        (parser.callbacks().title.clone(), lines)
    }

    pub fn application_cursor(&self) -> bool {
        self.parser.lock().unwrap().screen().application_cursor()
    }

    fn resize(&self, rows: u16, cols: u16) {
        self.parser
            .lock()
            .unwrap()
            .screen_mut()
            .set_size(rows, cols);
        self.output_seq.fetch_add(1, Ordering::Release);
    }
}

pub struct SpawnSpec<'a> {
    pub id: PaneId,
    pub command: Option<&'a str>,
    pub cwd: &'a Path,
    pub rows: u16,
    pub cols: u16,
    pub socket: PathBuf,
}

pub struct PaneRuntime {
    pub terminal: Arc<Terminal>,
    writer: PtyWriter,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
}

impl PaneRuntime {
    /// Starts `command` (or the user's shell) in a new PTY. A command runs via
    /// `$SHELL -c` so it gets the same PATH and quoting rules as typing it.
    pub fn spawn(spec: SpawnSpec) -> Result<Self> {
        let pty = native_pty_system()
            .openpty(PtySize {
                rows: spec.rows,
                cols: spec.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("failed to open pty")?;

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let mut command = CommandBuilder::new(&shell);
        if let Some(line) = spec.command {
            command.args(["-c", line]);
        }
        command.cwd(spec.cwd);
        command.env("TERM", "xterm-256color");
        command.env("ATERM_PANE", spec.id.to_string());
        command.env("ATERM_SOCKET", &spec.socket);

        let child = pty
            .slave
            .spawn_command(command)
            .context("failed to spawn process")?;
        drop(pty.slave);

        let reader = pty.master.try_clone_reader()?;
        let writer: PtyWriter = Arc::new(Mutex::new(pty.master.take_writer()?));
        let terminal = Arc::new(Terminal::new(spec.rows, spec.cols));

        let thread_terminal = Arc::clone(&terminal);
        let thread_writer = Arc::clone(&writer);
        std::thread::Builder::new()
            .name(format!("pty-reader-{}", spec.id))
            .spawn(move || pump_output(reader, &thread_terminal, &thread_writer))?;

        Ok(Self {
            terminal,
            writer,
            master: pty.master,
            child,
        })
    }

    pub fn writer(&self) -> PtyWriter {
        Arc::clone(&self.writer)
    }

    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        self.terminal.resize(rows, cols);
        Ok(())
    }

    /// Process group currently in the foreground of the PTY.
    pub fn foreground_pgid(&self) -> Option<i32> {
        self.master.process_group_leader()
    }

    /// Exit code once the child has exited, without blocking.
    pub fn try_exit_code(&mut self) -> Option<u32> {
        self.child
            .try_wait()
            .ok()
            .flatten()
            .map(|status| status.exit_code())
    }
}

impl Drop for PaneRuntime {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn pump_output(mut reader: Box<dyn Read + Send>, terminal: &Terminal, writer: &PtyWriter) {
    let mut buf = [0u8; 16 * 1024];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let replies = terminal.process(&buf[..n]);
                if !replies.is_empty() {
                    let mut writer = writer.lock().unwrap();
                    let _ = writer.write_all(&replies).and_then(|_| writer.flush());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Terminal;

    #[test]
    fn answers_cursor_position_queries() {
        let terminal = Terminal::new(10, 40);
        let replies = terminal.process(b"\x1b[3;5Hhi\x1b[6n");
        assert_eq!(replies, b"\x1b[3;7R");
        assert!(terminal.text().contains("hi"));
    }

    #[test]
    fn tracks_title_and_trims_rows() {
        let terminal = Terminal::new(3, 20);
        terminal.process(b"\x1b]2;working\x07hello   ");
        let (title, lines) = terminal.detection_snapshot();
        assert_eq!(title, "working");
        assert_eq!(lines, vec!["hello", "", ""]);
    }
}
