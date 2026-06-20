//! `screenlink-core` — shared protocol types, transport framing, TLS security,
//! pairing, and the encrypted realtime channel for ScreenLink.
//!
//! This crate has no dependency on the OS-specific feature crates (input,
//! clipboard, video). They depend on it. Keep the dependency arrow pointing one
//! way so Phase 2 plugs in without rewrites.

pub mod config;
pub mod error;
pub mod framing;
pub mod pairing;
pub mod protocol;
pub mod realtime;
pub mod security;
pub mod trust;

pub use error::{Error, Result};
pub use protocol::{
    Capabilities, ControlMsg, DeviceId, DeviceInfo, InputEvent, Key, Mode, MouseButton, ScreenEdge,
    PROTOCOL_VERSION,
};

/// The mDNS / DNS-SD service type ScreenLink advertises and browses for.
pub const MDNS_SERVICE_TYPE: &str = "_screenlink._tcp.local.";

/// Default TCP port for the TLS control channel.
pub const DEFAULT_CONTROL_PORT: u16 = 47820;

/// Default UDP port for the encrypted realtime channel (input events).
pub const DEFAULT_REALTIME_PORT: u16 = 47821;

/// Default UDP port for the encrypted video channel (Extend/Mirror mode).
pub const DEFAULT_VIDEO_PORT: u16 = 47822;

/// Ensure a process-wide rustls crypto provider is installed (ring).
///
/// Safe to call multiple times; subsequent calls are no-ops. Call once early in
/// `main`, and defensively from tests.
pub fn init_crypto() {
    // Ignore the error: it only fails if a provider is already installed, which
    // is exactly the state we want.
    let _ = rustls::crypto::ring::default_provider().install_default();
}
