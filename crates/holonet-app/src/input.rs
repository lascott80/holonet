//! Turns GPUI keystrokes into the bytes a terminal sends (xterm conventions).
//! App shortcuts (Ctrl Shift ...) are handled before this and never reach it.

use gpui::Keystroke;

/// Bytes for a keystroke, or `None` if the key sends nothing.
/// `app_cursor` is DECCKM: arrows send `ESC O x` instead of `ESC [ x`.
pub fn encode(ks: &Keystroke, app_cursor: bool) -> Option<Vec<u8>> {
    let m = &ks.modifiers;
    // xterm modifier parameter: 1 + shift + 2*alt + 4*ctrl.
    let param = 1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.control as u8;

    let csi_letter = |letter: char| -> Vec<u8> {
        if param > 1 {
            format!("\x1b[1;{param}{letter}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{letter}").into_bytes()
        } else {
            format!("\x1b[{letter}").into_bytes()
        }
    };
    let csi_tilde = |code: u8| -> Vec<u8> {
        if param > 1 { format!("\x1b[{code};{param}~") } else { format!("\x1b[{code}~") }.into_bytes()
    };
    let ss3_f = |letter: char| -> Vec<u8> {
        if param > 1 { format!("\x1b[1;{param}{letter}") } else { format!("\x1bO{letter}") }.into_bytes()
    };
    let alt_prefixed = |bytes: Vec<u8>| -> Vec<u8> {
        if m.alt { [b"\x1b".as_slice(), &bytes].concat() } else { bytes }
    };

    let bytes = match ks.key.as_str() {
        "enter" => alt_prefixed(b"\r".to_vec()),
        "backspace" => alt_prefixed(if m.control { vec![0x08] } else { vec![0x7f] }),
        "tab" => if m.shift { b"\x1b[Z".to_vec() } else { b"\t".to_vec() },
        "escape" => alt_prefixed(b"\x1b".to_vec()),
        "space" if m.control => vec![0],
        "up" => csi_letter('A'),
        "down" => csi_letter('B'),
        "right" => csi_letter('C'),
        "left" => csi_letter('D'),
        "home" => csi_letter('H'),
        "end" => csi_letter('F'),
        "insert" => csi_tilde(2),
        "delete" => csi_tilde(3),
        "pageup" => csi_tilde(5),
        "pagedown" => csi_tilde(6),
        "f1" => ss3_f('P'),
        "f2" => ss3_f('Q'),
        "f3" => ss3_f('R'),
        "f4" => ss3_f('S'),
        "f5" => csi_tilde(15),
        "f6" => csi_tilde(17),
        "f7" => csi_tilde(18),
        "f8" => csi_tilde(19),
        "f9" => csi_tilde(20),
        "f10" => csi_tilde(21),
        "f11" => csi_tilde(23),
        "f12" => csi_tilde(24),
        key => {
            // Ctrl+Alt with a typed character is AltGr on Windows layouts: send the character.
            let altgr = m.control && m.alt && ks.key_char.as_deref().is_some_and(|c| c != key);
            if m.control && !altgr {
                let ch = key.chars().next().filter(|_| key.chars().count() == 1)?;
                let code = match ch.to_ascii_lowercase() {
                    c @ 'a'..='z' => c as u8 - b'a' + 1,
                    '@' | '2' => 0,
                    '[' | '3' => 0x1b,
                    '\\' | '4' => 0x1c,
                    ']' | '5' => 0x1d,
                    '^' | '6' => 0x1e,
                    '_' | '-' | '7' => 0x1f,
                    '8' | '?' => 0x7f,
                    _ => return None,
                };
                alt_prefixed(vec![code])
            } else {
                let text = ks.key_char.clone().or_else(|| (key == "space").then(|| " ".into()))?;
                if m.alt && !altgr { alt_prefixed(text.into_bytes()) } else { text.into_bytes() }
            }
        }
    };
    Some(bytes)
}

/// Text to send for a paste, wrapped when the shell asked for bracketed paste.
pub fn paste(text: &str, bracketed: bool) -> Vec<u8> {
    // Terminals send carriage returns for line breaks.
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed {
        // Strip any end marker inside the paste so it cannot break out.
        let text = text.replace("\x1b[201~", "");
        format!("\x1b[200~{text}\x1b[201~").into_bytes()
    } else {
        text.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(spec: &str, key_char: Option<&str>) -> Keystroke {
        let mut ks = Keystroke::parse(spec).unwrap();
        ks.key_char = key_char.map(Into::into);
        ks
    }

    #[test]
    fn printable_and_control() {
        assert_eq!(encode(&key("a", Some("a")), false), Some(b"a".to_vec()));
        assert_eq!(encode(&key("shift-a", Some("A")), false), Some(b"A".to_vec()));
        assert_eq!(encode(&key("ctrl-c", None), false), Some(vec![3]));
        assert_eq!(encode(&key("ctrl-d", None), false), Some(vec![4]));
        assert_eq!(encode(&key("ctrl-[", None), false), Some(vec![0x1b]));
        assert_eq!(encode(&key("alt-b", Some("b")), false), Some(b"\x1bb".to_vec()));
        assert_eq!(encode(&key("enter", None), false), Some(b"\r".to_vec()));
        assert_eq!(encode(&key("backspace", None), false), Some(vec![0x7f]));
        assert_eq!(encode(&key("space", Some(" ")), false), Some(b" ".to_vec()));
    }

    #[test]
    fn altgr_sends_the_character() {
        // e.g. AltGr+Q on a German layout types '@'.
        assert_eq!(encode(&key("ctrl-alt-q", Some("@")), false), Some(b"@".to_vec()));
    }

    #[test]
    fn arrows_follow_cursor_mode_and_modifiers() {
        assert_eq!(encode(&key("up", None), false), Some(b"\x1b[A".to_vec()));
        assert_eq!(encode(&key("up", None), true), Some(b"\x1bOA".to_vec()));
        assert_eq!(encode(&key("ctrl-right", None), false), Some(b"\x1b[1;5C".to_vec()));
        assert_eq!(encode(&key("pageup", None), false), Some(b"\x1b[5~".to_vec()));
        assert_eq!(encode(&key("f5", None), false), Some(b"\x1b[15~".to_vec()));
    }

    #[test]
    fn paste_is_bracketed_and_safe() {
        assert_eq!(paste("a\nb", false), b"a\rb".to_vec());
        assert_eq!(paste("x\x1b[201~y", true), b"\x1b[200~xy\x1b[201~".to_vec());
    }
}
