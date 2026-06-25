//! Windows low-level input capture (`WH_MOUSE_LL` / `WH_KEYBOARD_LL`).
//!
//! Low-level hooks must be serviced by a thread running a message loop, and the
//! C callback signature carries no user pointer — so shared state lives in
//! statics. We forward every event to the host loop over a channel, and when
//! `SUPPRESS` is set (control is on the remote) we *eat* the event locally and
//! keep the physical cursor parked at the seam.
//!
//! NOTE: hook behavior (especially cursor parking and feedback suppression) is
//! tuned against real two-machine use; see the manual test checklist in
//! `docs/`. Injection and the edge state machine are independently unit-testable.

use crate::keymap_win::key_for_vk;
use crate::win_inject::INJECT_SIGNATURE;
use crate::{CapturedEvent, Capturer};
use screenlink_core::protocol::{InputEvent, MouseButton};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::OnceLock;
use std::time::Duration;
use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, SetCursorPos, SetWindowsHookExW,
    TranslateMessage, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, MSG, MSLLHOOKSTRUCT,
    WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDOWN,
    WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN, WM_XBUTTONUP,
};

static EVENT_TX: OnceLock<SyncSender<CapturedEvent>> = OnceLock::new();
static SUPPRESS: AtomicBool = AtomicBool::new(false);
static LAST_X: AtomicI32 = AtomicI32::new(0);
static LAST_Y: AtomicI32 = AtomicI32::new(0);
static PARK_ON: AtomicBool = AtomicBool::new(false);
static PARK_X: AtomicI32 = AtomicI32::new(0);
static PARK_Y: AtomicI32 = AtomicI32::new(0);
// Drops the first few WM_MOUSEMOVE deltas after a park, so the cursor's
// edge→park snap isn't itself relayed as a huge bogus motion.
static PARK_SKIP: AtomicU32 = AtomicU32::new(0);

pub struct WinCapturer {
    rx: Receiver<CapturedEvent>,
}

impl WinCapturer {
    pub fn start() -> anyhow::Result<Self> {
        let (tx, rx) = sync_channel::<CapturedEvent>(1024);
        EVENT_TX
            .set(tx)
            .map_err(|_| anyhow::anyhow!("capturer already started"))?;

        let (px, py) = crate::win_inject::cursor_pos();
        LAST_X.store(px, Ordering::Relaxed);
        LAST_Y.store(py, Ordering::Relaxed);

        // The hook thread owns the hooks and pumps messages for their lifetime.
        std::thread::Builder::new()
            .name("screenlink-hooks".into())
            .spawn(|| unsafe {
                let hinst: HINSTANCE = GetModuleHandleW(None)
                    .map(HINSTANCE::from)
                    .unwrap_or_default();
                let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), hinst, 0);
                let kbd = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), hinst, 0);
                if mouse.is_err() || kbd.is_err() {
                    tracing::error!("failed to install low-level hooks: {mouse:?} {kbd:?}");
                    return;
                }
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            })?;

        Ok(Self { rx })
    }
}

impl Capturer for WinCapturer {
    fn poll(&self, timeout: Duration) -> Option<CapturedEvent> {
        self.rx.recv_timeout(timeout).ok()
    }

    fn set_suppress(&self, suppress: bool) {
        SUPPRESS.store(suppress, Ordering::Relaxed);
        if !suppress {
            PARK_ON.store(false, Ordering::Relaxed);
            PARK_SKIP.store(0, Ordering::Relaxed);
        }
    }

    fn park_cursor(&self, x: i32, y: i32) {
        PARK_X.store(x, Ordering::Relaxed);
        PARK_Y.store(y, Ordering::Relaxed);
        PARK_ON.store(true, Ordering::Relaxed);
        LAST_X.store(x, Ordering::Relaxed);
        LAST_Y.store(y, Ordering::Relaxed);
        // Eat the next few moves so the snap from edge to park doesn't relay as
        // a giant delta into the remote.
        PARK_SKIP.store(4, Ordering::Relaxed);
        // Physically move the cursor now — otherwise the hook keeps reading
        // info.pt at the old (edge) position, dx stays zero in the off-screen
        // direction, and only the user's back-toward-the-desktop wobble gets
        // relayed (which looked like inverted motion).
        unsafe {
            let _ = SetCursorPos(x, y);
        }
    }
}

