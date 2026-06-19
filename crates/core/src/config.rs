//! Persistent application configuration: this device's name, ports, the
//! snap-home hotkey, and the screen-arrangement / mode for each paired peer.

use crate::error::Result;
use crate::protocol::{Mode, ScreenEdge};
use crate::{DEFAULT_CONTROL_PORT, DEFAULT_REALTIME_PORT};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// How a paired peer is arranged relative to this device, and its chosen mode.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PeerConfig {
    pub fingerprint: String,
    pub name: String,
    /// Which edge of this host's screen the peer sits past.
    pub edge: ScreenEdge,
    /// Position along the shared edge, 0.0..=1.0 (e.g. how far down a side edge).
    pub offset: f32,
    pub mode: Mode,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AppConfig {
    pub device_name: String,
    pub control_port: u16,
    pub realtime_port: u16,
    /// Human-readable hotkey that yanks control back to this machine.
    pub snap_home_hotkey: String,
    pub peers: Vec<PeerConfig>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            device_name: default_device_name(),
            control_port: DEFAULT_CONTROL_PORT,
            realtime_port: DEFAULT_REALTIME_PORT,
            snap_home_hotkey: "Ctrl+Alt+Home".to_string(),
            peers: Vec::new(),
        }
    }
}

impl AppConfig {
    pub fn load_or_default(path: &std::path::Path) -> Self {
        if path.exists() {
            if let Ok(bytes) = std::fs::read(path) {
                if let Ok(cfg) = postcard::from_bytes::<AppConfig>(&bytes) {
                    return cfg;
                }
            }
        }
        AppConfig::default()
    }

    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, postcard::to_stdvec(self)?)?;
        Ok(())
    }

    pub fn peer(&self, fingerprint: &str) -> Option<&PeerConfig> {
        self.peers.iter().find(|p| p.fingerprint == fingerprint)
    }

    pub fn peer_mut(&mut self, fingerprint: &str) -> Option<&mut PeerConfig> {
        self.peers.iter_mut().find(|p| p.fingerprint == fingerprint)
    }

    /// Insert or update a peer's arrangement entry.
    pub fn upsert_peer(&mut self, peer: PeerConfig) {
        if let Some(existing) = self.peer_mut(&peer.fingerprint) {
            *existing = peer;
        } else {
            self.peers.push(peer);
        }
    }
}

/// Best-effort friendly device name from the OS hostname.
pub fn default_device_name() -> String {
    std::env::var("COMPUTERNAME")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "ScreenLink device".to_string())
}

/// Per-user directory for config, identity, trust store, and logs.
pub fn data_dir() -> PathBuf {
    if let Some(pd) = ProjectDirs::from("io", "ScreenLink", "ScreenLink") {
        pd.data_local_dir().to_path_buf()
    } else {
        std::env::temp_dir().join("ScreenLink")
    }
}

pub fn config_path() -> PathBuf {
    data_dir().join("config.bin")
}

pub fn trust_path() -> PathBuf {
    data_dir().join("trust.bin")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_replaces_existing_peer() {
        let mut cfg = AppConfig::default();
        cfg.upsert_peer(PeerConfig {
            fingerprint: "fp1".into(),
            name: "A".into(),
            edge: ScreenEdge::Right,
            offset: 0.0,
            mode: Mode::Control,
        });
        cfg.upsert_peer(PeerConfig {
            fingerprint: "fp1".into(),
            name: "A renamed".into(),
            edge: ScreenEdge::Left,
            offset: 0.5,
            mode: Mode::Extend,
        });
        assert_eq!(cfg.peers.len(), 1);
        let p = cfg.peer("fp1").unwrap();
        assert_eq!(p.name, "A renamed");
        assert_eq!(p.edge, ScreenEdge::Left);
        assert_eq!(p.mode, Mode::Extend);
    }

    #[test]
    fn config_roundtrips_through_postcard() {
        let cfg = AppConfig::default();
        let bytes = postcard::to_stdvec(&cfg).unwrap();
        let back: AppConfig = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(cfg, back);
    }
}
