//! LAN peer discovery for ScreenLink.
//!
//! Primary path is mDNS / DNS-SD (`_screenlink._tcp.local.`) via `mdns-sd`,
//! which works zero-config when both devices are on the same subnet. We embed
//! each device's certificate fingerprint and friendly name in TXT records so a
//! discovered peer maps directly onto the trust store.
//!
//! Manual peers (typed IP/port) are merged in for networks where mDNS is blocked,
//! and [`selftest`] explains *why* a peer can't be reached when discovery or
//! connection fails.

pub mod selftest;

use screenlink_core::MDNS_SERVICE_TYPE;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use tracing::{debug, warn};

/// A peer we learned about (via mDNS or manual entry).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredPeer {
    /// Certificate fingerprint advertised in TXT (empty for manual peers until
    /// the TLS handshake reveals it).
    pub fingerprint: String,
    pub name: String,
    pub addrs: Vec<IpAddr>,
    pub port: u16,
    pub source: PeerSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeerSource {
    Mdns,
    Manual,
}

impl DiscoveredPeer {
    /// Best address to dial: prefer IPv4, then any.
    pub fn primary_addr(&self) -> Option<IpAddr> {
        self.addrs
            .iter()
            .find(|a| a.is_ipv4())
            .or_else(|| self.addrs.first())
            .copied()
    }
}

/// Owns the mDNS daemon and an updating view of discovered peers. Manual peers
/// are layered on top by the caller via [`Discovery::set_manual_peers`].
pub struct Discovery {
    #[allow(dead_code)]
    daemon: Option<mdns_sd::ServiceDaemon>,
    our_fingerprint: String,
    mdns_peers: Arc<Mutex<HashMap<String, DiscoveredPeer>>>,
    manual_peers: Arc<Mutex<Vec<DiscoveredPeer>>>,
}

impl Discovery {
    /// Start advertising this device and browsing for peers. Returns a working
    /// [`Discovery`] even if mDNS fails to start (manual peers still function).
    pub fn start(device_name: &str, fingerprint: &str, control_port: u16) -> Self {
        let mdns_peers = Arc::new(Mutex::new(HashMap::new()));
        let manual_peers = Arc::new(Mutex::new(Vec::new()));

        let daemon =
            match Self::start_mdns(device_name, fingerprint, control_port, mdns_peers.clone()) {
                Ok(d) => Some(d),
                Err(e) => {
                    warn!("mDNS unavailable, manual peers only: {e}");
                    None
                }
            };

        Self {
            daemon,
            our_fingerprint: fingerprint.to_string(),
            mdns_peers,
            manual_peers,
        }
    }

    fn start_mdns(
        device_name: &str,
        fingerprint: &str,
        control_port: u16,
        peers: Arc<Mutex<HashMap<String, DiscoveredPeer>>>,
    ) -> anyhow::Result<mdns_sd::ServiceDaemon> {
        let daemon = mdns_sd::ServiceDaemon::new()?;

        // Instance name uses the short fingerprint to stay unique even if two
        // devices share a friendly name.
        let short = &fingerprint[..fingerprint.len().min(12)];
        let instance = format!("{device_name} [{short}]");
        let host = format!("screenlink-{short}.local.");
        let props = [("fp", fingerprint), ("name", device_name), ("ver", "1")];

        let service = mdns_sd::ServiceInfo::new(
            MDNS_SERVICE_TYPE,
            &instance,
            &host,
            "",
            control_port,
            &props[..],
        )?
        .enable_addr_auto();
        daemon.register(service)?;

        let receiver = daemon.browse(MDNS_SERVICE_TYPE)?;
        let our_fp = fingerprint.to_string();
        std::thread::spawn(move || {
            while let Ok(event) = receiver.recv() {
                match event {
                    mdns_sd::ServiceEvent::ServiceResolved(info) => {
                        let fp = info
                            .get_property_val_str("fp")
                            .unwrap_or_default()
                            .to_string();
                        if fp == our_fp {
                            continue; // don't list ourselves
                        }
                        let name = info
                            .get_property_val_str("name")
                            .unwrap_or_else(|| info.get_fullname())
                            .to_string();
                        let addrs: Vec<IpAddr> = info.get_addresses().iter().copied().collect();
                        let peer = DiscoveredPeer {
                            fingerprint: fp.clone(),
                            name,
                            addrs,
                            port: info.get_port(),
                            source: PeerSource::Mdns,
                        };
                        debug!("mDNS resolved peer: {peer:?}");
                        let key = if fp.is_empty() {
                            info.get_fullname().to_string()
                        } else {
                            fp
                        };
                        peers.lock().unwrap().insert(key, peer);
                    }
                    mdns_sd::ServiceEvent::ServiceRemoved(_ty, fullname) => {
                        peers
                            .lock()
                            .unwrap()
                            .retain(|_, p| !fullname.contains(&p.name));
                    }
                    _ => {}
                }
            }
        });

        Ok(daemon)
    }

    /// Replace the manual peer list (e.g. from config / the UI).
    pub fn set_manual_peers(&self, peers: Vec<DiscoveredPeer>) {
        *self.manual_peers.lock().unwrap() = peers;
    }

    /// Current merged view: mDNS peers plus manual peers (manual wins on address
    /// collision so a user override takes effect).
    pub fn peers(&self) -> Vec<DiscoveredPeer> {
        let mut out: Vec<DiscoveredPeer> =
            self.mdns_peers.lock().unwrap().values().cloned().collect();
        for m in self.manual_peers.lock().unwrap().iter() {
            if !out.iter().any(|p| p.addrs == m.addrs && p.port == m.port) {
                out.push(m.clone());
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    pub fn our_fingerprint(&self) -> &str {
        &self.our_fingerprint
    }
}