fn send(ev: CapturedEvent) {
    if let Some(tx) = EVENT_TX.get() {
        // Non-blocking: drop on overflow rather than stall the hook (which would
        // freeze global input).
        let _ = tx.try_send(ev);
    }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code != HC_ACTION as i32 {
        return CallNextHookEx(None, code, wparam, lparam);
    }
    let info = &*(lparam.0 as *const MSLLHOOKSTRUCT);
    if info.dwExtraInfo == INJECT_SIGNATURE {
        // Our own injected event (shouldn't normally reach here, but guard).
        return CallNextHookEx(None, code, wparam, lparam);
    }

    let suppress = SUPPRESS.load(Ordering::Relaxed);
    let msg = wparam.0 as u32;

    match msg {
        WM_MOUSEMOVE => {
            let x = info.pt.x;
            let y = info.pt.y;
            let dx = x - LAST_X.load(Ordering::Relaxed);
            let dy = y - LAST_Y.load(Ordering::Relaxed);
            LAST_X.store(x, Ordering::Relaxed);
            LAST_Y.store(y, Ordering::Relaxed);

            let skip = PARK_SKIP.load(Ordering::Relaxed);
            if skip > 0 {
                PARK_SKIP.fetch_sub(1, Ordering::Relaxed);
            } else if dx != 0 || dy != 0 {
                send(CapturedEvent::Move {
                    dx,
                    dy,
                    abs_x: x,
                    abs_y: y,
                });
            }

            if suppress {
                // Keep the physical cursor parked at the seam. Only reset when the
                // cursor has actually drifted — otherwise the synthetic move that
                // SetCursorPos itself generates would trigger another reset, an
                // infinite feedback storm that makes motion erratic.
                if PARK_ON.load(Ordering::Relaxed) {
                    let px = PARK_X.load(Ordering::Relaxed);
                    let py = PARK_Y.load(Ordering::Relaxed);
                    if x != px || y != py {
                        let _ = SetCursorPos(px, py);
                    }
                    LAST_X.store(px, Ordering::Relaxed);
                    LAST_Y.store(py, Ordering::Relaxed);
                }
                return LRESULT(1); // eat
            }
        }
        _ => {
            if let Some(ev) = mouse_button_or_wheel(msg, info) {
                send(CapturedEvent::Input(ev));
                if suppress {
                    return LRESULT(1);
                }
            }
        }
    }

    CallNextHookEx(None, code, wparam, lparam)
}

fn mouse_button_or_wheel(msg: u32, info: &MSLLHOOKSTRUCT) -> Option<InputEvent> {
    let high = ((info.mouseData >> 16) & 0xFFFF) as u16;
    match msg {
        WM_LBUTTONDOWN => Some(btn(MouseButton::Left, true)),
        WM_LBUTTONUP => Some(btn(MouseButton::Left, false)),
        WM_RBUTTONDOWN => Some(btn(MouseButton::Right, true)),
        WM_RBUTTONUP => Some(btn(MouseButton::Right, false)),
        WM_MBUTTONDOWN => Some(btn(MouseButton::Middle, true)),
        WM_MBUTTONUP => Some(btn(MouseButton::Middle, false)),
        WM_XBUTTONDOWN => Some(btn(xbutton(high), true)),
        WM_XBUTTONUP => Some(btn(xbutton(high), false)),
        // Pass the raw wheel delta through (±120 per notch on a discrete wheel,
        // much smaller for touchpad fine scroll). Integer-dividing here would
        // round touchpad scrolls down to zero — that's why two-finger scrolling
        // on the remote was a no-op.
        WM_MOUSEWHEEL => Some(InputEvent::MouseWheel {
            dx: 0,
            dy: high as i16 as i32,
        }),
        WM_MOUSEHWHEEL => Some(InputEvent::MouseWheel {
            dx: high as i16 as i32,
            dy: 0,
        }),
        _ => None,
    }
}

fn xbutton(high: u16) -> MouseButton {
    if high == 2 {
        MouseButton::X2
    } else {
        MouseButton::X1
    }
}

fn btn(button: MouseButton, pressed: bool) -> InputEvent {
    InputEvent::MouseButton { button, pressed }
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code != HC_ACTION as i32 {
        return CallNextHookEx(None, code, wparam, lparam);
    }
    let info = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
    if info.dwExtraInfo == INJECT_SIGNATURE {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    let msg = wparam.0 as u32;
    let pressed = matches!(msg, WM_KEYDOWN | WM_SYSKEYDOWN);
    let released = matches!(msg, WM_KEYUP | WM_SYSKEYUP);
    if pressed || released {
        let extended = (info.flags.0 & LLKHF_EXTENDED.0) != 0;
        let ev = InputEvent::Key {
            key: key_for_vk(info.vkCode as u16, extended),
            pressed,
        };
        send(CapturedEvent::Input(ev));
        if SUPPRESS.load(Ordering::Relaxed) {
            return LRESULT(1);
        }
    }

    CallNextHookEx(None, code, wparam, lparam)
}
