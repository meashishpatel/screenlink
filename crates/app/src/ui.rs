//! Tray icon + egui settings/status window.
//!
//! Runs on the main thread (eframe owns the event loop). It talks to the network
//! layer purely through the `cmd_tx` / `ev_rx` channels set up in `main`, so the
//! UI never blocks on I/O.

use crate::net::{AppCore, ConnState, NetCommand, NetEvent};
use screenlink_core::config::{AppConfig, PeerConfig};
use screenlink_core::protocol::{Mode, ScreenEdge};
use screenlink_core::trust::TrustStore;
use screenlink_discovery::{Discovery, PeerSource};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const EDGES: [ScreenEdge; 4] = [
    ScreenEdge::Left,
    ScreenEdge::Right,
    ScreenEdge::Top,
    ScreenEdge::Bottom,
];

/// Everything the UI needs, handed over from `main`.
pub struct UiDeps {
    pub core: AppCore,
    pub discovery: Discovery,
    pub trust: Arc<TrustStore>,
    pub config: Arc<Mutex<AppConfig>>,
    pub cmd_tx: tokio::sync::mpsc::Sender<NetCommand>,
    pub ev_rx: tokio::sync::mpsc::UnboundedReceiver<NetEvent>,
    /// Kept alive so the async runtime isn't dropped while the window is open.
    pub _rt: tokio::runtime::Runtime,
}

pub fn run(deps: UiDeps) -> anyhow::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([760.0, 620.0])
            .with_min_inner_size([560.0, 420.0])
            .with_title("ScreenLink")
            .with_icon(std::sync::Arc::new(egui::IconData {
                rgba: crate::icon::rgba(64),
                width: 64,
                height: 64,
            })),
        ..Default::default()
    };
    eframe::run_native(
        "ScreenLink",
        native_options,
        Box::new(|_cc| Ok(Box::new(ScreenLinkApp::new(deps)) as Box<dyn eframe::App>)),
    )
    .map_err(|e| anyhow::anyhow!("eframe error: {e}"))
}

struct PeerRow {
    name: String,
    state: ConnState,
    rtt_ms: Option<f32>,
}

struct ScreenLinkApp {
    deps: UiDeps,
    our_name: String,
    our_fp_short: String,
    our_ip: String,
    our_control_port: u16,
    log: Vec<String>,
    peers: HashMap<String, PeerRow>,
    pairing: Option<PairingPrompt>,
    control_on_remote: bool,
    selected_edge: ScreenEdge,
    manual_ip: String,
    manual_port: String,
    last_selftest: Option<String>,
    _tray: Option<tray_icon::TrayIcon>,
    tray_quit_id: Option<tray_icon::menu::MenuId>,
}

struct PairingPrompt {
    fingerprint: String,
    name: String,
    pin: String,
}

impl ScreenLinkApp {
    fn new(deps: UiDeps) -> Self {
        let our_name = deps.core.device_name.clone();
        let our_fp_short = deps.core.identity.device_id().short().to_string();
        let our_ip = local_ipv4().unwrap_or_else(|| "unknown".into());
        let our_control_port = deps.core.control_port;
        let (tray, tray_quit_id) = build_tray();
        Self {
            deps,
            our_name,
            our_fp_short,
            our_ip,
            our_control_port,
            log: vec!["ScreenLink started.".into()],
            peers: HashMap::new(),
            pairing: None,
            control_on_remote: false,
            selected_edge: ScreenEdge::Right,
            manual_ip: String::new(),
            manual_port: screenlink_core::DEFAULT_CONTROL_PORT.to_string(),
            last_selftest: None,
            _tray: tray,
            tray_quit_id,
        }
    }

    fn send(&self, cmd: NetCommand) {
        if let Err(e) = self.deps.cmd_tx.try_send(cmd) {
            tracing::warn!("dropped command: {e}");
        }
    }

    fn log_line(&mut self, s: impl Into<String>) {
        self.log.push(s.into());
        if self.log.len() > 200 {
            let drain = self.log.len() - 200;
            self.log.drain(0..drain);
        }
    }

