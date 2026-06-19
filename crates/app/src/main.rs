//! ScreenLink entry point.
//!
//! Normal launch starts discovery + the network layer and opens the tray/settings
//! UI (the `gui` feature). `--loopback` runs a self-contained two-peer demo over
//! localhost so the Phase 1 vertical slice can be exercised on one machine.

// Hide the console window for GUI release builds on Windows.
#![cfg_attr(
    all(windows, feature = "gui", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod clipboardsync;
#[cfg(feature = "gui")]
mod icon;
mod inputloop;
mod logging;
mod loopback;
mod net;
#[cfg(feature = "gui")]
mod ui;

use net::{AppCore, NetCommand, NetEvent};
use screenlink_core::config::{self, AppConfig};
use screenlink_core::security::Identity;
use screenlink_core::trust::TrustStore;
use std::sync::{Arc, Mutex};

fn main() -> anyhow::Result<()> {
    logging::init();
    screenlink_core::init_crypto();
    screenlink_input::set_dpi_aware();

    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--loopback") {
        return loopback::run();
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("ScreenLink — software KVM for Windows");
        println!("Usage: screenlink [--loopback] [--headless]");
        println!("  --loopback   run a one-machine host+client demo over localhost");
        println!("  --headless   run the network/tray service without the settings window");
        return Ok(());
    }
    let headless = args.iter().any(|a| a == "--headless");

    // --- Load persistent state ---
    let dir = config::data_dir();
    std::fs::create_dir_all(&dir).ok();
    let identity = Arc::new(Identity::load_or_generate(&dir)?);
    let trust = Arc::new(TrustStore::load(&config::trust_path())?);
    let cfg = AppConfig::load_or_default(&config::config_path());
    let config = Arc::new(Mutex::new(cfg.clone()));

    tracing::info!(
        "this device: {} ({})",
        cfg.device_name,
        identity.device_id().short()
    );

    // --- Channels ---
    let (ev_tx, ev_rx) = tokio::sync::mpsc::unbounded_channel::<NetEvent>();
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel::<NetCommand>(64);

    // --- Core ---
    let injector = Arc::new(Mutex::new(screenlink_input::new_injector()?));
    let make_capturer: net::CapturerFactory = Arc::new(screenlink_input::new_capturer);
    let core = AppCore {
        identity: identity.clone(),
        trust: trust.clone(),
        control_port: cfg.control_port,
        realtime_port: cfg.realtime_port,
        device_name: cfg.device_name.clone(),
        events: ev_tx,
        injector,
        inbound_rt: Arc::new(Mutex::new(None)),
        make_capturer,
    };

    // --- Async runtime (background threads) ---
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    {
        let core = core.clone();
        rt.spawn(async move { net::run(core, cmd_rx).await });
    }

    // --- Discovery ---
    let discovery = screenlink_discovery::Discovery::start(
        &cfg.device_name,
        identity.fingerprint(),
        cfg.control_port,
    );

    if headless {
        run_headless(rt, ev_rx);
        return Ok(());
    }

    #[cfg(feature = "gui")]
    {
        ui::run(ui::UiDeps {
            core,
            discovery,
            trust,
            config,
            cmd_tx,
            ev_rx,
            _rt: rt,
        })?;
    }
    #[cfg(not(feature = "gui"))]
    {
        let _ = (discovery, cmd_tx);
        run_headless(rt, ev_rx);
    }
    Ok(())
}

/// Keep the service alive and log network events (no window).
fn run_headless(
    rt: tokio::runtime::Runtime,
    mut ev_rx: tokio::sync::mpsc::UnboundedReceiver<NetEvent>,
) {
    tracing::info!("running headless; press Ctrl+C to quit");
    rt.block_on(async move {
        loop {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    tracing::info!("shutting down");
                    break;
                }
                ev = ev_rx.recv() => {
                    match ev {
                        Some(e) => tracing::info!("event: {e:?}"),
                        None => break,
                    }
                }
            }
        }
    });
}
