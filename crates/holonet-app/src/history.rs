//! Session logs while a session runs: shell output to plain text, written by
//! a thread of its own (`holonet-vault::logs` seals and stores it).
//!
//! The terminal hands every output batch to a [`LogTap`]; the thread strips
//! escape sequences and appends the text. It seals what it has every
//! [`FLUSH_EVERY`], so a crash loses at most that much, and the History
//! page can read an open session's log.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use alacritty_terminal::vte::{Params, Parser, Perform};
use holonet_vault::LogWriter;

/// How often an open log seals what it has.
pub const FLUSH_EVERY: Duration = Duration::from_secs(2);

/// Saved in place of a full-screen program's output (an editor, `top`):
/// its screen redraws make no sense as lines.
pub const FULL_SCREEN_NOTE: &str = "[A full-screen program ran here. Its screen is not saved.]";

enum Msg {
    Output(Vec<u8>),
    /// A line of Holonet's own, such as "Reconnected at 14:02".
    Note(String),
    Finish,
}

/// Where the terminal sends output for the log. Cheap to clone.
#[derive(Clone)]
pub struct LogTap(mpsc::Sender<Msg>);

impl LogTap {
    pub fn output(&self, bytes: &[u8]) {
        let _ = self.0.send(Msg::Output(bytes.to_vec()));
    }
}

/// An open session log, owned by its pane.
pub struct SessionLog {
    tx: mpsc::Sender<Msg>,
    bytes: Arc<AtomicU64>,
    thread: Option<std::thread::JoinHandle<Result<u64, String>>>,
}

impl SessionLog {
    pub fn start(writer: LogWriter) -> Self {
        let (tx, rx) = mpsc::channel();
        let bytes = Arc::new(AtomicU64::new(0));
        let counter = bytes.clone();
        let thread = std::thread::Builder::new()
            .name("holonet-log".into())
            .spawn(move || run(writer, rx, counter))
            .expect("could not start the log thread");
        Self { tx, bytes, thread: Some(thread) }
    }

    pub fn tap(&self) -> LogTap {
        LogTap(self.tx.clone())
    }

    pub fn note(&self, line: impl Into<String>) {
        let _ = self.tx.send(Msg::Note(line.into()));
    }

    /// Plain-text bytes written so far.
    pub fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }

    /// Seal the rest and close. The thread finishes on its own; join the
    /// handle (off the UI thread) for the final size or why writing failed.
    pub fn finish(mut self) -> std::thread::JoinHandle<Result<u64, String>> {
        let _ = self.tx.send(Msg::Finish);
        self.thread.take().expect("finished once")
    }
}

impl Drop for SessionLog {
    fn drop(&mut self) {
        // A pane dropped without finishing still seals what it has.
        let _ = self.tx.send(Msg::Finish);
    }
}

fn run(mut writer: LogWriter, rx: mpsc::Receiver<Msg>, bytes: Arc<AtomicU64>) -> Result<u64, String> {
    let mut parser = Parser::new();
    let mut plain = Plain::default();
    let mut dirty = false;
    let err = |e: holonet_vault::VaultError| e.to_string();
    loop {
        let msg = rx.recv_timeout(FLUSH_EVERY);
        match msg {
            Ok(Msg::Output(b)) => {
                parser.advance(&mut plain, &b);
                dirty = true;
            }
            Ok(Msg::Note(line)) => {
                plain.note(&line);
                dirty = true;
            }
            Ok(Msg::Finish) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                plain.end_partial();
                writer.append(&std::mem::take(&mut plain.out)).map_err(err)?;
                let total = writer.finish().map_err(err)?;
                bytes.store(total, Ordering::Relaxed);
                return Ok(total);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if dirty {
                    writer.append(&std::mem::take(&mut plain.out)).map_err(err)?;
                    writer.flush().map_err(err)?;
                    dirty = false;
                }
            }
        }
        if plain.out.len() >= holonet_vault::logs::CHUNK {
            writer.append(&std::mem::take(&mut plain.out)).map_err(err)?;
        }
        bytes.store(writer.bytes() + plain.out.len() as u64, Ordering::Relaxed);
    }
}

/// Plain text from terminal output: printed characters and line ends, with
/// `\r`, backspace, cursor left and right, and erase-line acting within the
/// current line, so a redrawn progress bar or an edited command line is
/// saved as it last looked. Full-screen programs (the alternate screen) are
/// left out.
#[derive(Default)]
pub struct Plain {
    line: Vec<char>,
    col: usize,
    alternate: bool,
    pub out: String,
}

impl Plain {
    fn end_line(&mut self) {
        let text: String = self.line.iter().collect();
        self.out.push_str(text.trim_end());
        self.out.push('\n');
        self.line.clear();
        self.col = 0;
    }

