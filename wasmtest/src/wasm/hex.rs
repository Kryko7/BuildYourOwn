//! Hex dumps with the interesting bytes marked, the same shape `kafkatest` prints.

use std::fmt::Write as _;
use std::ops::Range;

/// A classic 16-byte-per-row hex dump, with a `^^` caret line under every marked byte.
pub fn hexdump(bytes: &[u8], marks: &[Range<usize>], max_bytes: usize) -> String {
    let shown = bytes.len().min(max_bytes);
    let mut out = String::new();
    for row in 0..shown.div_ceil(16) {
        let start = row * 16;
        let end = (start + 16).min(shown);
        let mut hex = String::new();
        let mut ascii = String::new();
        for (i, cell) in (start..start + 16).enumerate() {
            if i % 8 == 0 && i != 0 {
                hex.push(' ');
            }
            match bytes.get(cell).filter(|_| cell < end) {
                Some(&c) => {
                    let _ = write!(hex, "{c:02x} ");
                    ascii.push(if (0x20..0x7f).contains(&c) {
                        char::from(c)
                    } else {
                        '.'
                    });
                }
                None => {
                    hex.push_str("   ");
                    ascii.push(' ');
                }
            }
        }
        let _ = writeln!(out, "{start:04x}  {hex} |{ascii}|");
        let mut caret = String::new();
        let mut any = false;
        for i in start..end {
            if i % 8 == 0 && i != start {
                caret.push(' ');
            }
            if marks.iter().any(|m| m.contains(&i)) {
                caret.push_str("^^ ");
                any = true;
            } else {
                caret.push_str("   ");
            }
        }
        if any {
            let _ = writeln!(out, "      {caret}");
        }
    }
    if bytes.len() > shown {
        let _ = writeln!(out, "      … {} more bytes", bytes.len() - shown);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dump_shows_offsets_hex_and_ascii() {
        let text = hexdump(b"\0asm\x01\x00\x00\x00", &[], 64);
        assert!(text.contains("0000  00 61 73 6d"), "{text}");
        assert!(text.contains("|.asm"), "{text}");
    }

    #[test]
    fn marks_get_a_caret_line() {
        let text = hexdump(b"\0asm\x01\x00\x00\x00", &[0..4], 64);
        assert!(text.contains("^^ ^^ ^^ ^^"), "{text}");
    }

    #[test]
    fn a_long_dump_is_truncated_and_says_so() {
        let bytes = vec![0u8; 100];
        let text = hexdump(&bytes, &[], 32);
        assert!(text.contains("… 68 more bytes"), "{text}");
    }
}
