//! Encrypted realtime channel (UDP) crypto.
//!
//! The realtime channel carries input events (Phase 1) and, later, video
//! packets (Phase 2). It is unreliable by design — low latency beats
//! reliability for cursor/keys. We add:
//!
//! * AEAD confidentiality + integrity with ChaCha20-Poly1305, using a 32-byte
//!   key exported from the established TLS session (so the UDP key is bound to
//!   the authenticated control channel and never sent on the wire).
//! * An 8-byte big-endian sequence number per packet (cleartext header) used to
//!   form the nonce and to detect reordering / replays.
//! * A sliding replay window so an attacker can't re-inject captured packets.
//!
//! Each direction uses a distinct 4-byte nonce salt, so the two directions never
//! reuse a (key, nonce) pair even though they share the exported key.

use crate::error::{Error, Result};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};

const SEQ_LEN: usize = 8;
const SALT_INITIATOR: [u8; 4] = [b'S', b'L', 0x01, 0x00];
const SALT_RESPONDER: [u8; 4] = [b'S', b'L', 0x02, 0x00];

/// Sealing/opening context for one peer connection's realtime channel.
pub struct RealtimeCrypto {
    cipher: ChaCha20Poly1305,
    send_salt: [u8; 4],
    recv_salt: [u8; 4],
    send_seq: u64,
    replay: ReplayWindow,
    aad: [u8; 8],
}

impl RealtimeCrypto {
    /// `key` is the 32 bytes exported from TLS keying material. `is_initiator`
    /// must be `true` on exactly one side (the host/connector) and `false` on the
    /// other, so the two directions get different nonce salts.
    pub fn new(key: [u8; 32], epoch: u64, is_initiator: bool) -> Self {
        let cipher = ChaCha20Poly1305::new_from_slice(&key).expect("32-byte key");
        let (send_salt, recv_salt) = if is_initiator {
            (SALT_INITIATOR, SALT_RESPONDER)
        } else {
            (SALT_RESPONDER, SALT_INITIATOR)
        };
        Self {
            cipher,
            send_salt,
            recv_salt,
            send_seq: 0,
            replay: ReplayWindow::default(),
            aad: epoch.to_be_bytes(),
        }
    }

    fn nonce(salt: [u8; 4], seq: u64) -> Nonce {
        let mut n = [0u8; 12];
        n[..4].copy_from_slice(&salt);
        n[4..].copy_from_slice(&seq.to_be_bytes());
        *Nonce::from_slice(&n)
    }

    /// Seal a plaintext payload into a wire packet: `seq(8) || ciphertext||tag`.
    pub fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let seq = self.send_seq;
        self.send_seq = self.send_seq.wrapping_add(1);
        let nonce = Self::nonce(self.send_salt, seq);
        let ct = self
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext,
                    aad: &self.aad,
                },
            )
            .map_err(|e| Error::Crypto(e.to_string()))?;
        let mut out = Vec::with_capacity(SEQ_LEN + ct.len());
        out.extend_from_slice(&seq.to_be_bytes());
        out.extend_from_slice(&ct);
        Ok(out)
    }

    /// Open a wire packet, verifying integrity and rejecting replays/old packets.
    /// Returns the sequence number and decrypted payload.
    pub fn open(&mut self, packet: &[u8]) -> Result<(u64, Vec<u8>)> {
        if packet.len() < SEQ_LEN {
            return Err(Error::Crypto("short packet".into()));
        }
        let mut seq_bytes = [0u8; SEQ_LEN];
        seq_bytes.copy_from_slice(&packet[..SEQ_LEN]);
        let seq = u64::from_be_bytes(seq_bytes);
        let nonce = Self::nonce(self.recv_salt, seq);
        let pt = self
            .cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: &packet[SEQ_LEN..],
                    aad: &self.aad,
                },
            )
            .map_err(|e| Error::Crypto(format!("decrypt/auth failed: {e}")))?;
        if !self.replay.accept(seq) {
            return Err(Error::Crypto(format!(
                "replayed or too-old packet seq={seq}"
            )));
        }
        Ok((seq, pt))
    }
}

/// Anti-replay sliding window (RFC 3711-style) over a 64-packet window.
#[derive(Default)]
pub struct ReplayWindow {
    highest: u64,
    bitmap: u64,
    seen_any: bool,
}

impl ReplayWindow {
    const WIDTH: u64 = 64;

    /// Record `seq`; return `false` if it's a replay or older than the window.
    pub fn accept(&mut self, seq: u64) -> bool {
        if !self.seen_any {
            self.seen_any = true;
            self.highest = seq;
            self.bitmap = 1;
            return true;
        }
        if seq > self.highest {
            let shift = seq - self.highest;
            if shift >= Self::WIDTH {
                self.bitmap = 1;
            } else {
                self.bitmap = (self.bitmap << shift) | 1;
            }
            self.highest = seq;
            true
        } else {
            let diff = self.highest - seq;
            if diff >= Self::WIDTH {
                return false; // too old
            }
            let mask = 1u64 << diff;
            if self.bitmap & mask != 0 {
                false // already seen
            } else {
                self.bitmap |= mask;
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> (RealtimeCrypto, RealtimeCrypto) {
        let key = [7u8; 32];
        (
            RealtimeCrypto::new(key, 1, true),
            RealtimeCrypto::new(key, 1, false),
        )
    }

    #[test]
    fn seal_open_roundtrip() {
        let (mut host, mut client) = pair();
        let pkt = host.seal(b"hello input").unwrap();
        let (seq, pt) = client.open(&pkt).unwrap();
        assert_eq!(seq, 0);
        assert_eq!(pt, b"hello input");
    }

    #[test]
    fn tampered_packet_is_rejected() {
        let (mut host, mut client) = pair();
        let mut pkt = host.seal(b"abc").unwrap();
        let last = pkt.len() - 1;
        pkt[last] ^= 0xff;
        assert!(client.open(&pkt).is_err());
    }

    #[test]
    fn replay_is_rejected() {
        let (mut host, mut client) = pair();
        let pkt = host.seal(b"x").unwrap();
        assert!(client.open(&pkt).is_ok());
        assert!(
            client.open(&pkt).is_err(),
            "second delivery must be rejected"
        );
    }

    #[test]
    fn wrong_epoch_fails_auth() {
        let key = [9u8; 32];
        let mut host = RealtimeCrypto::new(key, 1, true);
        let mut client = RealtimeCrypto::new(key, 2, false); // different epoch (AAD)
        let pkt = host.seal(b"x").unwrap();
        assert!(client.open(&pkt).is_err());
    }

    #[test]
    fn replay_window_accepts_reordering_within_window() {
        let mut w = ReplayWindow::default();
        assert!(w.accept(10));
        assert!(w.accept(12));
        assert!(w.accept(11)); // late but within window
        assert!(!w.accept(12)); // duplicate
        assert!(!w.accept(10)); // duplicate
    }

    #[test]
    fn replay_window_rejects_ancient() {
        let mut w = ReplayWindow::default();
        assert!(w.accept(1000));
        assert!(!w.accept(1)); // far older than window
    }
}