    fn drain_events(&mut self) {
        while let Ok(ev) = self.deps.ev_rx.try_recv() {
            match ev {
                NetEvent::Status(s) => self.log_line(s),
                NetEvent::PeerState {
                    fingerprint,
                    name,
                    state,
                } => {
                    self.log_line(format!("{name}: {state:?}"));
                    let entry = self.peers.entry(fingerprint).or_insert_with(|| PeerRow {
                        name: name.clone(),
                        state: state.clone(),
                        rtt_ms: None,
                    });
                    entry.name = name;
                    entry.state = state;
                }
                NetEvent::PairingPrompt {
                    fingerprint,
                    name,
                    pin,
                } => {
                    self.log_line(format!("Pairing with {name}: confirm PIN {pin}"));
                    self.pairing = Some(PairingPrompt {
                        fingerprint,
                        name,
                        pin,
                    });
                }
                NetEvent::PairingResult { fingerprint, ok } => {
                    self.log_line(format!(
                        "Pairing {}",
                        if ok { "succeeded" } else { "cancelled/failed" }
                    ));
                    if self.pairing.as_ref().map(|p| &p.fingerprint) == Some(&fingerprint) {
                        self.pairing = None;
                    }
                }
                NetEvent::Rtt {
                    fingerprint,
                    rtt_ms,
                } => {
                    if let Some(row) = self.peers.get_mut(&fingerprint) {
                        row.rtt_ms = Some(rtt_ms);
                    }
                }
                NetEvent::ControlOnRemote(on) => self.control_on_remote = on,
                NetEvent::SelfTest(r) => {
                    self.last_selftest = Some(format!("{:?}: {}", r.verdict, r.message));
                    self.log_line(format!("Self-test: {:?}", r.verdict));
                }
            }
        }
    }

    fn poll_tray(&mut self) {
        if let Ok(event) = tray_icon::menu::MenuEvent::receiver().try_recv() {
            if self.tray_quit_id.as_ref() == Some(&event.id) {
                std::process::exit(0);
            }
        }
    }
}

impl eframe::App for ScreenLinkApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();
        self.poll_tray();

        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading("ScreenLink");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.control_on_remote {
                        ui.colored_label(
                            egui::Color32::from_rgb(80, 200, 120),
                            "● control on remote",
                        );
                    } else {
                        ui.label("● control local");
                    }
                });
            });
            ui.label(format!(
                "This device: {}  (id {})",
                self.our_name, self.our_fp_short
            ));
            ui.label(
                egui::RichText::new(format!(
                    "Reachable at {}:{}  (give this to the other laptop for manual connect)",
                    self.our_ip, self.our_control_port
                ))
                .weak(),
            );
            ui.add_space(4.0);
        });

        egui::TopBottomPanel::bottom("log")
            .resizable(true)
            .show(ctx, |ui| {
                ui.add_space(2.0);
                ui.label(egui::RichText::new("Activity").strong());
                egui::ScrollArea::vertical()
                    .max_height(120.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in &self.log {
                            ui.label(line.as_str());
                        }
                    });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            // Pairing prompt takes priority.
            if let Some(p) = &self.pairing {
                egui::Frame::group(ui.style())
                    .fill(egui::Color32::from_rgb(40, 40, 28))
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(format!("Pair with {}?", p.name))
                                .size(18.0)
                                .strong(),
                        );
                        ui.label("Confirm this code matches the one shown on the other device:");
                        ui.label(
                            egui::RichText::new(p.pin.as_str())
                                .monospace()
                                .size(40.0)
                                .strong(),
                        );
                        ui.horizontal(|ui| {
                            let fp = p.fingerprint.clone();
                            if ui.button("✅ Codes match — pair").clicked() {
                                self.send(NetCommand::ConfirmPairing {
                                    fingerprint: fp.clone(),
                                });
                            }
                            if ui.button("✋ Cancel").clicked() {
                                self.send(NetCommand::CancelPairing { fingerprint: fp });
                            }
                        });
                    });
                ui.separator();
            }

            ui.collapsing("Devices on this network", |ui| {
                self.devices_ui(ui);
            });

            ui.separator();
            ui.collapsing("Screen arrangement", |ui| {
                self.arrangement_ui(ui);
            });

            ui.separator();
            ui.collapsing("Manual connection & diagnostics", |ui| {
                self.manual_ui(ui);
            });

            ui.separator();
            ui.collapsing("Paired devices", |ui| {
                self.paired_ui(ui);
            });

            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("⏎ Snap control home").clicked() {
                    self.send(NetCommand::SnapHome);
                }
                ui.label(format!(
                    "(hotkey: {})",
                    self.deps.config.lock().unwrap().snap_home_hotkey
                ));
            });
        });

        // Keep polling events even without user interaction.
        ctx.request_repaint_after(Duration::from_millis(200));
    }
}

