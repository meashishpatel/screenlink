//! Networking + session orchestration: the TLS control channel, fingerprint
//! pinning, PIN pairing, the encrypted UDP realtime channel, and the host /
//! controlled session loops that tie input and clipboard together.

use crate::{clipboardsync, inputloop};
use screenlink_core::framing::{read_msg, write_msg};
use screenlink_core::pairing::{comparison_pin, PairingState};
use screenlink_core::protocol::{
    Capabilities, ClipboardData, ControlMsg, DeviceInfo, InputEvent, Mode, ScreenEdge,
    PROTOCOL_VERSION,
};
use screenlink_core::realtime::RealtimeCrypto;
use screenlink_core::security::{
    client_config, server_config, Identity, PinnedVerifier, VerifyMode,
};
use screenlink_core::trust::TrustStore;
use screenlink_input::{Capturer, Injector};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_rustls::{TlsAcceptor, TlsConnector};
use tracing::{debug, error, info, warn};

/// TLS exporter label for the realtime channel key. The key is bound to the
/// authenticated TLS session and never transmitted.
const RT_KEY_LABEL: &[u8] = b"screenlink realtime channel v1";

type BoxRead = Box<dyn AsyncRead + Unpin + Send>;
type BoxWrite = Box<dyn AsyncWrite + Unpin + Send>;

/// Factory closures so the loopback dev mode can substitute recording/scripted
/// backends for the real OS hooks.
pub type CapturerFactory = Arc<dyn Fn() -> anyhow::Result<Box<dyn Capturer>> + Send + Sync>;

/// Shared, cloneable handle to everything a session needs.
#[derive(Clone)]
pub struct AppCore {
    pub identity: Arc<Identity>,
    pub trust: Arc<TrustStore>,
    pub control_port: u16,
    pub realtime_port: u16,
    pub device_name: String,
    pub events: mpsc::UnboundedSender<NetEvent>,
    pub injector: Arc<Mutex<Box<dyn Injector>>>,
    pub inbound_rt: Arc<Mutex<Option<RealtimeCrypto>>>,
    pub make_capturer: CapturerFactory,
    /// Latest decoded frame of a peer's screen being mirrored to us (for the UI).
    pub video_frame: crate::mirror::FrameSlot,
}

impl AppCore {
    pub fn device_info(&self) -> DeviceInfo {
        DeviceInfo {
            id: self.identity.device_id(),
            name: self.device_name.clone(),
            os: std::env::consts::OS.to_string(),
            realtime_port: self.realtime_port,
            caps: Capabilities {
                extend_mode: cfg!(feature = "extend"),
                ..Default::default()
            },
        }
    }
}

/// Connection state surfaced to the UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnState {
    Connecting,
    Pairing,
    Connected { controlling: bool },
    Disconnected,
    Error(String),
}

/// Events from the network layer to the UI.
#[derive(Clone, Debug)]
pub enum NetEvent {
    Status(String),
    PeerState {
        fingerprint: String,
        name: String,
        state: ConnState,
    },
    /// Show this comparison PIN and ask the user to confirm it matches the peer.
    PairingPrompt {
        fingerprint: String,
        name: String,
        pin: String,
    },
    PairingResult {
        fingerprint: String,
        ok: bool,
    },
    /// Round-trip time sample (ms) for the connected peer.
    Rtt {
        fingerprint: String,
        rtt_ms: f32,
    },
    /// True when local control is currently on the remote screen.
    ControlOnRemote(bool),
    SelfTest(screenlink_discovery::selftest::SelfTestResult),
}

/// Commands from the UI to the network layer.
#[derive(Clone, Debug)]
pub enum NetCommand {
    /// Initiate a controlling session to a peer's control address.
    Connect {
        addr: SocketAddr,
        name: String,
        edge: ScreenEdge,
    },
    ConfirmPairing {
        fingerprint: String,
    },
    CancelPairing {
        fingerprint: String,
    },
    Disconnect {
        fingerprint: String,
    },
    SetEdge {
        fingerprint: String,
        edge: ScreenEdge,
    },
    SnapHome,
    SelfTest {
        ip: std::net::IpAddr,
        port: u16,
    },
    /// Start/stop sharing this machine's screen to the peer (mirror).
    ShareScreen {
        fingerprint: String,
    },
    StopShareScreen {
        fingerprint: String,
    },
}

