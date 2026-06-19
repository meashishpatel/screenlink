//! Connectivity self-test: turn "it doesn't work" into an actionable diagnosis.
//!
//! "Same Wi-Fi" doesn't guarantee reachability. This probes a target and reports
//! the most likely cause so the UI can tell the user what to fix.

use std::net::{IpAddr, SocketAddr, TcpStream, UdpSocket};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// TCP connection to the control port succeeded.
    Ok,
    /// Host answered but the port is closed — peer app not running, or a firewall
    /// is rejecting (RST) the control port.
    PortClosed,
    /// No answer at all within the timeout — most often AP/client isolation or a
    /// firewall silently dropping packets.
    Unreachable,
    /// The OS reports the network/host as unreachable — likely a different
    /// subnet/VLAN with no route.
    NoRoute,
    /// We and the target appear to be on different IPv4 /24 prefixes.
    DifferentSubnet,
}

#[derive(Clone, Debug)]
pub struct SelfTestResult {
    pub verdict: Verdict,
    pub message: String,
    /// The local source IP the OS would use to reach the target, if known.
    pub local_ip: Option<IpAddr>,
}

/// Probe `target:port`. Blocking; intended to run off the UI thread.
pub fn run(target: IpAddr, port: u16, timeout: Duration) -> SelfTestResult {
    let local_ip = local_ip_towards(target);

    // Subnet sanity check (IPv4 /24 heuristic) — only a hint, not authoritative.
    if let (Some(IpAddr::V4(local)), IpAddr::V4(remote)) = (local_ip, target) {
        if local.octets()[..3] != remote.octets()[..3] {
            return SelfTestResult {
                verdict: Verdict::DifferentSubnet,
                message: format!(
                    "This device ({local}) and the peer ({remote}) look like they're on \
                     different subnets. Same Wi-Fi name doesn't always mean the same network \
                     — try Ethernet, a phone hotspot, or enter the peer's IP manually."
                ),
                local_ip,
            };
        }
    }

    let addr = SocketAddr::new(target, port);
    match TcpStream::connect_timeout(&addr, timeout) {
        Ok(_) => SelfTestResult {
            verdict: Verdict::Ok,
            message: "Reachable — the control port is open. You should be able to pair/connect."
                .to_string(),
            local_ip,
        },
        Err(e) => classify_error(e, local_ip),
    }
}

fn classify_error(e: std::io::Error, local_ip: Option<IpAddr>) -> SelfTestResult {
    use std::io::ErrorKind::*;
    let (verdict, message) = match e.kind() {
        ConnectionRefused => (
            Verdict::PortClosed,
            "The peer is reachable but refused the connection. Make sure ScreenLink is running \
             on it, and that its firewall allows ScreenLink on Private networks."
                .to_string(),
        ),
        TimedOut => (
            Verdict::Unreachable,
            "No response before timeout. This is the classic sign of AP/client isolation (common \
             on guest/hotel/corporate Wi-Fi) or a firewall silently dropping packets. Try a \
             private network, Ethernet, or a phone hotspot."
                .to_string(),
        ),
        NetworkUnreachable | HostUnreachable => (
            Verdict::NoRoute,
            "The network reports no route to the peer — likely a different subnet/VLAN. Use a \
             shared network or enter the peer's IP manually."
                .to_string(),
        ),
        _ => (
            Verdict::Unreachable,
            format!("Could not reach the peer: {e}"),
        ),
    };
    SelfTestResult {
        verdict,
        message,
        local_ip,
    }
}

/// Discover which local IP the OS would use to reach `target`, without sending
/// anything (UDP connect just sets the route).
fn local_ip_towards(target: IpAddr) -> Option<IpAddr> {
    let bind = if target.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let sock = UdpSocket::bind(bind).ok()?;
    sock.connect(SocketAddr::new(target, 9)).ok()?;
    sock.local_addr().ok().map(|a| a.ip())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_local_port_reports_refused_or_unreachable() {
        // 127.0.0.1 on an almost-certainly-unused high port: loopback is always
        // routable, so we expect a refusal (PortClosed), not a subnet error.
        let r = run(
            IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            1, // port 1 is privileged/unused
            Duration::from_millis(300),
        );
        assert!(
            matches!(r.verdict, Verdict::PortClosed | Verdict::Unreachable),
            "got {:?}",
            r.verdict
        );
    }
}
