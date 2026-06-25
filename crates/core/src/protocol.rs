//! Wire protocol: device identity, capabilities, control-channel messages, and
//! the device-independent input-event format carried on the realtime channel.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Bumped whenever the wire format changes incompatibly. Peers refuse to talk if
/// their major versions differ.
pub const PROTOCOL_VERSION: u16 = 1;

/// Stable identifier for a device: the lowercase hex SHA-256 fingerprint of its
/// long-term TLS certificate. This is what the trust store keys on, so identity
/// can't be spoofed without the private key.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeviceId(pub String);

impl DeviceId {
    /// Short, human-friendly form for the UI (first 8 hex chars).
    pub fn short(&self) -> &str {
        let n = self.0.len().min(8);
        &self.0[..n]
    }
}

impl fmt::Debug for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeviceId({}…)", self.short())
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// What a peer can do, exchanged in the hello handshake.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Capabilities {
    pub control_mode: bool,
    /// Whether this build/device can act as an Extend-mode display sink (Phase 2).
    pub extend_mode: bool,
    pub clipboard_text: bool,
    pub clipboard_images: bool,
    pub file_transfer: bool,
}

impl Default for Capabilities {
    fn default() -> Self {
        Self {
            control_mode: true,
            extend_mode: false,
            clipboard_text: true,
            clipboard_images: false,
            file_transfer: false,
        }
    }
}

/// Human-meaningful description of a device, exchanged on connect.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: DeviceId,
    pub name: String,
    pub os: String,
    /// UDP port this device listens on for the realtime channel, so a controller
    /// knows where to send input even if the two sides use different ports.
    pub realtime_port: u16,
    pub caps: Capabilities,
}

/// The per-connection mode the user selects for a device.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Control,
    Extend,
}

/// Which edge of the host's screen rectangle a peer is attached to.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ScreenEdge {
    Left,
    Right,
    Top,
    Bottom,
}

impl ScreenEdge {
    pub fn opposite(self) -> ScreenEdge {
        match self {
            ScreenEdge::Left => ScreenEdge::Right,
            ScreenEdge::Right => ScreenEdge::Left,
            ScreenEdge::Top => ScreenEdge::Bottom,
            ScreenEdge::Bottom => ScreenEdge::Top,
        }
    }
}

/// Mouse buttons in a device-independent form.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

/// A platform-neutral keyboard key, modeled on physical key *position* (US-layout
/// names, like USB HID usages) rather than any one OS's virtual-key codes. Each
/// OS backend maps this to/from its native codes, so a key pressed on Windows
/// produces the right key on Linux/macOS and vice-versa.
///
/// `Char` carries a Unicode character for text that has no positional name (the
/// injecting side types it directly); `Raw` carries an unmapped native code as a
/// same-OS fallback.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Key {
    // Letters (physical positions).
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    // Number row.
    Num0,
    Num1,
    Num2,
    Num3,
    Num4,
    Num5,
    Num6,
    Num7,
    Num8,
    Num9,
    // Function row.
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    // Modifiers.
    ControlLeft,
    ControlRight,
    ShiftLeft,
    ShiftRight,
    AltLeft,
    AltRight,
    MetaLeft,
    MetaRight,
    // Editing / whitespace.
    Enter,
    Escape,
    Backspace,
    Tab,
    Space,
    CapsLock,
    // Navigation / editing.
    Insert,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    // Punctuation (US-layout positions).
    Minus,
    Equal,
    BracketLeft,
    BracketRight,
    Backslash,
    Semicolon,
    Quote,
    Backquote,
    Comma,
    Period,
    Slash,
    // Numpad.
    Numpad0,
    Numpad1,
    Numpad2,
    Numpad3,
    Numpad4,
    Numpad5,
    Numpad6,
    Numpad7,
    Numpad8,
    Numpad9,
    NumpadAdd,
    NumpadSubtract,
    NumpadMultiply,
    NumpadDivide,
    NumpadDecimal,
    NumpadEnter,
    // System / locks.
    PrintScreen,
    ScrollLock,
    Pause,
    NumLock,
    ContextMenu,
    // Fallbacks.
    /// A literal character to type (layout-dependent text input).
    Char(char),
    /// An unmapped native key code (same-OS fallback only).
    Raw(u32),
}