impl ScreenLinkApp {
    fn devices_ui(&mut self, ui: &mut egui::Ui) {
        let peers = self.deps.discovery.peers();
        if peers.is_empty() {
            ui.label("No devices found yet. Make sure ScreenLink is running on the other laptop and both are on the same network.");
        }
        for peer in peers {
            ui.horizontal(|ui| {
                let src = match peer.source {
                    PeerSource::Mdns => "mDNS",
                    PeerSource::Manual => "manual",
                };
                let addr = peer
                    .primary_addr()
                    .map(|a| a.to_string())
                    .unwrap_or_else(|| "?".into());
                ui.label(format!("{}  [{src}]  {addr}:{}", peer.name, peer.port));
                let trusted =
                    !peer.fingerprint.is_empty() && self.deps.trust.is_trusted(&peer.fingerprint);
                if trusted {
                    ui.label("🔓 paired");
                }
                if let Some(row) = self.peers.get(&peer.fingerprint) {
                    state_badge(ui, &row.state);
                }
                if let Some(ip) = peer.primary_addr() {
                    if ui.button("🎮 Control").clicked() {
                        let addr = std::net::SocketAddr::new(ip, peer.port);
                        self.send(NetCommand::Connect {
                            addr,
                            name: peer.name.clone(),
                            edge: self.selected_edge,
                        });
                        self.log_line(format!("Connecting to {} ({addr})…", peer.name));
                    }
                    if ui.button("🔎 Test").clicked() {
                        self.send(NetCommand::SelfTest {
                            ip,
                            port: peer.port,
                        });
                    }
                }
            });
        }
    }

    fn arrangement_ui(&mut self, ui: &mut egui::Ui) {
        ui.label("Place the other screen relative to this one. Moving the cursor off that edge crosses control over.");
        ui.horizontal(|ui| {
            egui::ComboBox::from_label("Edge")
                .selected_text(format!("{:?}", self.selected_edge))
                .show_ui(ui, |ui| {
                    for e in EDGES {
                        ui.selectable_value(&mut self.selected_edge, e, format!("{e:?}"));
                    }
                });
            if ui.button("Apply to connected").clicked() {
                // Apply to every known peer; sessions that aren't controlling ignore it.
                let fps: Vec<String> = self.peers.keys().cloned().collect();
                for fp in fps {
                    self.send(NetCommand::SetEdge {
                        fingerprint: fp,
                        edge: self.selected_edge,
                    });
                }
            }
        });
        // Tiny arrangement sketch. The canvas must be tall enough for the
        // vertical (Top/Bottom) layouts, not just left/right.
        let (resp, painter) = ui.allocate_painter(
            egui::vec2(ui.available_width().min(320.0), 170.0),
            egui::Sense::hover(),
        );
        let rect = resp.rect;
        let box_size = egui::vec2(66.0, 44.0);
        let host = egui::Rect::from_center_size(rect.center(), box_size);
        let off = 58.0;
        let peer_center = match self.selected_edge {
            ScreenEdge::Right => host.center() + egui::vec2(off, 0.0),
            ScreenEdge::Left => host.center() - egui::vec2(off, 0.0),
            ScreenEdge::Top => host.center() - egui::vec2(0.0, off),
            ScreenEdge::Bottom => host.center() + egui::vec2(0.0, off),
        };
        let peer = egui::Rect::from_center_size(peer_center, box_size);
        let stroke = egui::Stroke::new(1.5, egui::Color32::GRAY);
        painter.rect_stroke(host, 4.0, egui::Stroke::new(2.0, egui::Color32::LIGHT_BLUE));
        painter.rect_stroke(peer, 4.0, stroke);
        painter.text(
            host.center(),
            egui::Align2::CENTER_CENTER,
            "this",
            egui::FontId::proportional(12.0),
            egui::Color32::WHITE,
        );
        painter.text(
            peer.center(),
            egui::Align2::CENTER_CENTER,
            "peer",
            egui::FontId::proportional(12.0),
            egui::Color32::GRAY,
        );
    }

