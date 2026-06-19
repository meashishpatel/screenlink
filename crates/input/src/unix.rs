//! Linux + macOS input backend (client role) via the `enigo` crate.
//!
//! This makes a Unix machine able to be **controlled** (inject the host's input
//! locally). Capture (acting as a *host*) is not implemented here yet — see
//! `new_capturer` below — so for now a Unix device can be a client only.
//!
//! `enigo`'s platform handle (an X11 connection on Linux, a `CGEventSource` on
//! macOS) is not guaranteed `Send`, and our `Injector` must be `Send`. So we own
//! the `Enigo` on a dedicated thread and talk to it over a channel; the
//! `Injector` itself just holds the `Sender`.
//!
//! Key mapping is best-effort: letters/digits/punctuation are typed as
//! characters and common control keys are mapped to named keys. On macOS the user
//! must grant **Accessibility** permission for injection to take effect.

use crate::edge::Rect;
use crate::{CapturedEvent, Capturer, Injector};
use enigo::{Axis, Button, Coordinate, Direction, Enigo, Key as EKey, Keyboard, Mouse, Settings};
use screenlink_core::protocol::{InputEvent, Key, MouseButton};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender};
use std::sync::OnceLock;
use std::time::Duration;

enum InjectMsg {
    Event(InputEvent),
    CursorNorm(f32, f32),
}

/// Injects input on Linux/macOS by forwarding to an `Enigo` owned by a worker
/// thread.
pub struct EnigoInjector {
    tx: Sender<InjectMsg>,
    desktop: Rect,
}

impl EnigoInjector {
    pub fn new() -> anyhow::Result<Self> {
        let (tx, rx) = channel::<InjectMsg>();
        let (init_tx, init_rx) = channel::<anyhow::Result<(i32, i32)>>();

        std::thread::Builder::new()
            .name("screenlink-inject".into())
            .spawn(move || {
                let mut enigo = match Enigo::new(&Settings::default()) {
                    Ok(e) => e,
                    Err(e) => {
                        let _ = init_tx.send(Err(anyhow::anyhow!("enigo init failed: {e}")));
                        return;
                    }
                };
                let (mut w, mut h) = enigo.main_display().unwrap_or((1920, 1080));
                w = w.max(1);
                h = h.max(1);
                let _ = init_tx.send(Ok((w, h)));

                while let Ok(msg) = rx.recv() {
                    match msg {
                        InjectMsg::Event(ev) => apply_event(&mut enigo, ev, w, h),
                        InjectMsg::CursorNorm(x, y) => {
                            let px = (x.clamp(0.0, 1.0) * w as f32).round() as i32;
                            let py = (y.clamp(0.0, 1.0) * h as f32).round() as i32;
                            let _ = enigo.move_mouse(px, py, Coordinate::Abs);
                        }
                    }
                }
            })?;

        let (w, h) = init_rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| anyhow::anyhow!("injector thread did not start"))??;
        Ok(Self {
            tx,
            desktop: Rect::new(0, 0, w, h),
        })
    }
}

impl Injector for EnigoInjector {
    fn inject(&mut self, ev: InputEvent) -> anyhow::Result<()> {
        self.tx
            .send(InjectMsg::Event(ev))
            .map_err(|_| anyhow::anyhow!("injector thread is gone"))
    }
    fn desktop_rect(&self) -> Rect {
        self.desktop
    }
    fn set_cursor_norm(&mut self, x: f32, y: f32) -> anyhow::Result<()> {
        self.tx
            .send(InjectMsg::CursorNorm(x, y))
            .map_err(|_| anyhow::anyhow!("injector thread is gone"))
    }
}

fn apply_event(enigo: &mut Enigo, ev: InputEvent, w: i32, h: i32) {
    let r = match ev {
        InputEvent::MouseMove { dx, dy } => enigo.move_mouse(dx, dy, Coordinate::Rel),
        InputEvent::MouseMoveAbs { x, y } => {
            let px = (x.clamp(0.0, 1.0) * w as f32).round() as i32;
            let py = (y.clamp(0.0, 1.0) * h as f32).round() as i32;
            enigo.move_mouse(px, py, Coordinate::Abs)
        }
        InputEvent::MouseButton { button, pressed } => {
            enigo.button(to_button(button), dir(pressed))
        }
        InputEvent::MouseWheel { dx, dy } => {
            if dy != 0 {
                enigo.scroll(-dy, Axis::Vertical)
            } else {
                enigo.scroll(dx, Axis::Horizontal)
            }
        }
        InputEvent::Key { key, pressed } => match to_enigo_key(key) {
            Some(k) => enigo.key(k, dir(pressed)),
            None => {
                tracing::trace!("no enigo mapping for {key:?}");
                Ok(())
            }
        },
    };
    if let Err(e) = r {
        tracing::trace!("enigo inject error: {e}");
    }
}

