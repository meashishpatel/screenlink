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
            // Virtual cursor position on the remote, normalized 0..1. We relay
            // *absolute* position (not deltas) so a lost/reordered UDP packet
            // can't permanently corrupt the cursor — the latest position wins.
            let mut vrx = 0.5f32;
            let mut vry = 0.5f32;
            let inv_w = 1.0 / desktop.w.max(1) as f32;
            let inv_h = 1.0 / desktop.h.max(1) as f32;
            // What the user is physically holding (modifier keys), tracked
            // regardless of which side has control. Used to seed the remote's
            // modifier state when control crosses over.
            let mut held_mods_local: Vec<Key> = Vec::new();
            // What we've told the remote is pressed (and not yet released).
            // On transition back to local we release each one so a modifier
            // can't get stuck after the snap-home chord or an edge return —
            // that "stuck Ctrl/Alt" is what made the keyboard look like it
            // randomly stopped working.
            let mut held_remote: Vec<Key> = Vec::new();
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
                    for k in held_remote.drain(..) {
                        send_rt(&mut rt, InputEvent::Key { key: k, pressed: false });
                    }
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
                                let edge_now = *edge_shared.lock().unwrap();
                                capturer.set_suppress(true);
                                // Park *back from* the crossed edge so the local
                                // cursor has room to move in every direction; if
                                // we parked at the seam, info.pt couldn't move
                                // off-screen and a "push deeper" wouldn't
                                // produce a delta — only the user's back-into-
                                // desktop wobble would, so the remote cursor
                                // appeared to move opposite the user's intent.
                                let (px, py) =
                                    park_position(edge_now, abs_x, abs_y, desktop);
                                capturer.park_cursor(px, py);
                                let (ex, ey) = entry_point(edge_now, entry_norm);
                                vrx = ex;
                                vry = ey;
                                let _ =
                                    ctrl_tx.blocking_send(ControlMsg::EdgeEnter { x: ex, y: ey });
                                // Seed the remote's modifier state with anything
                                // the user is already holding, so Shift+A etc.
                                // type correctly the first key after crossing.
                                for k in &held_mods_local {
                                    if !held_remote.contains(k) {
                                        send_rt(
                                            &mut rt,
                                            InputEvent::Key { key: *k, pressed: true },
                                        );
                                        held_remote.push(*k);
                                    }
                                }
                                let _ = events.send(NetEvent::ControlOnRemote(true));
                                debug!("crossed to remote at ({ex:.2},{ey:.2})");
                            }
                        }
                        ControlSite::Remote => {
                            // Accumulate the delta into the virtual position and
                            // relay the absolute position.
                            vrx = (vrx + dx as f32 * inv_w).clamp(0.0, 1.0);
                            vry = (vry + dy as f32 * inv_h).clamp(0.0, 1.0);
                            send_rt(&mut rt, InputEvent::MouseMoveAbs { x: vrx, y: vry });
                            if det.update_remote(dx, dy).is_some() {
                                capturer.set_suppress(false);
                                for k in held_remote.drain(..) {
                                    send_rt(
                                        &mut rt,
                                        InputEvent::Key { key: k, pressed: false },
                                    );
                                }
                                let _ = ctrl_tx.blocking_send(ControlMsg::EdgeLeave);
                                let _ = events.send(NetEvent::ControlOnRemote(false));
                                debug!("returned to local");
                            }
                        }
                    },
                    CapturedEvent::Input(ie) => {
                        if let InputEvent::Key { key, pressed } = &ie {
                            // Always track physical modifier state, regardless
                            // of where control lives — so the next cross to
                            // remote can re-press whatever the user is holding.
                            if is_modifier(*key) {
                                if *pressed {
                                    if !held_mods_local.contains(key) {
                                        held_mods_local.push(*key);
                                    }
                                } else {
                                    held_mods_local.retain(|k| k != key);
                                }
                            }
                            // Snap-home chord is handled locally and never relayed.
                            if hotkey.on_key(*key, *pressed) {
                                snap_home.store(true, Ordering::Relaxed);
                                continue;
                            }
                        }
                        if det.site() == ControlSite::Remote {
                            // Mirror remote's view of the keyboard so we can
                            // release everything cleanly on the way home.
                            if let InputEvent::Key { key, pressed } = &ie {
                                if *pressed {
                                    if !held_remote.contains(key) {
                                        held_remote.push(*key);
                                    }
                                } else {
                                    held_remote.retain(|k| k != key);
                                }
                            }
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

fn is_modifier(k: Key) -> bool {
    matches!(
        k,
        Key::ControlLeft
            | Key::ControlRight
            | Key::AltLeft
            | Key::AltRight
            | Key::ShiftLeft
            | Key::ShiftRight
            | Key::MetaLeft
            | Key::MetaRight
    )
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

/// Choose where on the local desktop to park the physical cursor while control
/// is on the remote: a point backed off from the crossed edge, clamped well
/// inside the desktop so the cursor has headroom to move in both directions
/// along every axis.
fn park_position(
    edge: ScreenEdge,
    abs_x: i32,
    abs_y: i32,
    desktop: screenlink_input::Rect,
) -> (i32, i32) {
    const BUFFER: i32 = 200;
    let (mut px, mut py) = match edge {
        ScreenEdge::Right => (abs_x - BUFFER, abs_y),
        ScreenEdge::Left => (abs_x + BUFFER, abs_y),
        ScreenEdge::Bottom => (abs_x, abs_y - BUFFER),
        ScreenEdge::Top => (abs_x, abs_y + BUFFER),
    };
    let min_x = desktop.x + BUFFER;
    let max_x = desktop.x + desktop.w - 1 - BUFFER;
    let min_y = desktop.y + BUFFER;
    let max_y = desktop.y + desktop.h - 1 - BUFFER;
    if max_x > min_x {
        px = px.clamp(min_x, max_x);
    }
    if max_y > min_y {
        py = py.clamp(min_y, max_y);
    }
    (px, py)
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
