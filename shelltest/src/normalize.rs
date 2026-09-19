//! Output normalization applied before matching, plus `{TMP}`-style placeholders.

#[derive(Debug, Clone, Copy, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NormalizeOpts {
    pub strip_prompt: Option<bool>,
    pub strip_ansi: Option<bool>,
    pub strip_cr: Option<bool>,
    pub trim_lines: Option<bool>,
}

#[derive(Debug, Clone, Copy)]
pub struct Normalizer<'a> {
    pub prompt: &'a str,
    pub strip_prompt: bool,
    pub strip_ansi: bool,
    pub strip_cr: bool,
    pub trim_lines: bool,
}

impl<'a> Normalizer<'a> {
    /// Defaults for stdout/stderr captured through pipes.
    pub fn pipe(prompt: &'a str, o: &NormalizeOpts) -> Self {
        Normalizer {
            prompt,
            strip_prompt: o.strip_prompt.unwrap_or(true),
            strip_ansi: o.strip_ansi.unwrap_or(false),
            strip_cr: o.strip_cr.unwrap_or(false),
            trim_lines: o.trim_lines.unwrap_or(true),
        }
    }

    /// Defaults for the merged terminal stream of a pty session: the prompt and
    /// trailing spaces are kept because pty tests assert on them.
    pub fn pty(prompt: &'a str, o: &NormalizeOpts) -> Self {
        Normalizer {
            prompt,
            strip_prompt: o.strip_prompt.unwrap_or(false),
            strip_ansi: o.strip_ansi.unwrap_or(true),
            strip_cr: o.strip_cr.unwrap_or(true),
            trim_lines: o.trim_lines.unwrap_or(false),
        }
    }

    pub fn apply(&self, text: &str) -> String {
        let mut t = if self.strip_ansi {
            strip_ansi(text)
        } else {
            text.to_string()
        };
        if self.strip_cr {
            t.retain(|c| c != '\r');
        }
        let mut lines: Vec<String> = t
            .split('\n')
            .map(|line| {
                let mut l = line;
                if self.strip_prompt && !self.prompt.is_empty() {
                    while let Some(rest) = l.strip_prefix(self.prompt) {
                        l = rest;
                    }
                }
                if self.trim_lines {
                    l.trim_end().to_string()
                } else {
                    l.to_string()
                }
            })
            .collect();
        if self.trim_lines {
            while lines.len() > 1 && lines.last().is_some_and(|l| l.is_empty()) {
                lines.pop();
            }
        } else if lines.len() > 1 && lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        lines.join("\n")
    }
}

/// Normalization for expectation strings: only trailing whitespace and the final newline.
pub fn normalize_expected(text: &str) -> String {
    Normalizer {
        prompt: "",
        strip_prompt: false,
        strip_ansi: false,
        strip_cr: false,
        trim_lines: true,
    }
    .apply(text)
}

/// Remove ANSI/VT escape sequences (CSI, OSC, and two-byte ESC sequences). BEL alone is kept.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for n in chars.by_ref() {
                    if ('\x40'..='\x7e').contains(&n) {
                        break;
                    }
                }
            }
            Some(']') => {
                let mut prev = '\0';
                for n in chars.by_ref() {
                    if n == '\x07' || (prev == '\x1b' && n == '\\') {
                        break;
                    }
                    prev = n;
                }
            }
            Some('(') | Some(')') | Some('#') => {
                chars.next();
            }
            _ => {}
        }
    }
    out
}

#[derive(Debug, Clone, Default)]
pub struct Placeholders {
    pub pairs: Vec<(&'static str, String)>,
}

impl Placeholders {
    /// Replace `{TMP}`-style placeholders. A `$` directly before the brace marks shell
    /// syntax (`${HOME}`), which is left alone.
    pub fn apply(&self, s: &str) -> String {
        let mut out = s.to_string();
        for (k, v) in &self.pairs {
            let mut result = String::with_capacity(out.len());
            let mut rest = out.as_str();
            while let Some(i) = rest.find(*k) {
                let escaped = result.ends_with('$') && i == 0 || rest[..i].ends_with('$');
                result.push_str(&rest[..i]);
                result.push_str(if escaped { k } else { v });
                rest = &rest[i + k.len()..];
            }
            result.push_str(rest);
            out = result;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_prompt_and_whitespace_in_pipe_mode() {
        let n = Normalizer::pipe("$ ", &NormalizeOpts::default());
        assert_eq!(n.apply("$ hello world  \n$ $ \n$ "), "hello world");
        assert_eq!(n.apply("hi\n"), "hi");
        assert_eq!(n.apply("\n\n"), "");
        assert_eq!(n.apply("a\n\nb\n"), "a\n\nb");
        assert_eq!(n.apply(""), "");
    }

    #[test]
    fn pty_mode_keeps_prompt_and_drops_ansi() {
        let n = Normalizer::pty("$ ", &NormalizeOpts::default());
        assert_eq!(
            n.apply("\x1b[?2004h$ echo hi\r\nhi\r\n$ \x07"),
            "$ echo hi\nhi\n$ \x07"
        );
    }

    #[test]
    fn ansi_variants() {
        assert_eq!(strip_ansi("a\x1b[31mb\x1b[0mc"), "abc");
        assert_eq!(strip_ansi("\x1b]0;title\x07x"), "x");
        assert_eq!(strip_ansi("\x1b(Bq\x1b7"), "q");
        assert_eq!(strip_ansi("bell\x07"), "bell\x07");
    }

    #[test]
    fn placeholders() {
        let p = Placeholders {
            pairs: vec![("{TMP}", "/t".into()), ("{BIN}", "/t/bin".into())],
        };
        assert_eq!(p.apply("{BIN}:{TMP}/x"), "/t/bin:/t/x");
        assert_eq!(p.apply("${TMP} {TMP} x${TMP}"), "${TMP} /t x${TMP}");
        assert_eq!(normalize_expected("hello\n"), "hello");
    }
}