/// Internal per-session command (routed from `NetCommand`).
#[derive(Clone, Debug)]
enum SessionCmd {
    Confirm,
    Cancel,
    Disconnect,
    SetEdge(ScreenEdge),
    SnapHome,
    StartMirror,
    StopMirror,
}

struct SessionHandle {
    peer_fp: Arc<Mutex<Option<String>>>,
    tx: mpsc::Sender<SessionCmd>,
}

enum Role {
    /// We have the keyboard/mouse and drive the peer.
    Controller {
        peer_ip: std::net::IpAddr,
        edge: ScreenEdge,
    },
    /// We are driven by the peer (inject its input).
    Controlled,
}

/// Run the whole network layer: server accept loop, UDP inject listener, and the
/// command dispatcher. Returns when `cmd_rx` closes.
pub async fn run(core: AppCore, mut cmd_rx: mpsc::Receiver<NetCommand>) {
    let sessions: Arc<Mutex<Vec<SessionHandle>>> = Arc::new(Mutex::new(Vec::new()));

    spawn_udp_listener(core.clone());

    // Server accept loop (we act as the controlled side for inbound peers).
    {
        let core = core.clone();
        let sessions = sessions.clone();
        tokio::spawn(async move {
            if let Err(e) = accept_loop(core.clone(), sessions).await {
                error!("accept loop ended: {e}");
                let _ = core
                    .events
                    .send(NetEvent::Status(format!("Listener stopped: {e}")));
            }
        });
    }

    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            NetCommand::Connect { addr, name, edge } => {
                let core = core.clone();
                let sessions = sessions.clone();
                let (tx, rx) = mpsc::channel(16);
                let peer_fp = Arc::new(Mutex::new(None));
                sessions.lock().unwrap().push(SessionHandle {
                    peer_fp: peer_fp.clone(),
                    tx,
                });
                tokio::spawn(async move {
                    let role = Role::Controller {
                        peer_ip: addr.ip(),
                        edge,
                    };
                    if let Err(e) =
                        connect_and_run(core.clone(), addr, name.clone(), role, rx, peer_fp).await
                    {
                        warn!("controller session to {addr} ended: {e}");
                        let _ = core.events.send(NetEvent::PeerState {
                            fingerprint: String::new(),
                            name,
                            state: ConnState::Error(e.to_string()),
                        });
                    }
                });
            }
            NetCommand::SelfTest { ip, port } => {
                let core = core.clone();
                tokio::task::spawn_blocking(move || {
                    let r =
                        screenlink_discovery::selftest::run(ip, port, Duration::from_millis(1500));
                    let _ = core.events.send(NetEvent::SelfTest(r));
                });
            }
            NetCommand::ConfirmPairing { fingerprint } => {
                route(&sessions, &fingerprint, SessionCmd::Confirm).await
            }
            NetCommand::CancelPairing { fingerprint } => {
                route(&sessions, &fingerprint, SessionCmd::Cancel).await
            }
            NetCommand::Disconnect { fingerprint } => {
                route(&sessions, &fingerprint, SessionCmd::Disconnect).await
            }
            NetCommand::SetEdge { fingerprint, edge } => {
                route(&sessions, &fingerprint, SessionCmd::SetEdge(edge)).await
            }
            NetCommand::SnapHome => {
                broadcast(&sessions, SessionCmd::SnapHome).await;
            }
            NetCommand::ShareScreen { fingerprint } => {
                route(&sessions, &fingerprint, SessionCmd::StartMirror).await
            }
            NetCommand::StopShareScreen { fingerprint } => {
                route(&sessions, &fingerprint, SessionCmd::StopMirror).await
            }
        }
    }
}

