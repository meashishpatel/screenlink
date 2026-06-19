//! Extend-mode pipeline architecture (Phase 2).
//!
//! The traits here define the seams the native code plugs into. The data flow:
//!
//! **Host:**  `VirtualDisplay` (add a monitor) → `FrameSource` (Desktop
//! Duplication captures it) → `VideoEncoder` (Media Foundation HW H.264/HEVC) →
//! [`crate::transport`] (chunk + seal + UDP).
//!
//! **Client:** UDP → [`crate::transport`] (reassemble) → `VideoDecoder`
//! (MF/DXVA) → `Presenter` (D3D11 flip-model swapchain).
//!
//! Everything in this module is platform-independent so it compiles and is
//! testable everywhere. The Windows implementations of these traits live behind
//! `#[cfg(all(windows, feature = "extend"))]` and are filled in incrementally —
//! see `docs/phase2-architecture.md`. A pure-Rust [`TestPatternSource`] lets the
//! encode/transport path be exercised without a capture device or the driver.

use crate::DisplayMode;

/// A captured/decoded raw frame in BGRA8 (the Desktop Duplication / D3D11 native
/// format). `stride` is bytes per row (may exceed `width * 4` due to padding).
#[derive(Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub bgra: Vec<u8>,
}

/// A compressed frame ready for [`crate::transport::split_frame`].
#[derive(Clone, Debug)]
pub struct EncodedFrame {
    pub keyframe: bool,
    pub data: Vec<u8>,
}

/// Creates/removes the virtual monitor on the host (IddCx driver, Phase 2).
pub trait VirtualDisplay {
    fn enable(&mut self, mode: DisplayMode) -> anyhow::Result<()>;
    fn disable(&mut self) -> anyhow::Result<()>;
}

/// Produces raw frames (host: Desktop Duplication of the virtual monitor).
pub trait FrameSource {
    /// Returns the next frame, or `None` if none is available yet (no change).
    fn next_frame(&mut self) -> anyhow::Result<Option<Frame>>;
}

/// Compresses raw frames (host: Media Foundation hardware encoder).
pub trait VideoEncoder {
    fn encode(&mut self, frame: &Frame) -> anyhow::Result<EncodedFrame>;
    /// Ask for an IDR/keyframe on the next encode (e.g. after packet loss).
    fn request_keyframe(&mut self);
}

/// Decompresses frames (client: MF/DXVA hardware decoder).
pub trait VideoDecoder {
    fn decode(&mut self, encoded: &EncodedFrame) -> anyhow::Result<Option<Frame>>;
}

/// Presents decoded frames (client: D3D11 flip-model swapchain).
pub trait Presenter {
    fn present(&mut self, frame: &Frame) -> anyhow::Result<()>;
}

/// A pure-Rust animated test pattern, standing in for a real capture source so
/// the encode/transport/decode path can be developed and tested with no driver
/// and no second machine.
pub struct TestPatternSource {
    width: u32,
    height: u32,
    tick: u32,
}

impl TestPatternSource {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            tick: 0,
        }
    }
}

impl FrameSource for TestPatternSource {
    fn next_frame(&mut self) -> anyhow::Result<Option<Frame>> {
        let (w, h) = (self.width, self.height);
        let stride = w * 4;
        let mut bgra = vec![0u8; (stride * h) as usize];
        let t = self.tick;
        for y in 0..h {
            for x in 0..w {
                let i = ((y * stride) + x * 4) as usize;
                bgra[i] = ((x + t) % 256) as u8; // B
                bgra[i + 1] = ((y + t) % 256) as u8; // G
                bgra[i + 2] = ((x ^ y) % 256) as u8; // R
                bgra[i + 3] = 255; // A
            }
        }
        self.tick = self.tick.wrapping_add(4);
        Ok(Some(Frame {
            width: w,
            height: h,
            stride,
            bgra,
        }))
    }
}

/// Placeholder encoder until the Media Foundation hardware MFT is wired up.
pub struct UnimplementedEncoder;
impl VideoEncoder for UnimplementedEncoder {
    fn encode(&mut self, _frame: &Frame) -> anyhow::Result<EncodedFrame> {
        anyhow::bail!("Media Foundation hardware encoder not implemented yet (Phase 2)")
    }
    fn request_keyframe(&mut self) {}
}

/// Placeholder decoder until MF/DXVA decode is wired up.
pub struct UnimplementedDecoder;
impl VideoDecoder for UnimplementedDecoder {
    fn decode(&mut self, _encoded: &EncodedFrame) -> anyhow::Result<Option<Frame>> {
        anyhow::bail!("MF/DXVA decoder not implemented yet (Phase 2)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pattern_produces_correct_sized_bgra() {
        let mut src = TestPatternSource::new(64, 48);
        let f = src.next_frame().unwrap().unwrap();
        assert_eq!(f.width, 64);
        assert_eq!(f.height, 48);
        assert_eq!(f.bgra.len(), (64 * 48 * 4) as usize);
        // Alpha channel is fully opaque.
        assert!(f.bgra.iter().skip(3).step_by(4).all(|&a| a == 255));
    }

    #[test]
    fn test_pattern_animates() {
        let mut src = TestPatternSource::new(8, 8);
        let a = src.next_frame().unwrap().unwrap();
        let b = src.next_frame().unwrap().unwrap();
        assert_ne!(a.bgra, b.bgra, "frames should change over time");
    }
}
