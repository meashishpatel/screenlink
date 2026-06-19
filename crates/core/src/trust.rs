//! Persistent set of paired (trusted) devices, keyed by certificate fingerprint.

use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrustedDevice {
    pub fingerprint: String,
    pub name: String,
}

#[derive(Default, Serialize, Deserialize)]
struct TrustData {
    devices: HashMap<String, TrustedDevice>,
}

/// Thread-safe, file-backed trust store. The TLS verifier reads it on the
/// handshake path, so reads must be cheap and lock-friendly.
pub struct TrustStore {
    inner: RwLock<TrustData>,
    path: Option<PathBuf>,
}

impl TrustStore {
    pub fn in_memory() -> Self {
        Self {
            inner: RwLock::new(TrustData::default()),
            path: None,
        }
    }

    /// Load from `path`, or start empty (still bound to `path` for later saves).
    pub fn load(path: &Path) -> Result<Self> {
        let data = if path.exists() {
            let bytes = std::fs::read(path)?;
            postcard::from_bytes(&bytes).unwrap_or_default()
        } else {
            TrustData::default()
        };
        Ok(Self {
            inner: RwLock::new(data),
            path: Some(path.to_path_buf()),
        })
    }

    pub fn is_trusted(&self, fingerprint: &str) -> bool {
        self.inner.read().unwrap().devices.contains_key(fingerprint)
    }

    pub fn get(&self, fingerprint: &str) -> Option<TrustedDevice> {
        self.inner.read().unwrap().devices.get(fingerprint).cloned()
    }

    /// Add or update a trusted device, then persist.
    pub fn trust(&self, fingerprint: &str, name: &str) -> Result<()> {
        {
            let mut g = self.inner.write().unwrap();
            g.devices.insert(
                fingerprint.to_string(),
                TrustedDevice {
                    fingerprint: fingerprint.to_string(),
                    name: name.to_string(),
                },
            );
        }
        self.save()
    }

    /// Remove a device (unpair), then persist.
    pub fn revoke(&self, fingerprint: &str) -> Result<()> {
        {
            let mut g = self.inner.write().unwrap();
            g.devices.remove(fingerprint);
        }
        self.save()
    }

    pub fn list(&self) -> Vec<TrustedDevice> {
        self.inner
            .read()
            .unwrap()
            .devices
            .values()
            .cloned()
            .collect()
    }

    fn save(&self) -> Result<()> {
        if let Some(path) = &self.path {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let bytes = postcard::to_stdvec(&*self.inner.read().unwrap())?;
            std::fs::write(path, bytes)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trust_revoke_roundtrip() {
        let s = TrustStore::in_memory();
        assert!(!s.is_trusted("aa"));
        s.trust("aa", "Laptop A").unwrap();
        assert!(s.is_trusted("aa"));
        assert_eq!(s.get("aa").unwrap().name, "Laptop A");
        assert_eq!(s.list().len(), 1);
        s.revoke("aa").unwrap();
        assert!(!s.is_trusted("aa"));
    }
}