fn dir(pressed: bool) -> Direction {
    if pressed {
        Direction::Press
    } else {
        Direction::Release
    }
}

fn to_button(b: MouseButton) -> Button {
    match b {
        MouseButton::Left => Button::Left,
        MouseButton::Right => Button::Right,
        MouseButton::Middle => Button::Middle,
        MouseButton::X1 => Button::Back,
        MouseButton::X2 => Button::Forward,
    }
}

/// Best-effort map from the portable key model to enigo. Returns `None` for keys
/// without a portable enigo equivalent (they are skipped).
fn to_enigo_key(key: Key) -> Option<EKey> {
    use Key::*;
    let k = match key {
        A => EKey::Unicode('a'),
        B => EKey::Unicode('b'),
        C => EKey::Unicode('c'),
        D => EKey::Unicode('d'),
        E => EKey::Unicode('e'),
        F => EKey::Unicode('f'),
        G => EKey::Unicode('g'),
        H => EKey::Unicode('h'),
        I => EKey::Unicode('i'),
        J => EKey::Unicode('j'),
        K => EKey::Unicode('k'),
        L => EKey::Unicode('l'),
        M => EKey::Unicode('m'),
        N => EKey::Unicode('n'),
        O => EKey::Unicode('o'),
        P => EKey::Unicode('p'),
        Q => EKey::Unicode('q'),
        R => EKey::Unicode('r'),
        S => EKey::Unicode('s'),
        T => EKey::Unicode('t'),
        U => EKey::Unicode('u'),
        V => EKey::Unicode('v'),
        W => EKey::Unicode('w'),
        X => EKey::Unicode('x'),
        Y => EKey::Unicode('y'),
        Z => EKey::Unicode('z'),
        Num0 => EKey::Unicode('0'),
        Num1 => EKey::Unicode('1'),
        Num2 => EKey::Unicode('2'),
        Num3 => EKey::Unicode('3'),
        Num4 => EKey::Unicode('4'),
        Num5 => EKey::Unicode('5'),
        Num6 => EKey::Unicode('6'),
        Num7 => EKey::Unicode('7'),
        Num8 => EKey::Unicode('8'),
        Num9 => EKey::Unicode('9'),
        Minus => EKey::Unicode('-'),
        Equal => EKey::Unicode('='),
        BracketLeft => EKey::Unicode('['),
        BracketRight => EKey::Unicode(']'),
        Backslash => EKey::Unicode('\\'),
        Semicolon => EKey::Unicode(';'),
        Quote => EKey::Unicode('\''),
        Backquote => EKey::Unicode('`'),
        Comma => EKey::Unicode(','),
        Period => EKey::Unicode('.'),
        Slash => EKey::Unicode('/'),
        Numpad0 => EKey::Unicode('0'),
        Numpad1 => EKey::Unicode('1'),
        Numpad2 => EKey::Unicode('2'),
        Numpad3 => EKey::Unicode('3'),
        Numpad4 => EKey::Unicode('4'),
        Numpad5 => EKey::Unicode('5'),
        Numpad6 => EKey::Unicode('6'),
        Numpad7 => EKey::Unicode('7'),
        Numpad8 => EKey::Unicode('8'),
        Numpad9 => EKey::Unicode('9'),
        NumpadAdd => EKey::Unicode('+'),
        NumpadSubtract => EKey::Unicode('-'),
        NumpadMultiply => EKey::Unicode('*'),
        NumpadDivide => EKey::Unicode('/'),
        NumpadDecimal => EKey::Unicode('.'),
        NumpadEnter => EKey::Return,
        Enter => EKey::Return,
        Escape => EKey::Escape,
        Backspace => EKey::Backspace,
        Tab => EKey::Tab,
        Space => EKey::Space,
        CapsLock => EKey::CapsLock,
        Delete => EKey::Delete,
        Home => EKey::Home,
        End => EKey::End,
        PageUp => EKey::PageUp,
        PageDown => EKey::PageDown,
        ArrowUp => EKey::UpArrow,
        ArrowDown => EKey::DownArrow,
        ArrowLeft => EKey::LeftArrow,
        ArrowRight => EKey::RightArrow,
        ControlLeft | ControlRight => EKey::Control,
        ShiftLeft | ShiftRight => EKey::Shift,
        AltLeft | AltRight => EKey::Alt,
        MetaLeft | MetaRight => EKey::Meta,
        F1 => EKey::F1,
        F2 => EKey::F2,
        F3 => EKey::F3,
        F4 => EKey::F4,
        F5 => EKey::F5,
        F6 => EKey::F6,
        F7 => EKey::F7,
        F8 => EKey::F8,
        F9 => EKey::F9,
        F10 => EKey::F10,
        F11 => EKey::F11,
        F12 => EKey::F12,
        Char(c) => EKey::Unicode(c),
        // No portable enigo equivalent (Insert, PrintScreen, ScrollLock, Pause,
        // NumLock, ContextMenu, Raw): skip.
        _ => return None,
    };
    Some(k)
}

