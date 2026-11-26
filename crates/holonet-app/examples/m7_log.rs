//! Milestone 7 spike (c): a session log writer. Strips escape sequences from
//! shell output to plain text, then encrypts it in chunks with
//! XChaCha20-Poly1305 and appends them to a file, so a crash loses at most
//! the chunk being written.
//!
//!   cargo run -p holonet-app --example m7_log --release -- <dir>

use std::io::Write;
use std::time::Instant;

use alacritty_terminal::vte::{Parser, Perform};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};

/// Plain text from terminal output: printed characters, line ends, and `\r`
/// and backspace overwriting within the current line (progress bars).
#[derive(Default)]
struct Plain {
    line: Vec<char>,
    col: usize,
    out: String,
}

impl Plain {
    fn end_line(&mut self) {
        let text: String = self.line.iter().collect();
        self.out.push_str(text.trim_end());
        self.out.push('\n');
        self.line.clear();
        self.col = 0;
    }
}

impl Perform for Plain {
    fn print(&mut self, c: char) {
        if self.col < self.line.len() {
            self.line[self.col] = c;
        } else {
            self.line.push(c);
        }
        self.col += 1;
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => self.end_line(),
            b'\r' => self.col = 0,
            0x08 => self.col = self.col.saturating_sub(1),
            b'\t' => {
                let next = (self.col / 8 + 1) * 8;
                while self.col < next {
                    self.print(' ');
                }
            }
            _ => {}
        }
    }
}

const CHUNK: usize = 64 * 1024;

fn bulk(bytes: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes + 256);
    let mut i = 0usize;
    while out.len() < bytes {
        let line = format!(
            "\x1b[2m12:00:{:02}\x1b[0m \x1b[32mINFO\x1b[0m request id=\x1b[35m{i}\x1b[0m path=/api/v1/items/{} status=200 took={}ms\r\n",
            i % 60,
            i % 997,
            i % 250
        );
        out.extend_from_slice(line.as_bytes());
        i += 1;
    }
    out
}

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("a folder for the log"));
    let data = bulk(10_000_000);
    let key = [7u8; 32];
    let cipher = XChaCha20Poly1305::new((&key).into());

    // Strip and encrypt as the app would: in 32 KB reads, a chunk per 64 KB.
    let path = dir.join("session.log");
    let mut file = std::fs::File::create(&path).unwrap();
    let mut parser = Parser::new();
    let mut plain = Plain::default();
    let (mut strip_t, mut seal_t, mut chunks) = (0f64, 0f64, 0usize);
    let mut plain_bytes = 0usize;
    let t_all = Instant::now();
    for read in data.chunks(32 * 1024) {
        let t = Instant::now();
        parser.advance(&mut plain, read);
        strip_t += t.elapsed().as_secs_f64();
        if plain.out.len() >= CHUNK {
            let t = Instant::now();
            let text = std::mem::take(&mut plain.out);
            plain_bytes += text.len();
            seal(&cipher, &mut file, chunks as u64, text.as_bytes());
            chunks += 1;
            seal_t += t.elapsed().as_secs_f64();
        }
    }
    let text = std::mem::take(&mut plain.out);
    plain_bytes += text.len();
    seal(&cipher, &mut file, chunks as u64, text.as_bytes());
    chunks += 1;
    file.sync_all().unwrap();
    let all = t_all.elapsed().as_secs_f64();
    let size = std::fs::metadata(&path).unwrap().len();
    println!(
        "10.0 MB of output -> {:.1} MB plain in {chunks} chunks, {:.1} MB on disk; strip {:.0} ms ({:.0} MB/s), seal+write {:.0} ms; all {:.0} ms ({:.0} MB/s)",
        plain_bytes as f64 / 1e6,
        size as f64 / 1e6,
        strip_t * 1e3,
        10.0 / strip_t,
        seal_t * 1e3,
        all * 1e3,
        10.0 / all
    );

    // Read it back, then cut the file mid-chunk as a crash would.
    let t = Instant::now();
    let back = open(&cipher, &std::fs::read(&path).unwrap());
    println!(
        "read back {} chunks, {:.1} MB, in {:.0} ms",
        back.0,
        back.1.len() as f64 / 1e6,
        t.elapsed().as_secs_f64() * 1e3
    );
    assert_eq!(back.1.lines().nth(2).unwrap(), "12:00:02 INFO request id=2 path=/api/v1/items/2 status=200 took=2ms");
    let bytes = std::fs::read(&path).unwrap();
    let torn = open(&cipher, &bytes[..bytes.len() - 100]);
    println!(
        "torn file: {} of {chunks} chunks readable, {}",
        torn.0,
        if torn.2 { "last one dropped" } else { "nothing dropped" }
    );

    // Overwrites: a progress bar redrawn with \r, a backspace, and a tab.
    let mut p = Plain::default();
    Parser::new().advance(&mut p, b"Copying  10%\rCopying  55%\rCopying 100%\r\nab\x08c\tx\r\n");
    print!("progress sample: {:?}", p.out);
    println!();
    std::fs::remove_file(&path).unwrap();
}

/// One chunk: length (u32 LE), the 24-byte nonce, then ciphertext and tag.
/// The chunk number is the associated data, so chunks cannot be reordered.
fn seal(cipher: &XChaCha20Poly1305, file: &mut std::fs::File, n: u64, text: &[u8]) {
    let mut nonce = [0u8; 24];
    getrandom::fill(&mut nonce).unwrap();
    let ct = cipher
        .encrypt(&XNonce::from(nonce), chacha20poly1305::aead::Payload { msg: text, aad: &n.to_le_bytes() })
        .unwrap();
    let mut rec = Vec::with_capacity(28 + ct.len());
    rec.extend_from_slice(&(ct.len() as u32).to_le_bytes());
    rec.extend_from_slice(&nonce);
    rec.extend_from_slice(&ct);
    file.write_all(&rec).unwrap();
}

/// Chunks read, the text, and whether a torn last chunk was dropped.
fn open(cipher: &XChaCha20Poly1305, mut bytes: &[u8]) -> (usize, String, bool) {
    let mut text = String::new();
    let mut n = 0u64;
    while bytes.len() >= 28 {
        let len = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
        if bytes.len() < 28 + len {
            return (n as usize, text, true);
        }
        let nonce = &XNonce::try_from(&bytes[4..28]).unwrap();
        let pt = cipher
            .decrypt(nonce, chacha20poly1305::aead::Payload { msg: &bytes[28..28 + len], aad: &n.to_le_bytes() })
            .expect("chunk authenticates");
        text.push_str(std::str::from_utf8(&pt).unwrap());
        bytes = &bytes[28 + len..];
        n += 1;
    }
    (n as usize, text, !bytes.is_empty())
}