/// Send a command to sessions matching `fingerprint` (empty matches all, for
/// pairing prompts where the fp may not be displayed yet). Prunes dead handles.
async fn route(sessions: &Arc<Mutex<Vec<SessionHandle>>>, fingerprint: &str, cmd: SessionCmd) {
    let targets: Vec<mpsc::Sender<SessionCmd>> = {
        let guard = sessions.lock().unwrap();
        guard
            .iter()
            .filter(|h| {
                let fp = h.peer_fp.lock().unwrap();
                fingerprint.is_empty() || fp.as_deref() == Some(fingerprint)
            })
            .map(|h| h.tx.clone())
            .collect()
    };
    for tx in targets {
        let _ = tx.send(cmd.clone()).await;
    }
}

async fn broadcast(sessions: &Arc<Mutex<Vec<SessionHandle>>>, cmd: SessionCmd) {
    let txs: Vec<_> = sessions
        .lock()
        .unwrap()
        .iter()
        .map(|h| h.tx.clone())
        .collect();
    for tx in txs {
        let _ = tx.send(cmd.clone()).await;
    }
}

async fn accept_loop(
    core: AppCore,
    sessions: Arc<Mutex<Vec<SessionHandle>>>,
) -> anyhow::Result<()> {
    // We accept any cert at the TLS layer and pin identity at the app layer (we
    // never act on input/clipboard until the peer is trusted/paired).
    let verifier = PinnedVerifier::new(VerifyMode::PairTofu);
    let cfg = server_config(&core.identity, verifier)?;
    let acceptor = TlsAcceptor::from(cfg);

    let listener = TcpListener::bind(("0.0.0.0", core.control_port)).await?;
    info!("control listener on 0.0.0.0:{}", core.control_port);

    loop {
        let (tcp, peer) = listener.accept().await?;
        let acceptor = acceptor.clone();
        let core = core.clone();
        let sessions = sessions.clone();
        tokio::spawn(async move {
            debug!("inbound connection from {peer}");
            let tls = match acceptor.accept(tcp).await {
                Ok(t) => t,
                Err(e) => {
                    warn!("TLS accept from {peer} failed: {e}");
                    return;
                }
            };
            let key = match export_key(tls.get_ref().1) {
                Ok(k) => k,
                Err(e) => {
                    warn!("keying material export failed: {e}");
                    return;
                }
            };
            let (rd, wr) = tokio::io::split(tls);
            let (tx, rx) = mpsc::channel(16);
            let peer_fp = Arc::new(Mutex::new(None));
            sessions.lock().unwrap().push(SessionHandle {
                peer_fp: peer_fp.clone(),
                tx,
            });
            if let Err(e) = run_session(
                core,
                key,
                peer.ip(),
                Box::new(rd),
                Box::new(wr),
                Role::Controlled,
                rx,
                peer_fp,
            )
            .await
            {
                debug!("controlled session from {peer} ended: {e}");
            }
        });
    }
}

async fn connect_and_run(
    core: AppCore,
    addr: SocketAddr,
    name: String,
    role: Role,
    scmd_rx: mpsc::Receiver<SessionCmd>,
    peer_fp: Arc<Mutex<Option<String>>>,
) -> anyhow::Result<()> {
    let _ = core.events.send(NetEvent::PeerState {
        fingerprint: String::new(),
        name: name.clone(),
        state: ConnState::Connecting,
    });

    let verifier = PinnedVerifier::new(VerifyMode::PairTofu);
    let cfg = client_config(&core.identity, verifier)?;
    let connector = TlsConnector::from(cfg);

    let tcp = TcpStream::connect(addr).await?;
    tcp.set_nodelay(true).ok();
    let tls = connector
        .connect(screenlink_core::security::sni_name(), tcp)
        .await?;
    let key = export_key(tls.get_ref().1)?;
    let (rd, wr) = tokio::io::split(tls);
    run_session(
        core,
        key,
        addr.ip(),
        Box::new(rd),
        Box::new(wr),
        role,
        scmd_rx,
        peer_fp,
    )
    .await
}