// ---------------------------------------------------------------------------
// Capture / host role (EXPERIMENTAL) via rdev global grab.
//
// rdev delivers global input on Linux (X11) and macOS, and its `grab` mode can
// suppress events (return None) so they don't reach local apps while control is
// on the remote. Caveats that need on-device verification:
//   * macOS: needs Accessibility + Input Monitoring permission; suppression works.
//   * Linux: works on X11 sessions; Wayland is not supported, and rdev's
//     suppression on Linux is best-effort. Edge detection assumes a 1920×1080
//     desktop until a per-OS display-size query is added.
// The grab callback is a plain `fn` with no user pointer, so shared state lives
// in statics (same pattern as the Windows hooks).
// ---------------------------------------------------------------------------

static CAP_TX: OnceLock<SyncSender<CapturedEvent>> = OnceLock::new();
static CAP_SUPPRESS: AtomicBool = AtomicBool::new(false);
static CAP_LAST_X: AtomicI32 = AtomicI32::new(0);
static CAP_LAST_Y: AtomicI32 = AtomicI32::new(0);
static CAP_PRIMED: AtomicBool = AtomicBool::new(false);

pub struct RdevCapturer {
    rx: Receiver<CapturedEvent>,
}

impl RdevCapturer {
    pub fn start() -> anyhow::Result<Self> {
        let (tx, rx) = sync_channel::<CapturedEvent>(1024);
        CAP_TX
            .set(tx)
            .map_err(|_| anyhow::anyhow!("capturer already started"))?;

        std::thread::Builder::new()
            .name("screenlink-grab".into())
            .spawn(|| {
                if let Err(e) = rdev::grab(grab_callback) {
                    tracing::error!(
                        "global input grab failed (needs X11 + Accessibility/Input Monitoring \
                         permission; Wayland is unsupported): {e:?}"
                    );
                }
            })?;

        Ok(Self { rx })
    }
}

impl Capturer for RdevCapturer {
    fn poll(&self, timeout: Duration) -> Option<CapturedEvent> {
        self.rx.recv_timeout(timeout).ok()
    }
    fn set_suppress(&self, suppress: bool) {
        CAP_SUPPRESS.store(suppress, Ordering::Relaxed);
    }
    fn park_cursor(&self, _x: i32, _y: i32) {
        // Not needed: suppressing the move event keeps the cursor in place.
    }
}

fn send_cap(ev: CapturedEvent) {
    if let Some(tx) = CAP_TX.get() {
        let _ = tx.try_send(ev);
    }
}

fn grab_callback(event: rdev::Event) -> Option<rdev::Event> {
    use rdev::EventType;
    let suppress = CAP_SUPPRESS.load(Ordering::Relaxed);
    match event.event_type {
        EventType::MouseMove { x, y } => {
            let xi = x as i32;
            let yi = y as i32;
            let ox = CAP_LAST_X.swap(xi, Ordering::Relaxed);
            let oy = CAP_LAST_Y.swap(yi, Ordering::Relaxed);
            // Skip the delta on the very first sample (no previous point).
            let (dx, dy) = if CAP_PRIMED.swap(true, Ordering::Relaxed) {
                (xi - ox, yi - oy)
            } else {
                (0, 0)
            };
            if dx != 0 || dy != 0 {
                send_cap(CapturedEvent::Move {
                    dx,
                    dy,
                    abs_x: xi,
                    abs_y: yi,
                });
            }
        }
        EventType::KeyPress(k) => send_cap(CapturedEvent::Input(InputEvent::Key {
            key: from_rdev_key(k),
            pressed: true,
        })),
        EventType::KeyRelease(k) => send_cap(CapturedEvent::Input(InputEvent::Key {
            key: from_rdev_key(k),
            pressed: false,
        })),
        EventType::ButtonPress(b) => send_cap(CapturedEvent::Input(InputEvent::MouseButton {
            button: from_rdev_button(b),
            pressed: true,
        })),
        EventType::ButtonRelease(b) => send_cap(CapturedEvent::Input(InputEvent::MouseButton {
            button: from_rdev_button(b),
            pressed: false,
        })),
        EventType::Wheel { delta_x, delta_y } => {
            send_cap(CapturedEvent::Input(InputEvent::MouseWheel {
                dx: delta_x as i32,
                dy: delta_y as i32,
            }))
        }
    }
    if suppress {
        None
    } else {
        Some(event)
    }
}

