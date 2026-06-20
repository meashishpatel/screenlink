//! Screen mirror: stream this machine's screen to a peer (sender) and decode +
//! display a peer's screen (receiver), over a dedicated encrypted UDP video
//! channel.
//!
//! The video channel uses a key *derived* from (but independent of) the input
//! channel's key, so the two streams never share a (key, nonce) pair.
//!
//! Without `--features extend`, the sender streams an animated **test pattern**
//! so the whole pipeline (network → decode → display) can be verified without the
//! capture driver; with it, the sender streams the real screen via Desktop
//! Duplication.

use screenlink_core::realtime::RealtimeCrypto;
use screenlink_core::DEFAULT_VIDEO_PORT;
use screenlink_video::codec::{JpegDecoder, JpegEncoder};
use screenlink_video::pipeline::{EncodedFrame, FrameSource, VideoDecoder, VideoEncoder};
use screenlink_video::transport::{split_frame, Reassembler, VideoChunk, DEFAULT_MAX_CHUNK};
use std::net::{IpAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Latest decoded frame for display: RGBA8, top-down, tightly packed.
pub struct RgbaFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Shared slot holding the most recent frame to display (or `None`).
pub type FrameSlot = Arc<Mutex<Option<RgbaFrame>>>;

/// Stream this machine's screen to `peer_ip`'s video port until `stop` is set.
pub fn spawn_sender(peer_ip: IpAddr, video_key: [u8; 32], epoch: u64, stop: Arc<AtomicBool>) {
    std::thread::Builder::new()
        .name("screenlink-mirror-tx".into())
        .spawn(move || {
            let udp = match UdpSocket::bind(("0.0.0.0", 0)) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("mirror sender bind failed: {e}");
                    return;
                }
            };
            if let Err(e) = udp.connect((peer_ip, DEFAULT_VIDEO_PORT)) {
                tracing::error!("mirror sender connect failed: {e}");
                return;
            }
            let mut source = match make_source() {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("screen capture unavailable: {e}");
                    return;
                }
            };
            let mut rt = RealtimeCrypto::new(video_key, epoch, true);
            let mut enc = JpegEncoder::default();
            let mut frame_id: u32 = 0;
            tracing::info!("mirror sender streaming to {peer_ip}:{DEFAULT_VIDEO_PORT}");

            while !stop.load(Ordering::Relaxed) {
                match source.next_frame() {
                    Ok(Some(frame)) => match enc.encode(&frame) {
                        Ok(encoded) => {
                            for chunk in split_frame(
                                frame_id,
                                encoded.keyframe,
                                &encoded.data,
                                DEFAULT_MAX_CHUNK,
                            ) {
                                if let Ok(pt) = postcard::to_stdvec(&chunk) {
                                    if let Ok(pkt) = rt.seal(&pt) {
                                        let _ = udp.send(&pkt);
                                    }
                                }
                            }
                            frame_id = frame_id.wrapping_add(1);
                        }
                        Err(e) => tracing::warn!("encode failed: {e}"),
                    },
                    Ok(None) => {} // no new frame this tick
                    Err(e) => {
                        tracing::warn!("capture ended: {e}");
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(50)); // ~20 fps cap
            }
            tracing::info!("mirror sender stopped");
        })
        .expect("spawn mirror sender");
}

/// Receive + decode a peer's screen into `slot` for display, until `stop` is set.
pub fn spawn_receiver(video_key: [u8; 32], epoch: u64, slot: FrameSlot, stop: Arc<AtomicBool>) {
    std::thread::Builder::new()
        .name("screenlink-mirror-rx".into())
        .spawn(move || {
            let sock = match UdpSocket::bind(("0.0.0.0", DEFAULT_VIDEO_PORT)) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("mirror receiver bind on :{DEFAULT_VIDEO_PORT} failed: {e}");
                    return;
                }
            };
            sock.set_read_timeout(Some(Duration::from_millis(250))).ok();
            let mut rt = RealtimeCrypto::new(video_key, epoch, false);
            let mut re = Reassembler::new();
            let mut dec = JpegDecoder;
            let mut buf = vec![0u8; 2048];
            tracing::info!("mirror receiver listening on :{DEFAULT_VIDEO_PORT}");

            while !stop.load(Ordering::Relaxed) {
                let n = match sock.recv_from(&mut buf) {
                    Ok((n, _)) => n,
                    Err(ref e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        continue
                    }
                    Err(e) => {
                        tracing::warn!("mirror receiver recv error: {e}");
                        continue;
                    }
                };
                let pt = match rt.open(&buf[..n]) {
                    Ok((_, pt)) => pt,
                    Err(_) => continue,
                };
                let chunk = match postcard::from_bytes::<VideoChunk>(&pt) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                if let Some((_, payload)) = re.push(chunk) {
                    if let Ok(Some(frame)) = dec.decode(&EncodedFrame {
                        keyframe: true,
                        data: payload,
                    }) {
                        let mut rgba = frame.bgra;
                        for px in rgba.chunks_exact_mut(4) {
                            px.swap(0, 2); // BGRA -> RGBA
                        }
                        *slot.lock().unwrap() = Some(RgbaFrame {
                            width: frame.width,
                            height: frame.height,
                            rgba,
                        });
                    }
                }
            }
            *slot.lock().unwrap() = None;
            tracing::info!("mirror receiver stopped");
        })
        .expect("spawn mirror receiver");
}

#[cfg(all(windows, feature = "extend"))]
fn make_source() -> anyhow::Result<Box<dyn FrameSource>> {
    Ok(Box::new(
        screenlink_video::capture::DesktopDuplicationSource::new()?,
    ))
}

#[cfg(not(all(windows, feature = "extend")))]
fn make_source() -> anyhow::Result<Box<dyn FrameSource>> {
    // No capture driver in this build: stream an animated test pattern so the
    // mirror path can be verified end-to-end (you'll see a moving gradient).
    Ok(Box::new(
        screenlink_video::pipeline::TestPatternSource::new(960, 540),
    ))
}
