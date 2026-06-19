//! Bidirectional text clipboard sync (Phase 1).
//!
//! The manager tracks the last text it saw or applied, so a value arriving from
//! the peer doesn't bounce straight back as a "local change" (echo loop). Images
//! and files are Phase 3.

use arboard::Clipboard;

pub struct ClipboardManager {
    cb: Clipboard,
    last_seen: Option<String>,
}

impl ClipboardManager {
    pub fn new() -> anyhow::Result<Self> {
        let cb = Clipboard::new()?;
        Ok(Self {
            cb,
            last_seen: None,
        })
    }

    /// Return `Some(text)` if the local clipboard text changed since we last saw
    /// or applied it; otherwise `None`. Non-text or empty clipboards are ignored.
    pub fn poll_local_change(&mut self) -> Option<String> {
        let text = self.cb.get_text().ok()?;
        if text.is_empty() {
            return None;
        }
        match &self.last_seen {
            Some(prev) if prev == &text => None,
            _ => {
                self.last_seen = Some(text.clone());
                Some(text)
            }
        }
    }

    /// Apply text received from the peer, suppressing the echo back.
    pub fn apply_remote(&mut self, text: &str) -> anyhow::Result<()> {
        self.last_seen = Some(text.to_string());
        self.cb.set_text(text.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Clipboard access needs a desktop session; this test is best-effort and
    // skips cleanly in headless CI where no clipboard is available.
    #[test]
    fn apply_then_poll_does_not_echo() {
        let Ok(mut m) = ClipboardManager::new() else {
            eprintln!("no clipboard available; skipping");
            return;
        };
        if m.apply_remote("screenlink-test-value").is_err() {
            eprintln!("clipboard write unavailable; skipping");
            return;
        }
        // Right after applying remote text, it must not be reported as a *local*
        // change (that would echo back to the peer forever).
        assert_eq!(m.poll_local_change(), None);
    }
}