    /// End a line left unfinished (a prompt), if any.
    fn end_partial(&mut self) {
        if !self.line.is_empty() {
            self.end_line();
        }
    }

    fn note(&mut self, line: &str) {
        self.end_partial();
        self.out.push_str(line);
        self.out.push('\n');
    }
}

impl Perform for Plain {
    fn print(&mut self, c: char) {
        if self.alternate {
            return;
        }
        if self.col < self.line.len() {
            self.line[self.col] = c;
        } else {
            self.line.resize(self.col, ' ');
            self.line.push(c);
        }
        self.col += 1;
    }

    fn execute(&mut self, byte: u8) {
        if self.alternate {
            return;
        }
        match byte {
            b'\n' => self.end_line(),
            b'\r' => self.col = 0,
            0x08 => self.col = self.col.saturating_sub(1),
            b'\t' => self.col = (self.col / 8 + 1) * 8,
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        let first = params.iter().next().and_then(|p| p.first().copied()).unwrap_or(0);
        if intermediates == b"?" && matches!(action, 'h' | 'l') {
            let alt = params.iter().any(|p| matches!(p.first(), Some(47 | 1047 | 1049)));
            if alt && action == 'h' && !self.alternate {
                self.note(FULL_SCREEN_NOTE);
                self.alternate = true;
            } else if alt && action == 'l' {
                self.alternate = false;
                self.line.clear();
                self.col = 0;
            }
            return;
        }
        if self.alternate || !intermediates.is_empty() {
            return;
        }
        let n = usize::from(first.max(1));
        match action {
            'C' => self.col += n,
            'D' => self.col = self.col.saturating_sub(n),
            // Column, from 1.
            'G' => self.col = n - 1,
            'K' => match first {
                0 => self.line.truncate(self.col),
                1 => self.line.iter_mut().take(self.col + 1).for_each(|c| *c = ' '),
                _ => self.line.clear(),
            },
            // Delete characters at the cursor (line editing).
            'P' => {
                let end = (self.col + n).min(self.line.len());
                if self.col < end {
                    self.line.drain(self.col..end);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(bytes: &[u8]) -> String {
        let mut p = Plain::default();
        Parser::new().advance(&mut p, bytes);
        p.end_partial();
        p.out
    }

    #[test]
    fn colours_go_and_lines_stay() {
        assert_eq!(
            plain(b"\x1b[32mdeploy@prod\x1b[0m:~$ systemctl is-active nginx\r\n\x1b[1;32mactive\x1b[0m\r\n"),
            "deploy@prod:~$ systemctl is-active nginx\nactive\n"
        );
    }

    #[test]
    fn redraws_keep_their_last_state() {
        // A progress bar, erase-line, a backspace and a tab.
        assert_eq!(plain(b"Copying  10%\rCopying  55%\r\x1b[KCopying 100%\r\n"), "Copying 100%\n");
        assert_eq!(plain(b"ab\x08c\tx\r\n"), "ac      x\n");
        // Readline: cursor left twice, delete a character, then type.
        assert_eq!(plain(b"$ lss\x1b[D\x1b[P\r\n"), "$ ls\n");
        assert_eq!(plain(b"$ echo hi\x1b[2D\x1b[KHI\r\n"), "$ echo HI\n");
        // A window title (OSC) is not text.
        assert_eq!(plain(b"\x1b]0;deploy@prod: ~\x07$ \r\n"), "$\n");
    }

    #[test]
    fn full_screen_programs_are_left_out() {
        let out = plain(b"$ vim notes\r\n\x1b[?1049h\x1b[Hsome screen\r\n~\r\n\x1b[?1049l$ ls\r\na  b\r\n");
        assert_eq!(out, format!("$ vim notes\n{FULL_SCREEN_NOTE}\n$ ls\na  b\n"));
    }

    #[test]
    fn writer_thread_seals_and_reports() {
        let dir = tempfile::tempdir().unwrap();
        let key_vault = holonet_vault::UnlockedVault::create_with(
            &dir.path().join("v.holonet"),
            "correct horse",
            holonet_vault::KdfParams::insecure_for_tests(),
        )
        .unwrap();
        let logs = key_vault.logs();
        let id = holonet_model::RecordId::new();
        let log = SessionLog::start(logs.create(id).unwrap());
        let tap = log.tap();
        tap.output(b"\x1b[1mfirst\x1b[0m\r\nsecond, no newline");
        log.note("-- Reconnected at 14:02 --");
        tap.output(b"third\r\n");
        let total = log.finish().join().unwrap().unwrap();
        let text = logs.read(id).unwrap().text;
        assert_eq!(text, "first\nsecond, no newline\n-- Reconnected at 14:02 --\nthird\n");
        assert_eq!(total, text.len() as u64);
    }
}
