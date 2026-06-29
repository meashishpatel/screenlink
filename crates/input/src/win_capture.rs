//! Windows low-level input capture (`WH_MOUSE_LL` / `WH_KEYBOARD_LL`) plus a
//! hidden message-only window registered for **Raw Input** (`WM_INPUT`).
//!
//! Why two paths:
//!  - `WH_MOUSE_LL` gives us the *absolute screen position* (for edge detection
//!    while control is local) and button events — but `info.pt` saturates at the
//!    screen edge, which used to require parking the cursor 200 px back inside
//!    the desktop so the off-screen direction stayed measurable. That looked
//!    like a visible "bump backwards" on every cross.
//!  - **Raw Input** gives true HID-level relative deltas independent of where
//!    the cursor is physically pinned, *and* it catches wheel events from
//!    Precision Touchpads (two-finger scroll) that `WH_MOUSE_LL` may miss.
//!
//! While control is on the remote (`SUPPRESS` true) we `ClipCursor` the local
//! cursor to a 1×1 rect at the seam, so it cannot physically move and the
//! user sees only one cursor — the relayed one on the controlled device. Raw
//! Input gives us the deltas to drive that relayed cursor.

use crate::keymap_win::key_for_vk;
use crate::win_inject::INJECT_SIGNATURE;
use crate::{CapturedEvent, Capturer};
use screenlink_core::protocol::{InputEvent, MouseButton};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::OnceLock;
use std::time::Duration;
use windows::core::w;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::{
    GetRawInputData, RegisterRawInputDevices, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER,
    RAWMOUSE, RIDEV_INPUTSINK, RID_INPUT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, ClipCursor, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW,
    RegisterClassW, SetCursorPos, SetWindowsHookExW, TranslateMessage, HC_ACTION, HMENU,
    HWND_MESSAGE, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, MSG, MSLLHOOKSTRUCT, WH_KEYBOARD_LL,
    WH_MOUSE_LL, WINDOW_EX_STYLE, WINDOW_STYLE, WM_INPUT, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL,
    WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN, WM_XBUTTONUP,
    WNDCLASSW,
};

static EVENT_TX: OnceLock<SyncSender<CapturedEvent>> = OnceLock::new();
static SUPPRESS: AtomicBool = AtomicBool::new(false);
static LAST_X: AtomicI32 = AtomicI32::new(0);
static LAST_Y: AtomicI32 = AtomicI32::new(0);
static PARK_ON: AtomicBool = AtomicBool::new(false);
static PARK_X: AtomicI32 = AtomicI32::new(0);
static PARK_Y: AtomicI32 = AtomicI32::new(0);
/// True once the Raw Input device has been registered against our hidden
/// window. While this is set, the legacy `WM_MOUSEWHEEL` / `WM_MOUSEHWHEEL`
/// path in the LL hook stops emitting wheel events (Raw Input is authoritative
/// — that way standard mice don't double-scroll), and the LL hook stops
/// emitting `Move` events while suppressed (Raw Input gives true HID deltas).
static RAW_INPUT_OK: AtomicBool = AtomicBool::new(false);

const RI_MOUSE_WHEEL: u16 = 0x0400;
const RI_MOUSE_HWHEEL: u16 = 0x0800;
const MOUSE_MOVE_ABSOLUTE_FLAG: u16 = 0x01;
const RIM_TYPEMOUSE_RAW: u32 = 0;

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
                if let Err(e) = install_raw_input(hinst) {
                    tracing::warn!(
                        "raw input setup failed: {e}; falling back to legacy hook-only capture"
                    );
                } else {
                    RAW_INPUT_OK.store(true, Ordering::Relaxed);
                    tracing::info!("raw input active (touchpad scroll + cursor-clipped deltas)");
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
            // Release the cursor confinement so the user can move freely again.
            unsafe {
                let _ = ClipCursor(None);
            }
        }
    }

    fn park_cursor(&self, x: i32, y: i32) {
        PARK_X.store(x, Ordering::Relaxed);
        PARK_Y.store(y, Ordering::Relaxed);
        PARK_ON.store(true, Ordering::Relaxed);
        LAST_X.store(x, Ordering::Relaxed);
        LAST_Y.store(y, Ordering::Relaxed);
        unsafe {
            // Move the cursor to the seam and then clip it to a 1×1 rect there.
            // ClipCursor with a 1-pixel area effectively immobilizes the local
            // cursor: it can't drift, doesn't generate `WM_MOUSEMOVE` events,
            // and visually never crosses the desktop while control is remote.
            // Deltas keep flowing because Raw Input reports HID motion
            // independent of cursor position.
            let _ = SetCursorPos(x, y);
            let rect = RECT {
                left: x,
                top: y,
                right: x + 1,
                bottom: y + 1,
            };
            let _ = ClipCursor(Some(&rect));
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

unsafe fn install_raw_input(hinst: HINSTANCE) -> anyhow::Result<()> {
    let class_name = w!("ScreenLinkRawInputWindow");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(raw_input_wnd_proc),
        hInstance: hinst,
        lpszClassName: class_name,
        ..Default::default()
    };
    let atom = RegisterClassW(&wc);
    if atom == 0 {
        // Already-registered isn't actually fatal but we couldn't know that
        // from the API alone; CreateWindowExW would still succeed by class
        // name. So treat 0 as best-effort and continue.
        tracing::debug!("RegisterClassW returned 0 (possibly already registered)");
    }

    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        class_name,
        w!(""),
        WINDOW_STYLE(0),
        0,
        0,
        0,
        0,
        HWND_MESSAGE,
        HMENU::default(),
        hinst,
        None,
    )?;

    let rid = RAWINPUTDEVICE {
        usUsagePage: 0x01, // HID Generic Desktop
        usUsage: 0x02,     // Mouse
        dwFlags: RIDEV_INPUTSINK,
        hwndTarget: hwnd,
    };
    RegisterRawInputDevices(&[rid], std::mem::size_of::<RAWINPUTDEVICE>() as u32)?;
    Ok(())
}

