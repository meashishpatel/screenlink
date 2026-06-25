//! Windows input injection via `SendInput`, and desktop/DPI helpers.

use crate::keymap_win::{vk_for, VkMapping};
use crate::{edge::Rect, Injector};
use screenlink_core::protocol::{InputEvent, Key, MouseButton};
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEEVENTF_ABSOLUTE,
    MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN,
    MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT,
    MOUSE_EVENT_FLAGS, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetSystemMetrics, SetCursorPos, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

/// Tag attached to injected events via `dwExtraInfo` so our own capture hook can
/// recognize and ignore them (prevents feedback loops when a device both
/// captures and injects).
pub(crate) const INJECT_SIGNATURE: usize = 0x5C11_5C11;

const XBUTTON1: u32 = 0x0001;
const XBUTTON2: u32 = 0x0002;

pub fn set_dpi_aware() {
    unsafe {
        // Best-effort; ignore failure on older Windows.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

pub fn virtual_desktop_rect() -> Rect {
    unsafe {
        Rect::new(
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1),
            GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1),
        )
    }
}

pub fn cursor_pos() -> (i32, i32) {
    unsafe {
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        (p.x, p.y)
    }
}

#[derive(Default)]
pub struct WinInjector;

impl WinInjector {
    pub fn new() -> Self {
        Self
    }
}

impl Injector for WinInjector {
    fn inject(&mut self, ev: InputEvent) -> anyhow::Result<()> {
        let input = match ev {
            InputEvent::MouseMove { dx, dy } => mouse_input(dx, dy, 0, MOUSEEVENTF_MOVE),
            InputEvent::MouseMoveAbs { x, y } => {
                let r = virtual_desktop_rect();
                // Absolute coordinates for VIRTUALDESK are 0..65535 across the
                // whole virtual screen.
                let ax = (x.clamp(0.0, 1.0) * 65535.0).round() as i32;
                let ay = (y.clamp(0.0, 1.0) * 65535.0).round() as i32;
                let _ = r;
                mouse_input(
                    ax,
                    ay,
                    0,
                    MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                )
            }
            InputEvent::MouseButton { button, pressed } => {
                let (flags, data) = button_flags(button, pressed);
                mouse_input(0, 0, data, flags)
            }
            InputEvent::MouseWheel { dx, dy } => {
                // Protocol carries raw wheel units (Windows WHEEL_DELTA basis:
                // ±120 per notch, smaller for touchpad fine scroll) — pass
                // straight through to SendInput's mouseData.
                if dy != 0 {
                    mouse_input(0, 0, dy as u32, MOUSEEVENTF_WHEEL)
                } else {
                    mouse_input(0, 0, dx as u32, MOUSEEVENTF_HWHEEL)
                }
            }
            InputEvent::Key { key, pressed } => return inject_key(key, pressed),
        };
        send_one(input)
    }

    fn desktop_rect(&self) -> Rect {
        virtual_desktop_rect()
    }

    fn set_cursor_norm(&mut self, x: f32, y: f32) -> anyhow::Result<()> {
        let r = virtual_desktop_rect();
        let px = r.x + (x.clamp(0.0, 1.0) * r.w as f32).round() as i32;
        let py = r.y + (y.clamp(0.0, 1.0) * r.h as f32).round() as i32;
        unsafe {
            SetCursorPos(px, py)?;
        }
        Ok(())
    }
}

fn button_flags(button: MouseButton, pressed: bool) -> (MOUSE_EVENT_FLAGS, u32) {
    match (button, pressed) {
        (MouseButton::Left, true) => (MOUSEEVENTF_LEFTDOWN, 0),
        (MouseButton::Left, false) => (MOUSEEVENTF_LEFTUP, 0),
        (MouseButton::Right, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
        (MouseButton::Right, false) => (MOUSEEVENTF_RIGHTUP, 0),
        (MouseButton::Middle, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
        (MouseButton::Middle, false) => (MOUSEEVENTF_MIDDLEUP, 0),
        (MouseButton::X1, true) => (MOUSEEVENTF_XDOWN, XBUTTON1),
        (MouseButton::X1, false) => (MOUSEEVENTF_XUP, XBUTTON1),
        (MouseButton::X2, true) => (MOUSEEVENTF_XDOWN, XBUTTON2),
        (MouseButton::X2, false) => (MOUSEEVENTF_XUP, XBUTTON2),
    }
}

fn mouse_input(dx: i32, dy: i32, data: u32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: INJECT_SIGNATURE,
            },
        },
    }
}

fn inject_key(key: Key, pressed: bool) -> anyhow::Result<()> {
    match vk_for(key) {
        VkMapping::Vk(vk, extended) => {
            let mut flags = KEYBD_EVENT_FLAGS(0);
            if !pressed {
                flags |= KEYEVENTF_KEYUP;
            }
            if extended {
                flags |= KEYEVENTF_EXTENDEDKEY;
            }
            send_one(key_input(vk, flags))
        }
        VkMapping::Unicode(ch) => {
            // One INPUT per UTF-16 code unit (handles surrogate pairs).
            let mut buf = [0u16; 2];
            let units = ch.encode_utf16(&mut buf);
            let inputs: Vec<INPUT> = units
                .iter()
                .map(|&u| {
                    let mut flags = KEYEVENTF_UNICODE;
                    if !pressed {
                        flags |= KEYEVENTF_KEYUP;
                    }
                    unicode_input(u, flags)
                })
                .collect();
            send_all(&inputs)
        }
    }
}

fn unicode_input(scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: INJECT_SIGNATURE,
            },
        },
    }
}

fn send_all(inputs: &[INPUT]) -> anyhow::Result<()> {
    if inputs.is_empty() {
        return Ok(());
    }
    let n = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    if n as usize == inputs.len() {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "SendInput sent {n}/{} events",
            inputs.len()
        ))
    }
}

fn key_input(vk: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: INJECT_SIGNATURE,
            },
        },
    }
}

fn send_one(input: INPUT) -> anyhow::Result<()> {
    let n = unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
    if n == 1 {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "SendInput failed (blocked by UIPI or invalid event)"
        ))
    }
}
