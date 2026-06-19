//! Host-side input loop: capture local input, detect edge crossings, and relay
//! events to the controlled peer over the encrypted UDP realtime channel.
//!
//! Runs on a dedicated OS thread because the capturer blocks. Outgoing control
//! messages (edge enter/leave) are bridged to the async session via a tokio mpsc.

use crate::net::NetEvent;
use screenlink_core::protocol::{ControlMsg, InputEvent, Key, ScreenEdge};
use screenlink_core::realtime::RealtimeCrypto;
use screenlink_input::{CapturedEvent, Capturer, ControlSite, EdgeDetector};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, info};

/// Controls the running host input loop from the session/UI.
pub struct HostInputHandle {
    pub stop: Arc<AtomicBool>,
    pub snap_home: Arc<AtomicBool>,
    pub edge: Arc<Mutex<ScreenEdge>>,
}

/// Tracks modifier state to detect the global snap-home chord (Ctrl+Alt+Home),
/// using the portable [`Key`] model so it works regardless of OS. Pure + testable.
#[derive(Default)]
pub struct HotkeyDetector {
    ctrl: bool,
    alt: bool,
}

impl HotkeyDetector {
    /// Feed a key event; returns `true` when Ctrl+Alt+Home has just been pressed.
    pub fn on_key(&mut self, key: Key, pressed: bool) -> bool {
        match key {
            Key::ControlLeft | Key::ControlRight => {
                self.ctrl = pressed;
                false
            }
            Key::AltLeft | Key::AltRight => {
                self.alt = pressed;
                false
            }
            Key::Home => pressed && self.ctrl && self.alt,
            _ => false,
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn spawn(
    capturer: Box<dyn Capturer>,
    edge: ScreenEdge,
    hysteresis: i32,
    desktop: screenlink_input::Rect,
    udp: std::net::UdpSocket,
    mut rt: RealtimeCrypto,
    ctrl_tx: mpsc::Sender<ControlMsg>,
    events: mpsc::UnboundedSender<NetEvent>,
) -> HostInputHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let snap_home = Arc::new(AtomicBool::new(false));
    let edge_shared = Arc::new(Mutex::new(edge));

    let handle = HostInputHandle {
        stop: stop.clone(),
        snap_home: snap_home.clone(),
        edge: edge_shared.clone(),
    };

    std::thread::Builder::new()
        .name("screenlink-host-input".into())
        .spawn(move || {
            let mut det = EdgeDetector::new(edge, hysteresis);
            let mut hotkey = HotkeyDetector::default();
            let send_rt = |rt: &mut RealtimeCrypto, ev: InputEvent| {
                if let Ok(payload) = postcard::to_stdvec(&ev) {
                    if let Ok(pkt) = rt.seal(&payload) {
                        let _ = udp.send(&pkt);
                    }
                }
            };

            info!("host input loop started (edge {edge:?})");
            while !stop.load(Ordering::Relaxed) {
                // Keep the detector's edge in sync with UI changes.
                det.set_edge(*edge_shared.lock().unwrap());

                if snap_home.swap(false, Ordering::Relaxed) && det.force_home().is_some() {
                    capturer.set_suppress(false);
                    let _ = ctrl_tx.blocking_send(ControlMsg::EdgeLeave);
                    let _ = events.send(NetEvent::Status("Control snapped home".into()));
                }

                let Some(ev) = capturer.poll(Duration::from_millis(50)) else {
                    continue;
                };

                match ev {
                    CapturedEvent::Move {
                        dx,
                        dy,
                        abs_x,
                        abs_y,
                    } => match det.site() {
                        ControlSite::Local => {
                            if let Some(screenlink_input::Transition::ToRemote { entry_norm }) =
                                det.update_local(abs_x, abs_y, desktop)
                            {
                                capturer.set_suppress(true);
                                capturer.park_cursor(abs_x, abs_y);
                                let (ex, ey) =
                                    entry_point(*edge_shared.lock().unwrap(), entry_norm);
                                let _ =
                                    ctrl_tx.blocking_send(ControlMsg::EdgeEnter { x: ex, y: ey });
                                let _ = events.send(NetEvent::ControlOnRemote(true));
                                debug!("crossed to remote at ({ex:.2},{ey:.2})");
                            }
                        }
                        ControlSite::Remote => {
                            send_rt(&mut rt, InputEvent::MouseMove { dx, dy });
                            if det.update_remote(dx, dy).is_some() {
                                capturer.set_suppress(false);
                                let _ = ctrl_tx.blocking_send(ControlMsg::EdgeLeave);
                                let _ = events.send(NetEvent::ControlOnRemote(false));
                                debug!("returned to local");
                            }
                        }
                    },
                    CapturedEvent::Input(ie) => {
                        // Snap-home chord is handled locally and never relayed.
                        if let InputEvent::Key { key, pressed } = &ie {
                            if hotkey.on_key(*key, *pressed) {
                                snap_home.store(true, Ordering::Relaxed);
                                continue;
                            }
                        }
                        if det.site() == ControlSite::Remote {
                            send_rt(&mut rt, ie);
                        }
                    }
                }
            }
            // Make sure we never leave local input suppressed on exit.
            capturer.set_suppress(false);
            info!("host input loop stopped");
        })
        .expect("spawn host input thread");

    handle
}

/// Map a normalized position along the shared edge to the entry point on the
/// remote desktop (normalized 0..1, 0..1).
fn entry_point(edge: ScreenEdge, along: f32) -> (f32, f32) {
    match edge {
        // Crossing the host's right edge enters the remote from its left side.
        ScreenEdge::Right => (0.0, along),
        ScreenEdge::Left => (1.0, along),
        ScreenEdge::Bottom => (along, 0.0),
        ScreenEdge::Top => (along, 1.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_home_chord_requires_all_three() {
        let mut h = HotkeyDetector::default();
        // Home alone: nothing.
        assert!(!h.on_key(Key::Home, true));
        // Ctrl down, Home: still nothing (no Alt).
        h.on_key(Key::ControlLeft, true);
        assert!(!h.on_key(Key::Home, true));
        // Add Alt → Home fires.
        h.on_key(Key::AltLeft, true);
        assert!(h.on_key(Key::Home, true));
        // Releasing Ctrl breaks the chord.
        h.on_key(Key::ControlLeft, false);
        assert!(!h.on_key(Key::Home, true));
    }

    #[test]
    fn chord_only_fires_on_press_not_release() {
        let mut h = HotkeyDetector::default();
        h.on_key(Key::ControlRight, true);
        h.on_key(Key::AltRight, true);
        assert!(h.on_key(Key::Home, true));
        assert!(!h.on_key(Key::Home, false));
    }
}
