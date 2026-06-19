//! Input capture (host) and injection (client) plus the edge-transition logic.
//!
//! - [`edge`] is pure, OS-independent, and unit-tested.
//! - On Windows, [`new_injector`] uses `SendInput`/`SetCursorPos` and
//!   [`new_capturer`] installs low-level keyboard/mouse hooks.
//! - On other platforms (and for parts of the loopback dev mode), a no-op stub
//!   keeps everything compiling and the abstractions honest.

pub mod edge;

pub use edge::{ControlSite, EdgeDetector, Rect, Transition};
use screenlink_core::protocol::InputEvent;

/// Injects device-independent input events into the local OS (client side).
pub trait Injector: Send {
    /// Inject one event.
    fn inject(&mut self, ev: InputEvent) -> anyhow::Result<()>;
    /// The bounds of the (virtual) desktop, used to map absolute coordinates.
    fn desktop_rect(&self) -> Rect;
    /// Place the cursor at a normalized position (0..1) of the desktop.
    fn set_cursor_norm(&mut self, x: f32, y: f32) -> anyhow::Result<()>;
}

/// An event observed by the host's capture layer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CapturedEvent {
    /// Mouse motion, with both relative deltas (to relay) and absolute position
    /// (for local edge detection).
    Move {
        dx: i32,
        dy: i32,
        abs_x: i32,
        abs_y: i32,
    },
    /// A button, wheel, or key event already normalized for relay.
    Input(InputEvent),
}

/// Captures local input on the host and reports it, with the ability to suppress
/// local effect while control is on a remote screen.
pub trait Capturer: Send {
    /// Pull the next captured event, blocking up to `timeout`. `None` on timeout.
    fn poll(&self, timeout: std::time::Duration) -> Option<CapturedEvent>;
    /// When `true`, captured events are eaten locally (not delivered to local
    /// apps) and only relayed — i.e. control is on the remote.
    fn set_suppress(&self, suppress: bool);
    /// Park the cursor at this absolute pixel (used to lock it at the seam while
    /// remote). No-op on platforms without cursor control.
    fn park_cursor(&self, x: i32, y: i32);
}

#[cfg(windows)]
mod keymap_win;
#[cfg(not(any(windows, unix)))]
mod stub;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod win_capture;
#[cfg(windows)]
mod win_inject;

/// Mark the process per-monitor DPI aware so coordinates are physical pixels.
/// Call once at startup, before creating windows. No-op off Windows.
pub fn set_dpi_aware() {
    #[cfg(windows)]
    win_inject::set_dpi_aware();
}

/// The bounds of the local (virtual) desktop in physical pixels. Off Windows this
/// is a sensible default so the loopback dev mode has something to work with.
pub fn desktop_rect() -> Rect {
    #[cfg(windows)]
    {
        win_inject::virtual_desktop_rect()
    }
    #[cfg(not(windows))]
    {
        Rect::new(0, 0, 1920, 1080)
    }
}

/// Create the platform injector (client side).
pub fn new_injector() -> anyhow::Result<Box<dyn Injector>> {
    #[cfg(windows)]
    {
        Ok(Box::new(win_inject::WinInjector::new()))
    }
    #[cfg(unix)]
    {
        Ok(Box::new(unix::EnigoInjector::new()?))
    }
    #[cfg(not(any(windows, unix)))]
    {
        Ok(Box::new(stub::StubInjector::default()))
    }
}

/// Create the platform capturer (host side).
pub fn new_capturer() -> anyhow::Result<Box<dyn Capturer>> {
    #[cfg(windows)]
    {
        Ok(Box::new(win_capture::WinCapturer::start()?))
    }
    #[cfg(unix)]
    {
        // Experimental: global grab via rdev (X11 / macOS). See unix.rs.
        Ok(Box::new(unix::RdevCapturer::start()?))
    }
    #[cfg(not(any(windows, unix)))]
    {
        Ok(Box::new(stub::StubCapturer::default()))
    }
}
