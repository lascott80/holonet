//! Canned terminal output for spike 3: exercises colors, styles, wide
//! characters, Nerd Font glyphs and the escape sequences Ubuntu 26.04 sends.

/// Scrollback filler, then a screen of styled output ending at a prompt.
pub fn showcase() -> Vec<u8> {
    let mut s = String::new();
    for i in 1..=200 {
        s.push_str(&format!(
            "\x1b[2m2026-11-25 12:{:02}:{:02}\x1b[0m \x1b[32mINFO\x1b[0m worker[{}] processed batch \x1b[35m{}\x1b[0m\r\n",
            i / 60,
            i % 60,
            i % 4,
            1000 + i
        ));
    }

    let prompt = |cmd: &str| {
        format!(
            "\x1b[?2004h\x1b]3008;start=abc;type=shell;user=holotest\x1b\\\
             \x1b[38;2;92;200;255;1mholotest@sptogo\x1b[0m\x1b[38;2;138;138;138m:\x1b[0m\
             \x1b[38;2;207;207;207m~/src/holonet\x1b[0m \x1b[38;2;127;214;181m\u{E725} main\x1b[0m\
             \x1b[38;2;138;138;138m $\x1b[0m {cmd}\x1b[?2004l\r\n"
        )
    };

    s.push_str(&prompt("ls --color"));
    s.push_str(
        "\x1b[1;34m\u{F07B} crates\x1b[0m  \x1b[1;34m\u{F07B} docs\x1b[0m  \x1b[0m\u{E7A8} Cargo.toml\x1b[0m  \
         \x1b[32m\u{F489} build.sh\x1b[0m  \x1b[31m\u{F410} logs.tar.gz\x1b[0m  \x1b[36m\u{F481} link\x1b[0m\r\n",
    );

    s.push_str(&prompt("palette"));
    for row in 0..2 {
        for i in 0..8 {
            s.push_str(&format!("\x1b[48;5;{}m  {:>2}  ", row * 8 + i, row * 8 + i));
        }
        s.push_str("\x1b[0m\r\n");
    }
    for i in 16..88 {
        s.push_str(&format!("\x1b[48;5;{i}m "));
    }
    s.push_str("\x1b[0m\r\n");
    for i in 0..72 {
        let r = 255 * i / 71;
        s.push_str(&format!("\x1b[48;2;{};{};{}m ", r, 80, 255 - r));
    }
    s.push_str("\x1b[0m\r\n");

    s.push_str(&prompt("styles"));
    s.push_str(
        "\x1b[1mbold\x1b[0m \x1b[3mitalic\x1b[0m \x1b[1;3mbold italic\x1b[0m \x1b[4munderline\x1b[0m \
         \x1b[9mstrike\x1b[0m \x1b[2mdim\x1b[0m \x1b[7minverse\x1b[0m \x1b[31mred\x1b[1m bright\x1b[0m\r\n",
    );
    s.push_str("Wide: 日本語テキスト | 한국어 | emoji 🚀✨ | after\r\n");
    s.push_str("Ligatures: -> => != === <= >= :: |> <!-- -->\r\n");
    s.push_str("┌──────────┬────────┐\r\n│ host     │ status │\r\n├──────────┼────────┤\r\n");
    s.push_str("│ pi-garage│ \x1b[32m● up\x1b[0m   │\r\n│ nas      │ \x1b[33m● warn\x1b[0m │\r\n└──────────┴────────┘\r\n");
    s.push_str(&prompt(""));
    // Leave the cursor after the final prompt, without the trailing newline.
    while s.ends_with("\r\n") {
        s.truncate(s.len() - 2);
    }
    s.into_bytes()
}

/// About `bytes` of colored log output, for parse timing.
pub fn bulk(bytes: usize) -> Vec<u8> {
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