fn from_rdev_button(b: rdev::Button) -> MouseButton {
    match b {
        rdev::Button::Left => MouseButton::Left,
        rdev::Button::Right => MouseButton::Right,
        rdev::Button::Middle => MouseButton::Middle,
        rdev::Button::Unknown(9) => MouseButton::X2,
        _ => MouseButton::X1,
    }
}

fn from_rdev_key(k: rdev::Key) -> Key {
    use rdev::Key as R;
    match k {
        R::KeyA => Key::A,
        R::KeyB => Key::B,
        R::KeyC => Key::C,
        R::KeyD => Key::D,
        R::KeyE => Key::E,
        R::KeyF => Key::F,
        R::KeyG => Key::G,
        R::KeyH => Key::H,
        R::KeyI => Key::I,
        R::KeyJ => Key::J,
        R::KeyK => Key::K,
        R::KeyL => Key::L,
        R::KeyM => Key::M,
        R::KeyN => Key::N,
        R::KeyO => Key::O,
        R::KeyP => Key::P,
        R::KeyQ => Key::Q,
        R::KeyR => Key::R,
        R::KeyS => Key::S,
        R::KeyT => Key::T,
        R::KeyU => Key::U,
        R::KeyV => Key::V,
        R::KeyW => Key::W,
        R::KeyX => Key::X,
        R::KeyY => Key::Y,
        R::KeyZ => Key::Z,
        R::Num1 => Key::Num1,
        R::Num2 => Key::Num2,
        R::Num3 => Key::Num3,
        R::Num4 => Key::Num4,
        R::Num5 => Key::Num5,
        R::Num6 => Key::Num6,
        R::Num7 => Key::Num7,
        R::Num8 => Key::Num8,
        R::Num9 => Key::Num9,
        R::Num0 => Key::Num0,
        R::F1 => Key::F1,
        R::F2 => Key::F2,
        R::F3 => Key::F3,
        R::F4 => Key::F4,
        R::F5 => Key::F5,
        R::F6 => Key::F6,
        R::F7 => Key::F7,
        R::F8 => Key::F8,
        R::F9 => Key::F9,
        R::F10 => Key::F10,
        R::F11 => Key::F11,
        R::F12 => Key::F12,
        R::ControlLeft => Key::ControlLeft,
        R::ControlRight => Key::ControlRight,
        R::ShiftLeft => Key::ShiftLeft,
        R::ShiftRight => Key::ShiftRight,
        R::Alt => Key::AltLeft,
        R::AltGr => Key::AltRight,
        R::MetaLeft => Key::MetaLeft,
        R::MetaRight => Key::MetaRight,
        R::Return => Key::Enter,
        R::Escape => Key::Escape,
        R::Backspace => Key::Backspace,
        R::Tab => Key::Tab,
        R::Space => Key::Space,
        R::CapsLock => Key::CapsLock,
        R::Delete => Key::Delete,
        R::Insert => Key::Insert,
        R::Home => Key::Home,
        R::End => Key::End,
        R::PageUp => Key::PageUp,
        R::PageDown => Key::PageDown,
        R::UpArrow => Key::ArrowUp,
        R::DownArrow => Key::ArrowDown,
        R::LeftArrow => Key::ArrowLeft,
        R::RightArrow => Key::ArrowRight,
        R::Minus => Key::Minus,
        R::Equal => Key::Equal,
        R::LeftBracket => Key::BracketLeft,
        R::RightBracket => Key::BracketRight,
        R::BackSlash => Key::Backslash,
        R::SemiColon => Key::Semicolon,
        R::Quote => Key::Quote,
        R::BackQuote => Key::Backquote,
        R::Comma => Key::Comma,
        R::Dot => Key::Period,
        R::Slash => Key::Slash,
        R::Kp0 => Key::Numpad0,
        R::Kp1 => Key::Numpad1,
        R::Kp2 => Key::Numpad2,
        R::Kp3 => Key::Numpad3,
        R::Kp4 => Key::Numpad4,
        R::Kp5 => Key::Numpad5,
        R::Kp6 => Key::Numpad6,
        R::Kp7 => Key::Numpad7,
        R::Kp8 => Key::Numpad8,
        R::Kp9 => Key::Numpad9,
        R::KpPlus => Key::NumpadAdd,
        R::KpMinus => Key::NumpadSubtract,
        R::KpMultiply => Key::NumpadMultiply,
        R::KpDivide => Key::NumpadDivide,
        R::KpReturn => Key::NumpadEnter,
        R::PrintScreen => Key::PrintScreen,
        R::ScrollLock => Key::ScrollLock,
        R::Pause => Key::Pause,
        R::NumLock => Key::NumLock,
        R::Unknown(n) => Key::Raw(n),
        _ => Key::Raw(0),
    }
}