/// Device-independent input events carried on the realtime channel.
///
/// Mouse movement is relative (raw deltas) so it survives differing resolutions
/// and per-monitor DPI; the injecting side clamps to its desktop bounds. An
/// absolute normalized variant is provided for the moment control crosses an
/// edge, so the remote cursor appears at the right spot.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub enum InputEvent {
    /// Relative mouse motion in raw units.
    MouseMove {
        dx: i32,
        dy: i32,
    },
    /// Absolute position normalized to the remote primary desktop, range 0.0..=1.0.
    MouseMoveAbs {
        x: f32,
        y: f32,
    },
    MouseButton {
        button: MouseButton,
        pressed: bool,
    },
    /// Wheel deltas in raw Windows wheel units (±120 = one discrete notch;
    /// touchpad fine scroll produces much smaller values). `dy` vertical,
    /// `dx` horizontal. Non-Windows backends scale to/from their native
    /// tick unit at the edge.
    MouseWheel {
        dx: i32,
        dy: i32,
    },
    Key {
        key: Key,
        pressed: bool,
    },
}

/// Reliable control-channel messages (TLS 1.3 / TCP), length-prefixed postcard.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ControlMsg {
    /// First message each side sends after the TLS handshake.
    Hello {
        protocol: u16,
        info: DeviceInfo,
    },
    HelloAck {
        info: DeviceInfo,
    },
    /// Protocol/version mismatch or refusal, with a reason for the UI.
    Reject {
        reason: String,
    },

    // ---- Pairing (numeric-comparison model) ----
    /// Begin pairing. The 6-digit comparison PIN is derived identically on both
    /// sides from the two certificate fingerprints (see `pairing`), so no PIN is
    /// transmitted. This message just signals intent.
    PairBegin,
    /// Local user confirmed the displayed PIN matches the peer's.
    PairConfirm,
    /// Sent once both sides have confirmed; peer should persist trust.
    PairConfirmed,
    PairCancel {
        reason: String,
    },

    // ---- Session control ----
    SetMode {
        mode: Mode,
    },
    ModeAck {
        mode: Mode,
        ok: bool,
        reason: String,
    },

    /// Tell the peer where to send/expect realtime UDP and the negotiated epoch.
    RealtimeOpen {
        udp_port: u16,
        epoch: u64,
    },

    /// Screen mirror: the sender will stream its screen as video on the video
    /// channel; the receiver should start decoding/displaying it.
    StartMirror {
        epoch: u64,
    },
    StopMirror,

    // ---- Edge crossing handshake ----
    /// Control is entering the remote screen; `pos` is the normalized entry point
    /// (0..1) along/within the shared edge so the remote cursor appears correctly.
    EdgeEnter {
        x: f32,
        y: f32,
    },
    /// Control is leaving the remote screen, returning to the host.
    EdgeLeave,

    // ---- Clipboard ----
    Clipboard(ClipboardData),

    // ---- Liveness / RTT ----
    Ping {
        nonce: u64,
    },
    Pong {
        nonce: u64,
    },
}

/// Clipboard payloads. Phase 1 is text only; images/files are Phase 3.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ClipboardData {
    Text(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_id_short_is_truncated() {
        let id = DeviceId("abcdef0123456789".into());
        assert_eq!(id.short(), "abcdef01");
    }

    #[test]
    fn device_id_short_handles_tiny() {
        let id = DeviceId("ab".into());
        assert_eq!(id.short(), "ab");
    }

    #[test]
    fn edge_opposite_roundtrips() {
        for e in [
            ScreenEdge::Left,
            ScreenEdge::Right,
            ScreenEdge::Top,
            ScreenEdge::Bottom,
        ] {
            assert_eq!(e.opposite().opposite(), e);
        }
    }

    #[test]
    fn control_msg_roundtrips_through_postcard() {
        let msg = ControlMsg::EdgeEnter { x: 0.25, y: 0.9 };
        let bytes = postcard::to_stdvec(&msg).unwrap();
        let back: ControlMsg = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(msg, back);
    }
}