    fn manual_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("IP");
            ui.add(egui::TextEdit::singleline(&mut self.manual_ip).desired_width(140.0));
            ui.label("Port");
            ui.add(egui::TextEdit::singleline(&mut self.manual_port).desired_width(60.0));
        });
        ui.horizontal(|ui| {
            let parsed = self
                .manual_ip
                .parse::<std::net::IpAddr>()
                .ok()
                .zip(self.manual_port.parse::<u16>().ok());
            if ui
                .add_enabled(parsed.is_some(), egui::Button::new("🎮 Control"))
                .clicked()
            {
                if let Some((ip, port)) = parsed {
                    self.send(NetCommand::Connect {
                        addr: std::net::SocketAddr::new(ip, port),
                        name: format!("{ip}"),
                        edge: self.selected_edge,
                    });
                }
            }
            if ui
                .add_enabled(parsed.is_some(), egui::Button::new("🔎 Self-test"))
                .clicked()
            {
                if let Some((ip, port)) = parsed {
                    self.send(NetCommand::SelfTest { ip, port });
                }
            }
        });
        if let Some(s) = &self.last_selftest {
            ui.label(s.as_str());
        }
    }

    fn paired_ui(&mut self, ui: &mut egui::Ui) {
        let devices = self.deps.trust.list();
        if devices.is_empty() {
            ui.label("No paired devices yet.");
        }
        for d in devices {
            ui.horizontal(|ui| {
                let short = &d.fingerprint[..d.fingerprint.len().min(8)];
                let rtt = self
                    .peers
                    .get(&d.fingerprint)
                    .and_then(|p| p.rtt_ms)
                    .map(|r| format!("  •  {r:.0} ms"))
                    .unwrap_or_default();
                ui.label(format!("{}  (id {short}){rtt}", d.name));
                if ui.button("🗑 Unpair").clicked() {
                    let _ = self.deps.trust.revoke(&d.fingerprint);
                    self.send(NetCommand::Disconnect {
                        fingerprint: d.fingerprint.clone(),
                    });
                    // Drop any saved arrangement for it. Scope the lock so the
                    // guard is released before the &mut self log call below.
                    {
                        let mut cfg = self.deps.config.lock().unwrap();
                        cfg.peers
                            .retain(|p: &PeerConfig| p.fingerprint != d.fingerprint);
                        let _ = cfg.save(&screenlink_core::config::config_path());
                    }
                    self.log_line(format!("Unpaired {}", d.name));
                }
            });
        }
        let _ = Mode::Control; // (mode toggle per device lands with Extend mode, Phase 2)
    }
}

/// Render a small colored badge for a connection state.
fn state_badge(ui: &mut egui::Ui, state: &ConnState) {
    let (text, color) = match state {
        ConnState::Connecting => ("connecting", egui::Color32::from_rgb(220, 180, 60)),
        ConnState::Pairing => ("pairing", egui::Color32::from_rgb(220, 180, 60)),
        ConnState::Connected { controlling: true } => {
            ("● controlling", egui::Color32::from_rgb(80, 200, 120))
        }
        ConnState::Connected { controlling: false } => {
            ("● connected", egui::Color32::from_rgb(80, 200, 120))
        }
        ConnState::Disconnected => ("disconnected", egui::Color32::GRAY),
        ConnState::Error(_) => ("error", egui::Color32::from_rgb(220, 90, 90)),
    };
    ui.colored_label(color, text);
}

/// The LAN IPv4 this device would use to reach other hosts (no traffic sent).
fn local_ipv4() -> Option<String> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    // 192.0.2.1 is TEST-NET-1 (RFC 5737) — never actually contacted; this just
    // makes the OS pick the outbound interface.
    sock.connect("192.0.2.1:9").ok()?;
    match sock.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(v4) => Some(v4.to_string()),
        other => Some(other.to_string()),
    }
}

/// Best-effort tray icon. Returns `(None, None)` if the platform refuses it; the
/// settings window remains the primary UI.
fn build_tray() -> (Option<tray_icon::TrayIcon>, Option<tray_icon::menu::MenuId>) {
    use tray_icon::menu::{Menu, MenuItem};

    let menu = Menu::new();
    let quit = MenuItem::new("Quit ScreenLink", true, None);
    let quit_id = quit.id().clone();
    if menu.append(&quit).is_err() {
        return (None, None);
    }

    let icon = make_icon();
    match tray_icon::TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("ScreenLink")
        .with_icon(icon)
        .build()
    {
        Ok(t) => (Some(t), Some(quit_id)),
        Err(e) => {
            tracing::warn!("tray icon unavailable: {e}");
            (None, None)
        }
    }
}

/// The ScreenLink mark rendered for the tray (see `crate::icon`).
fn make_icon() -> tray_icon::Icon {
    const N: u32 = 32;
    tray_icon::Icon::from_rgba(crate::icon::rgba(N), N, N).expect("valid icon")
}
