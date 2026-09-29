use std::fs::OpenOptions;
use std::io::Write;

use anyhow::{Context, Result};
use base64::Engine;

/// Copies to the system clipboard. Over SSH (a Linux devbox, say) the machine's
/// clipboard is not the one the user sees, so it asks the local terminal to
/// copy instead with an OSC 52 escape; the same happens when there is no
/// clipboard to open, as on a headless box.
pub fn copy(text: &str) -> Result<()> {
    if over_ssh() {
        return copy_osc52(text);
    }
    match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.set_text(text.to_string()))
    {
        Ok(()) => Ok(()),
        Err(error) => copy_osc52(text).with_context(|| format!("failed to write clipboard: {error}")),
    }
}

fn over_ssh() -> bool {
    std::env::var_os("SSH_TTY").is_some() || std::env::var_os("SSH_CONNECTION").is_some()
}

/// Written to the controlling terminal, not stdout, so it never ends up in
/// piped output. Inside tmux this needs `set -g set-clipboard on`.
fn copy_osc52(text: &str) -> Result<()> {
    let mut tty = OpenOptions::new()
        .write(true)
        .open("/dev/tty")
        .context("no terminal to copy through (OSC 52)")?;
    tty.write_all(osc52(text).as_bytes())?;
    tty.flush()?;
    Ok(())
}

fn osc52(text: &str) -> String {
    format!(
        "\x1b]52;c;{}\x07",
        base64::engine::general_purpose::STANDARD.encode(text)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc52_wraps_base64_of_the_text() {
        assert_eq!(osc52("claude --resume a1"), "\x1b]52;c;Y2xhdWRlIC0tcmVzdW1lIGEx\x07");
    }
}