unsafe extern "system" fn raw_input_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_INPUT {
        process_raw_input(lparam);
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

unsafe fn process_raw_input(lparam: LPARAM) {
    let hri = HRAWINPUT(lparam.0 as *mut _);
    let header_size = std::mem::size_of::<RAWINPUTHEADER>() as u32;
    let mut size: u32 = 0;
    let _ = GetRawInputData(hri, RID_INPUT, None, &mut size, header_size);
    if size == 0 || size > 4096 {
        return;
    }
    let mut buf = vec![0u8; size as usize];
    let got = GetRawInputData(
        hri,
        RID_INPUT,
        Some(buf.as_mut_ptr() as _),
        &mut size,
        header_size,
    );
    if got == u32::MAX {
        return;
    }
    let ri = &*(buf.as_ptr() as *const RAWINPUT);
    if ri.header.dwType != RIM_TYPEMOUSE_RAW {
        return;
    }
    handle_raw_mouse(&ri.data.mouse);
}

unsafe fn handle_raw_mouse(m: &RAWMOUSE) {
    let suppress = SUPPRESS.load(Ordering::Relaxed);

    // ---- Relative motion ----
    // Only honor relative reports; absolute reports (pen, RDP) would confuse
    // the delta accumulator. While suppressed, raw deltas are the source of
    // truth for the relayed cursor on the remote. While not suppressed, the
    // LL hook's WM_MOUSEMOVE absolute path handles cursor + edge detection.
    let absolute = (m.usFlags.0 & MOUSE_MOVE_ABSOLUTE_FLAG) != 0;
    if suppress && !absolute && (m.lLastX != 0 || m.lLastY != 0) {
        send(CapturedEvent::Move {
            dx: m.lLastX,
            dy: m.lLastY,
            abs_x: PARK_X.load(Ordering::Relaxed),
            abs_y: PARK_Y.load(Ordering::Relaxed),
        });
    }

    // ---- Wheel ----
    // usButtonFlags is the union's first field; usButtonData is the next.
    let btn_flags = m.Anonymous.Anonymous.usButtonFlags;
    let btn_data = m.Anonymous.Anonymous.usButtonData as i16 as i32;
    if btn_flags & RI_MOUSE_WHEEL != 0 {
        send(CapturedEvent::Input(InputEvent::MouseWheel {
            dx: 0,
            dy: btn_data,
        }));
    }
    if btn_flags & RI_MOUSE_HWHEEL != 0 {
        send(CapturedEvent::Input(InputEvent::MouseWheel {
            dx: btn_data,
            dy: 0,
        }));
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
    let raw_input_active = RAW_INPUT_OK.load(Ordering::Relaxed);
    let msg = wparam.0 as u32;

    match msg {
        WM_MOUSEMOVE => {
            let x = info.pt.x;
            let y = info.pt.y;
            let dx = x - LAST_X.load(Ordering::Relaxed);
            let dy = y - LAST_Y.load(Ordering::Relaxed);
            LAST_X.store(x, Ordering::Relaxed);
            LAST_Y.store(y, Ordering::Relaxed);

            // While suppressed, Raw Input is the authoritative delta source
            // (the cursor is ClipCursor-pinned to 1×1 anyway). The LL hook
            // only emits Move events when control is on local — needed for
            // edge detection from absolute screen coordinates.
            if !suppress && (dx != 0 || dy != 0) {
                send(CapturedEvent::Move {
                    dx,
                    dy,
                    abs_x: x,
                    abs_y: y,
                });
            } else if suppress && !raw_input_active && (dx != 0 || dy != 0) {
                // Fallback when raw input isn't running for some reason: do
                // what the older code did and report deltas from the hook.
                send(CapturedEvent::Move {
                    dx,
                    dy,
                    abs_x: x,
                    abs_y: y,
                });
            }

            if suppress {
                // Cursor is clipped to 1×1 by ClipCursor while parked, so this
                // should normally be a no-op; the snap-back is here only as
                // belt-and-braces in case ClipCursor was somehow released.
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
            if let Some(ev) = mouse_button_or_wheel(msg, info, raw_input_active) {
                send(CapturedEvent::Input(ev));
            }
            if suppress {
                // Eat buttons/wheel locally even if we didn't emit (Raw Input
                // is handling the relay for wheel events).
                if matches!(
                    msg,
                    WM_LBUTTONDOWN
                        | WM_LBUTTONUP
                        | WM_RBUTTONDOWN
                        | WM_RBUTTONUP
                        | WM_MBUTTONDOWN
                        | WM_MBUTTONUP
                        | WM_XBUTTONDOWN
                        | WM_XBUTTONUP
                        | WM_MOUSEWHEEL
                        | WM_MOUSEHWHEEL
                ) {
                    return LRESULT(1);
                }
            }
        }
    }

    CallNextHookEx(None, code, wparam, lparam)
}

fn mouse_button_or_wheel(
    msg: u32,
    info: &MSLLHOOKSTRUCT,
    raw_input_active: bool,
) -> Option<InputEvent> {
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
        WM_MOUSEWHEEL if !raw_input_active => Some(InputEvent::MouseWheel {
            dx: 0,
            dy: high as i16 as i32,
        }),
        WM_MOUSEHWHEEL if !raw_input_active => Some(InputEvent::MouseWheel {
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
