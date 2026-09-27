//! Helpers for reading command input from files, stdin, and the terminal.

use std::{
    fs,
    io::{self, IsTerminal, Read},
    path::Path,
};

use anyhow::{Context, Result, bail};

/// Returns whether `path` selects stdin (`-`).
pub(crate) fn is_stdin(path: Option<&Path>) -> bool {
    path.is_some_and(|path| path == Path::new("-"))
}

/// Reads UTF-8 text from `path`, or from stdin when `path` is `-`.
pub(crate) fn read_text_file(path: &Path, label: &str) -> Result<String> {
    if is_stdin(Some(path)) {
        let mut text = String::new();
        io::stdin()
            .read_to_string(&mut text)
            .with_context(|| format!("could not read {label} from stdin"))?;
        Ok(text)
    } else {
        fs::read_to_string(path)
            .with_context(|| format!("could not read {label} from {}", path.display()))
    }
}

/// Reads a secret from `path` (`-` for stdin), dropping one trailing line ending.
pub(crate) fn read_secret_file(path: &Path, label: &str) -> Result<String> {
    let text = read_text_file(path, label)?;
    let secret = strip_line_ending(&text);
    if secret.is_empty() {
        bail!("{label} is empty");
    }
    Ok(secret.to_owned())
}

/// Prompts for a secret on the terminal without echoing it.
pub(crate) fn prompt_secret(prompt: &str, label: &str, alternative: &str) -> Result<String> {
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        bail!("reading the {label} requires a terminal; use {alternative}");
    }
    let secret = rpassword::prompt_password(prompt)
        .with_context(|| format!("could not read the {label} from the terminal"))?;
    if secret.is_empty() {
        bail!("{label} is empty");
    }
    Ok(secret)
}

fn strip_line_ending(text: &str) -> &str {
    let text = text.strip_suffix('\n').unwrap_or(text);
    text.strip_suffix('\r').unwrap_or(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_line_ending_removes_only_one_trailing_newline() {
        assert_eq!(strip_line_ending("secret \r\n\n"), "secret \r\n");
        assert_eq!(strip_line_ending("secret\r\n"), "secret");
        assert_eq!(strip_line_ending(" secret "), " secret ");
    }

    #[test]
    fn dash_selects_stdin() {
        assert!(is_stdin(Some(Path::new("-"))));
        assert!(!is_stdin(Some(Path::new("./-x"))));
        assert!(!is_stdin(None));
    }
}
