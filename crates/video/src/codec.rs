//! Software video codec for the screen-mirror MVP: per-frame JPEG.
//!
//! JPEG is simple, pure-Rust, and every frame is independent (so packet loss
//! just drops a frame instead of corrupting a stream). It's heavier on bandwidth
//! than H.264, so it's the MVP path — a hardware Media Foundation encoder can
//! replace it later behind the same [`VideoEncoder`] / [`VideoDecoder`] traits.

use crate::pipeline::{EncodedFrame, Frame, VideoDecoder, VideoEncoder};
use image::codecs::jpeg::JpegEncoder as ImgJpegEncoder;
use image::{ExtendedColorType, ImageFormat};

/// JPEG encoder. `quality` is 1..=100 (higher = better image, more bytes).
pub struct JpegEncoder {
    pub quality: u8,
}

impl Default for JpegEncoder {
    fn default() -> Self {
        Self { quality: 70 }
    }
}

impl VideoEncoder for JpegEncoder {
    fn encode(&mut self, frame: &Frame) -> anyhow::Result<EncodedFrame> {
        let (w, h) = (frame.width, frame.height);
        let stride = frame.stride as usize;
        let row_bytes = w as usize * 4;
        anyhow::ensure!(
            frame.bgra.len() >= stride * h as usize && stride >= row_bytes,
            "frame buffer smaller than width*height"
        );
        // BGRA (with stride/padding) -> tightly packed RGB for the encoder.
        let mut rgb = Vec::with_capacity(w as usize * h as usize * 3);
        for y in 0..h as usize {
            let row = &frame.bgra[y * stride..y * stride + row_bytes];
            for px in row.chunks_exact(4) {
                rgb.push(px[2]); // R
                rgb.push(px[1]); // G
                rgb.push(px[0]); // B
            }
        }
        let mut data = Vec::new();
        ImgJpegEncoder::new_with_quality(&mut data, self.quality).encode(
            &rgb,
            w,
            h,
            ExtendedColorType::Rgb8,
        )?;
        Ok(EncodedFrame {
            keyframe: true,
            data,
        })
    }

    fn request_keyframe(&mut self) {
        // Every JPEG frame is already a keyframe.
    }
}

/// JPEG decoder, producing BGRA frames (the native capture/present format).
#[derive(Default)]
pub struct JpegDecoder;

impl VideoDecoder for JpegDecoder {
    fn decode(&mut self, encoded: &EncodedFrame) -> anyhow::Result<Option<Frame>> {
        let img = image::load_from_memory_with_format(&encoded.data, ImageFormat::Jpeg)?.to_rgba8();
        let (w, h) = img.dimensions();
        let mut data = img.into_raw(); // RGBA
        for px in data.chunks_exact_mut(4) {
            px.swap(0, 2); // RGBA -> BGRA
        }
        Ok(Some(Frame {
            width: w,
            height: h,
            stride: w * 4,
            bgra: data,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{FrameSource, TestPatternSource};
    use crate::transport::{split_frame, Reassembler, DEFAULT_MAX_CHUNK};

    #[test]
    fn encode_then_decode_preserves_dimensions() {
        let mut src = TestPatternSource::new(160, 120);
        let frame = src.next_frame().unwrap().unwrap();
        let mut enc = JpegEncoder::default();
        let encoded = enc.encode(&frame).unwrap();
        assert!(!encoded.data.is_empty());

        let mut dec = JpegDecoder;
        let out = dec.decode(&encoded).unwrap().unwrap();
        assert_eq!((out.width, out.height), (160, 120));
        assert_eq!(out.bgra.len(), 160 * 120 * 4);
    }

    /// The whole pipeline end-to-end with no OS/driver: capture (test pattern) →
    /// JPEG encode → chunk + reassemble (transport) → JPEG decode → frame.
    #[test]
    fn full_pipeline_capture_encode_transport_decode() {
        let mut src = TestPatternSource::new(320, 240);
        let frame = src.next_frame().unwrap().unwrap();

        let encoded = JpegEncoder::default().encode(&frame).unwrap();

        // Transport: split into UDP-sized chunks and reassemble.
        let chunks = split_frame(0, encoded.keyframe, &encoded.data, DEFAULT_MAX_CHUNK);
        assert!(
            chunks.len() > 1,
            "a 320x240 JPEG should span multiple chunks"
        );
        let mut re = Reassembler::new();
        let mut reassembled = None;
        for c in chunks {
            if let Some(out) = re.push(c) {
                reassembled = Some(out);
            }
        }
        let (keyframe, payload) = reassembled.expect("frame reassembled");
        assert!(keyframe);
        assert_eq!(
            payload, encoded.data,
            "transport must reassemble byte-for-byte"
        );

        let out = JpegDecoder
            .decode(&EncodedFrame {
                keyframe,
                data: payload,
            })
            .unwrap()
            .unwrap();
        assert_eq!((out.width, out.height), (320, 240));
    }
}
