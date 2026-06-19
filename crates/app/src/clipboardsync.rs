//! Clipboard sync thread. Owns the `arboard` handle (which prefers a single
//! thread), polls for local changes to push to the peer, and applies remote text
//! without echoing it back.

use screenlink_core::protocol::{ClipboardData, ControlMsg};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::warn;

pub enum ClipCmd {
    ApplyRemote(String),
    Stop,
}

/// Spawn the clipboard sync thread. Returns a sender for applying remote text /
/// stopping. Local changes are pushed to `ctrl_tx` as `Clipboard` control msgs.
pub fn spawn(ctrl_tx: mpsc::Sender<ControlMsg>) -> std::sync::mpsc::Sender<ClipCmd> {
    let (tx, rx) = std::sync::mpsc::channel::<ClipCmd>();
    std::thread::Builder::new()
        .name("screenlink-clipboard".into())
        .spawn(move || {
            let mut mgr = match screenlink_clipboard::ClipboardManager::new() {
                Ok(m) => m,
                Err(e) => {
                    warn!("clipboard unavailable, sync disabled: {e}");
                    return;
                }
            };
            loop {
                // Apply any pending remote updates / stop.
                loop {
                    match rx.try_recv() {
                        Ok(ClipCmd::ApplyRemote(text)) => {
                            if let Err(e) = mgr.apply_remote(&text) {
                                warn!("clipboard apply failed: {e}");
                            }
                        }
                        Ok(ClipCmd::Stop) => return,
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
                    }
                }
                // Push local changes to the peer.
                if let Some(text) = mgr.poll_local_change() {
                    if ctrl_tx
                        .blocking_send(ControlMsg::Clipboard(ClipboardData::Text(text)))
                        .is_err()
                    {
                        return; // session gone
                    }
                }
                std::thread::sleep(Duration::from_millis(400));
            }
        })
        .expect("spawn clipboard thread");
    tx
}