/// Export the 32-byte realtime key from a completed TLS handshake. Works for both
/// client and server connections via the shared `export_keying_material`.
fn export_key<D>(conn: &rustls::ConnectionCommon<D>) -> anyhow::Result<[u8; 32]> {
    let key = conn
        .export_keying_material([0u8; 32], RT_KEY_LABEL, None)
        .map_err(|e| anyhow::anyhow!("export_keying_material: {e}"))?;
    Ok(key)
}

#[allow(clippy::too_many_arguments)]
async fn run_session(
    core: AppCore,
    key: [u8; 32],
    peer_ip: std::net::IpAddr,
    mut rd: BoxRead,
    mut wr: BoxWrite,
    role: Role,
    mut scmd_rx: mpsc::Receiver<SessionCmd>,
    peer_fp_slot: Arc<Mutex<Option<String>>>,
) -> anyhow::Result<()> {
    // Separate key for the video channel so it can't reuse a (key, nonce) pair
    // with the input channel.
    let video_key = screenlink_core::realtime::derive_subkey(&key, b"video");
    // Stop handles for the mirror sender (we share) and receiver (we view).
    let mut mirror_tx_stop: Option<Arc<std::sync::atomic::AtomicBool>> = None;
    let mut mirror_rx_stop: Option<Arc<std::sync::atomic::AtomicBool>> = None;
    // --- Hello exchange ---
    write_msg(
        &mut wr,
        &ControlMsg::Hello {
            protocol: PROTOCOL_VERSION,
            info: core.device_info(),
        },
    )
    .await?;
    let peer_info = match read_msg::<_, ControlMsg>(&mut rd).await? {
        ControlMsg::Hello { protocol, info } => {
            if protocol != PROTOCOL_VERSION {
                write_msg(
                    &mut wr,
                    &ControlMsg::Reject {
                        reason: format!("protocol {protocol} != {PROTOCOL_VERSION}"),
                    },
                )
                .await
                .ok();
                anyhow::bail!("protocol mismatch (peer {protocol}, us {PROTOCOL_VERSION})");
            }
            info
        }
        other => anyhow::bail!("expected Hello, got {other:?}"),
    };

    let peer_fp = peer_info.id.0.clone();
    let peer_name = peer_info.name.clone();
    *peer_fp_slot.lock().unwrap() = Some(peer_fp.clone());

    // Spawn a dedicated reader task so inbound messages arrive over a
    // *cancellation-safe* channel. Selecting directly on `read_msg` can drop a
    // partially-read frame when another select branch wins, desyncing the stream
    // (this is what made pairing intermittently hang).
    let (in_tx, mut in_rx) = mpsc::channel::<ControlMsg>(256);
    let reader = tokio::spawn(async move {
        // Ends when the peer closes / a framing error occurs (read_msg → Err) or
        // the session drops the receiver (send → Err).
        while let Ok(m) = read_msg::<_, ControlMsg>(&mut rd).await {
            if in_tx.send(m).await.is_err() {
                break;
            }
        }
    });

    // Writer task drains outgoing control messages onto the wire. All writes
    // (pairing, mode, clipboard, ping) go through `out_tx` from here on.
    let (out_tx, mut out_rx) = mpsc::channel::<ControlMsg>(256);
    let writer = tokio::spawn(async move {
        while let Some(m) = out_rx.recv().await {
            if write_msg(&mut wr, &m).await.is_err() {
                break;
            }
        }
    });

    // --- Pairing (if not already trusted) ---
    if !core.trust.is_trusted(&peer_fp) {
        let _ = core.events.send(NetEvent::PeerState {
            fingerprint: peer_fp.clone(),
            name: peer_name.clone(),
            state: ConnState::Pairing,
        });
        let pin = comparison_pin(core.identity.fingerprint(), &peer_fp);
        let _ = core.events.send(NetEvent::PairingPrompt {
            fingerprint: peer_fp.clone(),
            name: peer_name.clone(),
            pin,
        });
        out_tx.send(ControlMsg::PairBegin).await.ok();

        let mut state = PairingState::default();
        loop {
            tokio::select! {
                msg = in_rx.recv() => {
                    match msg {
                        Some(ControlMsg::PairBegin) => {}
                        Some(ControlMsg::PairConfirm) => state.confirm_remote(),
                        Some(ControlMsg::PairCancel { reason }) => {
                            let _ = core.events.send(NetEvent::PairingResult { fingerprint: peer_fp.clone(), ok: false });
                            anyhow::bail!("peer cancelled pairing: {reason}");
                        }
                        Some(other) => anyhow::bail!("unexpected during pairing: {other:?}"),
                        None => anyhow::bail!("connection closed during pairing"),
                    }
                }
                scmd = scmd_rx.recv() => {
                    match scmd {
                        Some(SessionCmd::Confirm) => {
                            state.confirm_local();
                            out_tx.send(ControlMsg::PairConfirm).await.ok();
                        }
                        Some(SessionCmd::Cancel) | Some(SessionCmd::Disconnect) | None => {
                            out_tx.send(ControlMsg::PairCancel { reason: "user cancelled".into() }).await.ok();
                            let _ = core.events.send(NetEvent::PairingResult { fingerprint: peer_fp.clone(), ok: false });
                            anyhow::bail!("local cancel during pairing");
                        }
                        _ => {}
                    }
                }
            }
            if state.is_complete() {
                core.trust.trust(&peer_fp, &peer_name)?;
                let _ = core.events.send(NetEvent::PairingResult {
                    fingerprint: peer_fp.clone(),
                    ok: true,
                });
                let _ = core
                    .events
                    .send(NetEvent::Status(format!("Paired with {peer_name}")));
                break;
            }
        }
    }

    // --- Authenticated. Wire up input + clipboard. ---
    let clip_tx = clipboardsync::spawn(out_tx.clone());
    let mut host_input: Option<inputloop::HostInputHandle> = None;
    let controlling = matches!(role, Role::Controller { .. });

    if let Role::Controller { peer_ip, edge } = &role {
        let peer_ip = *peer_ip;
        let edge = *edge;
        // Open the realtime UDP channel and start capturing.
        let epoch: u64 = rand::random();
        out_tx
            .send(ControlMsg::SetMode {
                mode: Mode::Control,
            })
            .await
            .ok();
        out_tx
            .send(ControlMsg::RealtimeOpen {
                udp_port: core.realtime_port,
                epoch,
            })
            .await
            .ok();

        let udp = std::net::UdpSocket::bind(("0.0.0.0", 0))?;
        udp.connect(SocketAddr::new(peer_ip, peer_info.realtime_port))?;
        let rt_out = RealtimeCrypto::new(key, epoch, true);
        let capturer = (core.make_capturer)()?;
        host_input = Some(inputloop::spawn(
            capturer,
            edge,
            25,
            screenlink_input::desktop_rect(),
            udp,
            rt_out,
            out_tx.clone(),
            core.events.clone(),
        ));
        info!("controlling {peer_name} (edge {edge:?}, realtime epoch {epoch})");
    }

    let _ = core.events.send(NetEvent::PeerState {
        fingerprint: peer_fp.clone(),
        name: peer_name.clone(),
        state: ConnState::Connected { controlling },
    });

    // Periodic ping for RTT + keepalive.
    let mut ping = tokio::time::interval(Duration::from_secs(3));
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut ping_sent: HashMap<u64, Instant> = HashMap::new();
    let mut ping_nonce: u64 = 0;

    let result: anyhow::Result<()> = loop {
        tokio::select! {
            msg = in_rx.recv() => {
                match msg {
                    Some(m) => {
                        // Mirror start/stop need the session-local stop handle, so
                        // they're handled here rather than in handle_inbound.
                        match &m {
                            ControlMsg::StartMirror { epoch } => {
                                if let Some(s) = mirror_rx_stop.take() {
                                    s.store(true, std::sync::atomic::Ordering::Relaxed);
                                }
                                let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
                                crate::mirror::spawn_receiver(
                                    video_key,
                                    *epoch,
                                    core.video_frame.clone(),
                                    stop.clone(),
                                );
                                mirror_rx_stop = Some(stop);
                                let _ = core.events.send(NetEvent::Status(format!(
                                    "Viewing {peer_name}'s screen"
                                )));
                            }
                            ControlMsg::StopMirror => {
                                if let Some(s) = mirror_rx_stop.take() {
                                    s.store(true, std::sync::atomic::Ordering::Relaxed);
                                }
                                *core.video_frame.lock().unwrap() = None;
                            }
                            _ => {}
                        }
                        if let Some(stop_reason) = handle_inbound(
                            &core, &role, m, key, &out_tx, &clip_tx, &peer_fp, &mut ping_sent,
                        ).await {
                            break Err(anyhow::anyhow!(stop_reason));
                        }
                    }
                    None => break Ok(()), // peer closed
                }
            }
            scmd = scmd_rx.recv() => {
                match scmd {
                    Some(SessionCmd::Disconnect) | None => break Ok(()),
                    Some(SessionCmd::SnapHome) => {
                        if let Some(h) = &host_input {
                            h.snap_home.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                    Some(SessionCmd::SetEdge(edge)) => {
                        if let Some(h) = &host_input {
                            *h.edge.lock().unwrap() = edge;
                        }
                    }
                    Some(SessionCmd::StartMirror) => {
                        if mirror_tx_stop.is_none() {
                            let epoch: u64 = rand::random();
                            if out_tx.send(ControlMsg::StartMirror { epoch }).await.is_err() {
                                break Ok(());
                            }
                            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
                            crate::mirror::spawn_sender(peer_ip, video_key, epoch, stop.clone());
                            mirror_tx_stop = Some(stop);
                            let _ = core.events.send(NetEvent::Status(format!(
                                "Sharing screen with {peer_name}"
                            )));
                        }
                    }
                    Some(SessionCmd::StopMirror) => {
                        if let Some(s) = mirror_tx_stop.take() {
                            s.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                        let _ = out_tx.send(ControlMsg::StopMirror).await;
                    }
                    _ => {}
                }
            }
            _ = ping.tick() => {
                ping_nonce += 1;
                ping_sent.insert(ping_nonce, Instant::now());
                if out_tx.send(ControlMsg::Ping { nonce: ping_nonce }).await.is_err() {
                    break Ok(());
                }
            }
        }
    };

    // --- Teardown ---
    if let Some(h) = host_input {
        h.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    if let Some(s) = mirror_tx_stop {
        s.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    if let Some(s) = mirror_rx_stop {
        s.store(true, std::sync::atomic::Ordering::Relaxed);
        *core.video_frame.lock().unwrap() = None;
    }
    let _ = clip_tx.send(clipboardsync::ClipCmd::Stop);
    if matches!(role, Role::Controlled) {
        *core.inbound_rt.lock().unwrap() = None;
    }
    drop(out_tx);
    writer.abort();
    reader.abort();
    let _ = core.events.send(NetEvent::PeerState {
        fingerprint: peer_fp,
        name: peer_name,
        state: ConnState::Disconnected,
    });
    result
}

/// Handle one inbound control message. Returns `Some(reason)` to end the session.
#[allow(clippy::too_many_arguments)]
async fn handle_inbound(
    core: &AppCore,
    role: &Role,
    msg: ControlMsg,
    key: [u8; 32],
    out_tx: &mpsc::Sender<ControlMsg>,
    clip_tx: &std::sync::mpsc::Sender<clipboardsync::ClipCmd>,
    peer_fp: &str,
    ping_sent: &mut HashMap<u64, Instant>,
) -> Option<String> {
    match msg {
        ControlMsg::SetMode { mode } => {
            // Controlled side acknowledges; controller ignores.
            if matches!(role, Role::Controlled) {
                let _ = out_tx
                    .send(ControlMsg::ModeAck {
                        mode,
                        ok: true,
                        reason: String::new(),
                    })
                    .await;
            }
        }
        ControlMsg::RealtimeOpen { epoch, .. } => {
            if matches!(role, Role::Controlled) {
                // The TLS-exported `key` is identical on both peers; combine it
                // with the controller-chosen epoch to open the inbound channel.
                // The process-wide UDP listener picks this up and starts
                // decrypting + injecting.
                *core.inbound_rt.lock().unwrap() = Some(RealtimeCrypto::new(key, epoch, false));
                info!("realtime channel opened (epoch {epoch})");
            }
        }
        ControlMsg::EdgeEnter { x, y } => {
            if matches!(role, Role::Controlled) {
                if let Ok(mut inj) = core.injector.lock() {
                    let _ = inj.set_cursor_norm(x, y);
                }
            }
        }
        ControlMsg::EdgeLeave => {}
        ControlMsg::Clipboard(ClipboardData::Text(text)) => {
            let _ = clip_tx.send(clipboardsync::ClipCmd::ApplyRemote(text));
        }
        ControlMsg::Ping { nonce } => {
            let _ = out_tx.send(ControlMsg::Pong { nonce }).await;
        }
        ControlMsg::Pong { nonce } => {
            if let Some(t) = ping_sent.remove(&nonce) {
                let rtt = t.elapsed().as_secs_f32() * 1000.0;
                let _ = core.events.send(NetEvent::Rtt {
                    fingerprint: peer_fp.to_string(),
                    rtt_ms: rtt,
                });
            }
        }
        ControlMsg::Reject { reason } => return Some(format!("peer rejected: {reason}")),
        ControlMsg::ModeAck { .. } | ControlMsg::Hello { .. } | ControlMsg::HelloAck { .. } => {}
        ControlMsg::PairBegin
        | ControlMsg::PairConfirm
        | ControlMsg::PairConfirmed
        | ControlMsg::PairCancel { .. } => {
            // Pairing already settled; ignore stragglers.
        }
        // Mirror start/stop are handled in the session loop (they need the
        // session-local stop handle), so they're no-ops here.
        ControlMsg::StartMirror { .. } | ControlMsg::StopMirror => {}
    }
    None
}

/// Process-wide UDP listener that decrypts inbound realtime packets and injects
/// them. One per device; the active `inbound_rt` is swapped in by the controlled
/// session.
fn spawn_udp_listener(core: AppCore) {
    std::thread::Builder::new()
        .name("screenlink-udp-inject".into())
        .spawn(move || {
            let sock = match std::net::UdpSocket::bind(("0.0.0.0", core.realtime_port)) {
                Ok(s) => s,
                Err(e) => {
                    error!("realtime UDP bind failed on :{}: {e}", core.realtime_port);
                    return;
                }
            };
            sock.set_read_timeout(Some(Duration::from_millis(250))).ok();
            info!("realtime UDP listener on 0.0.0.0:{}", core.realtime_port);
            let mut buf = [0u8; 2048];
            loop {
                let n = match sock.recv_from(&mut buf) {
                    Ok((n, _)) => n,
                    Err(ref e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        continue
                    }
                    Err(e) => {
                        warn!("udp recv error: {e}");
                        continue;
                    }
                };
                // Decrypt under the inbound lock, then inject outside it.
                let event = {
                    let mut guard = core.inbound_rt.lock().unwrap();
                    match guard.as_mut() {
                        Some(rt) => match rt.open(&buf[..n]) {
                            Ok((_, pt)) => postcard::from_bytes::<InputEvent>(&pt).ok(),
                            Err(_) => None,
                        },
                        None => None,
                    }
                };
                if let Some(ev) = event {
                    if let Ok(mut inj) = core.injector.lock() {
                        let _ = inj.inject(ev);
                    }
                }
            }
        })
        .expect("spawn udp listener");
}
