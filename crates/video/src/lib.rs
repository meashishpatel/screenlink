//! Phase 2 — Extend mode (wireless second monitor). **Feature-gated, stubbed.**
//!
//! This crate is intentionally a thin, compiling stub so Phase 1 ships cleanly.
//! The real pipeline (host side):
//!
//! 1. Enable an IddCx **virtual display** (an existing signed driver — see
//!    `driver/README.md`; we drive it rather than ship our own unsigned one).
//! 2. **Desktop Duplication** (DXGI Output Duplication) captures that display.
//! 3. **Media Foundation** hardware MFTs encode H.264/HEVC (Intel QSV / AMD AMF /
//!    NVIDIA NVENC, no vendor SDK).
//! 4. Packetize → encrypted UDP (reusing `screenlink-core::realtime`).
//!
//! Client side: depacketize → MF/DXVA hardware decode → D3D11 flip-model present,
//! with frame pacing and adaptive bitrate from measured RTT/loss.
//!
//! Build with `--features extend` to compile the (still-stub) pipeline entry
//! points; default builds omit it entirely.

#[cfg(all(windows, feature = "extend"))]
pub mod capture;
pub mod codec;
pub mod pipeline;
pub mod transport;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum VideoError {
    #[error("Extend mode is not built into this binary (rebuild with --features extend)")]
    NotBuilt,
    #[error("Extend mode is not implemented yet (Phase 2): {0}")]
    NotImplemented(&'static str),
}

/// Whether this binary was compiled with Extend-mode support.
pub const fn extend_compiled_in() -> bool {
    cfg!(feature = "extend")
}

/// Parameters for a virtual display / stream. Stable enough for the UI to use now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayMode {
    pub width: u32,
    pub height: u32,
    pub refresh_hz: u32,
}

impl Default for DisplayMode {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            refresh_hz: 60,
        }
    }
}

/// Host: begin presenting an extended display to a connected client.
pub fn start_extend_host(_mode: DisplayMode) -> Result<(), VideoError> {
    if !extend_compiled_in() {
        return Err(VideoError::NotBuilt);
    }
    Err(VideoError::NotImplemented(
        "virtual display + Desktop Duplication + Media Foundation encode",
    ))
}

/// Client: begin receiving and presenting an extended display from a host.
pub fn start_extend_client() -> Result<(), VideoError> {
    if !extend_compiled_in() {
        return Err(VideoError::NotBuilt);
    }
    Err(VideoError::NotImplemented("MF/DXVA decode + D3D11 present"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_start_is_gated_or_unimplemented() {
        // Either way it must be an Err in Phase 1 — never a silent success.
        assert!(start_extend_host(DisplayMode::default()).is_err());
    }
}
