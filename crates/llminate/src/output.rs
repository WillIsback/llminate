//! Terminal output helpers: colour, JSON pretty-printing, secret masking.

use std::io::IsTerminal;

/// Colour decisions for one run.
#[derive(Clone, Copy)]
pub struct Style {
    color: bool,
}

impl Style {
    /// Colour on when stdout is a TTY and neither `--no-color` nor `NO_COLOR` is set.
    pub fn new(no_color: bool) -> Self {
        let tty = std::io::stdout().is_terminal();
        let env_no = std::env::var_os("NO_COLOR").is_some();
        Self {
            color: tty && !no_color && !env_no,
        }
    }

    /// Bold.
    pub fn bold(&self, s: &str) -> String {
        self.wrap("1", s)
    }

    fn wrap(&self, code: &str, s: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
}

/// Pretty-print JSON, falling back to the raw text on parse errors.
pub fn pretty_json(body: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(body) {
        Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_else(|_| body.to_string()),
        Err(_) => body.to_string(),
    }
}

/// Mask an API key, keeping only the last four characters.
pub fn mask_key(key: &str) -> String {
    let n = key.chars().count();
    if n == 0 {
        return "(absente)".to_string();
    }
    let tail: String = key.chars().skip(n.saturating_sub(4)).collect();
    format!("sk-…{tail}")
}
