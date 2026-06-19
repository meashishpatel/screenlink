//! Numeric-comparison pairing (Bluetooth-style).
//!
//! No PIN is sent over the wire. Both devices independently derive the same
//! 6-digit code from the two certificate fingerprints they exchanged during the
//! TLS handshake. The user confirms the two screens show the same number, which
//! authenticates the channel: a man-in-the-middle would terminate two different
//! TLS sessions with two different fingerprints, producing two different codes —
//! the user would see them disagree and cancel.

use sha2::{Digest, Sha256};

/// Derive the 6-digit comparison code from both fingerprints. Order-independent.
pub fn comparison_pin(fp_a: &str, fp_b: &str) -> String {
    let (lo, hi) = if fp_a <= fp_b {
        (fp_a, fp_b)
    } else {
        (fp_b, fp_a)
    };
    let mut h = Sha256::new();
    h.update(b"screenlink-pair-v1");
    h.update(lo.as_bytes());
    h.update([0u8]); // domain separator between the two fields
    h.update(hi.as_bytes());
    let d = h.finalize();
    let n = u32::from_be_bytes([d[0], d[1], d[2], d[3]]) % 1_000_000;
    format!("{n:06}")
}

/// Tracks the two-sided confirmation needed to complete pairing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PairingState {
    pub local_confirmed: bool,
    pub remote_confirmed: bool,
}

impl PairingState {
    pub fn confirm_local(&mut self) {
        self.local_confirmed = true;
    }
    pub fn confirm_remote(&mut self) {
        self.remote_confirmed = true;
    }
    /// True once both sides have confirmed the codes match.
    pub fn is_complete(&self) -> bool {
        self.local_confirmed && self.remote_confirmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_is_six_digits() {
        let pin = comparison_pin("aa", "bb");
        assert_eq!(pin.len(), 6);
        assert!(pin.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn pin_is_order_independent() {
        assert_eq!(comparison_pin("abc", "xyz"), comparison_pin("xyz", "abc"));
    }

    #[test]
    fn pin_differs_for_different_pairs() {
        // Overwhelmingly likely to differ; this guards against a constant bug.
        assert_ne!(comparison_pin("aaa", "bbb"), comparison_pin("aaa", "ccc"));
    }

    #[test]
    fn pairing_completes_only_when_both_confirm() {
        let mut s = PairingState::default();
        assert!(!s.is_complete());
        s.confirm_local();
        assert!(!s.is_complete());
        s.confirm_remote();
        assert!(s.is_complete());
    }
}
