use anyhow::{Context, Result};

pub fn copy(text: &str) -> Result<()> {
    let mut clipboard = arboard::Clipboard::new().context("failed to open clipboard")?;
    clipboard
        .set_text(text.to_string())
        .context("failed to write clipboard")
}
