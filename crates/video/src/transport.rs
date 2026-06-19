//! Video packet transport: split an encoded frame into UDP-sized chunks and
//! reassemble them on the other side.
//!
//! Each `VideoChunk` is serialized with `postcard` and then sealed by
//! `screenlink_core::realtime::RealtimeCrypto` (same encrypted UDP channel used
//! for input), so this layer deals only in plaintext framing. It is fully
//! platform-independent and unit-tested — the native capture/encode code feeds
//! it and the decode/present code consumes it.

use serde::{Deserialize, Serialize};

/// Target max payload per chunk. Kept comfortably under a 1200-byte safe UDP MTU
/// after AEAD (16-byte tag + 8-byte seq) and postcard overhead.
pub const DEFAULT_MAX_CHUNK: usize = 1100;

/// One fragment of an encoded video frame.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct VideoChunk {
    pub frame_id: u32,
    pub index: u16,
    pub count: u16,
    pub keyframe: bool,
    pub data: Vec<u8>,
}

/// Split an encoded frame into chunks of at most `max_chunk` payload bytes.
pub fn split_frame(
    frame_id: u32,
    keyframe: bool,
    payload: &[u8],
    max_chunk: usize,
) -> Vec<VideoChunk> {
    let max_chunk = max_chunk.max(1);
    let slices: Vec<&[u8]> = if payload.is_empty() {
        vec![&[][..]]
    } else {
        payload.chunks(max_chunk).collect()
    };
    let count = slices.len() as u16;
    slices
        .into_iter()
        .enumerate()
        .map(|(i, c)| VideoChunk {
            frame_id,
            index: i as u16,
            count,
            keyframe,
            data: c.to_vec(),
        })
        .collect()
}

/// Reassembles chunks into frames, tolerating reordering within a frame and
/// dropping incomplete frames when a newer one starts.
#[derive(Default)]
pub struct Reassembler {
    frame_id: Option<u32>,
    count: u16,
    received: usize,
    keyframe: bool,
    parts: Vec<Option<Vec<u8>>>,
}

impl Reassembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk. Returns `Some((keyframe, payload))` when a frame completes.
    pub fn push(&mut self, chunk: VideoChunk) -> Option<(bool, Vec<u8>)> {
        match self.frame_id {
            Some(id) if id == chunk.frame_id => {}
            Some(id) if is_newer(chunk.frame_id, id) => {
                self.reset(chunk.frame_id, chunk.count, chunk.keyframe)
            }
            Some(_) => return None, // stale chunk from an older frame; ignore
            None => self.reset(chunk.frame_id, chunk.count, chunk.keyframe),
        }

        let idx = chunk.index as usize;
        if idx >= self.parts.len() {
            return None; // malformed (count disagreement)
        }
        if self.parts[idx].is_none() {
            self.parts[idx] = Some(chunk.data);
            self.received += 1;
        }

        if self.received == self.count as usize && self.count > 0 {
            let mut out = Vec::new();
            for p in &self.parts {
                out.extend_from_slice(p.as_ref().expect("all parts present"));
            }
            let kf = self.keyframe;
            self.frame_id = None; // ready for the next frame
            return Some((kf, out));
        }
        None
    }

    fn reset(&mut self, frame_id: u32, count: u16, keyframe: bool) {
        self.frame_id = Some(frame_id);
        self.count = count;
        self.received = 0;
        self.keyframe = keyframe;
        self.parts = vec![None; count as usize];
    }
}

/// True if `a` is newer than `b` accounting for u32 wraparound.
fn is_newer(a: u32, b: u32) -> bool {
    a != b && a.wrapping_sub(b) < u32::MAX / 2
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn split_then_reassemble_roundtrips() {
        let data = payload(5000);
        let chunks = split_frame(1, true, &data, 1100);
        assert!(chunks.len() >= 5);
        let mut r = Reassembler::new();
        let mut done = None;
        for c in chunks {
            if let Some(out) = r.push(c) {
                done = Some(out);
            }
        }
        let (kf, out) = done.expect("frame completed");
        assert!(kf);
        assert_eq!(out, data);
    }

    #[test]
    fn reassembles_out_of_order() {
        let data = payload(3000);
        let mut chunks = split_frame(7, false, &data, 1000);
        chunks.reverse();
        let mut r = Reassembler::new();
        let mut got = None;
        for c in chunks {
            if let Some(out) = r.push(c) {
                got = Some(out);
            }
        }
        assert_eq!(got.unwrap().1, data);
    }

    #[test]
    fn missing_chunk_never_completes() {
        let data = payload(3000);
        let mut chunks = split_frame(2, false, &data, 1000);
        chunks.remove(1); // drop a middle chunk
        let mut r = Reassembler::new();
        let mut completed = false;
        for c in chunks {
            if r.push(c).is_some() {
                completed = true;
            }
        }
        assert!(!completed, "a frame with a missing chunk must not complete");
    }

    #[test]
    fn newer_frame_supersedes_incomplete_one() {
        let f1 = split_frame(10, true, &payload(3000), 1000); // 3 chunks
        let f2 = split_frame(11, false, &payload(1500), 1000); // 2 chunks
        let mut r = Reassembler::new();
        // Only deliver part of frame 10, then all of frame 11.
        assert!(r.push(f1[0].clone()).is_none());
        let mut out = None;
        for c in f2 {
            if let Some(o) = r.push(c) {
                out = Some(o);
            }
        }
        assert!(
            out.is_some(),
            "frame 11 should complete despite partial frame 10"
        );
        assert_eq!(out.unwrap().1.len(), 1500);
    }

    #[test]
    fn empty_frame_is_one_chunk() {
        let chunks = split_frame(1, true, &[], 1000);
        assert_eq!(chunks.len(), 1);
        let mut r = Reassembler::new();
        let (_, out) = r.push(chunks[0].clone()).unwrap();
        assert!(out.is_empty());
    }
}
