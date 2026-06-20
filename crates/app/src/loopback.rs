//! `--loopback` dev mode: run a full host→client Control session between two
//! in-process peers over localhost, with no second machine and no real input
//! hooks. It drives a scripted sequence of input through the real transport
//! (TLS pairing + encrypted UDP) and asserts the controlled side injected it.
//!
//! This is both a manual smoke test you can run by hand and the backbone of the
//! integration test in `tests/`.

use crate::net::{self, AppCore, ConnState, NetCommand, NetEvent};
use screenlink_core::protocol::{InputEvent, Key, MouseButton, ScreenEdge};
use screenlink_core::security::Identity;
use screenlink_core::trust::TrustStore;
use screenlink_input::{CapturedEvent, Capturer, Injector, Rect};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Outcome of a loopback run, used by both the CLI and the integration test.
#[derive(Debug, Clone)]
pub struct LoopbackReport {
    pub injected: Vec<InputEvent>,
    pub cursor_sets: usize,
    pub a_trusts_b: bool,
    pub b_trusts_a: bool,
}

impl LoopbackReport {
    pub fn passed(&self) -> bool {
        // The host relays absolute position (MouseMoveAbs); accept either form.
        let has_move = self.injected.iter().any(|e| {
            matches!(
                e,
                InputEvent::MouseMove { .. } | InputEvent::MouseMoveAbs { .. }
            )
        });
        let has_button = self
            .injected
            .iter()
            .any(|e| matches!(e, InputEvent::MouseButton { pressed: true, .. }));
        let has_key = self
            .injected
            .iter()
            .any(|e| matches!(e, InputEvent::Key { .. }));
        has_move && has_button && has_key && self.a_trusts_b && self.b_trusts_a
    }
}

/// CLI entry: run the demo and print a human summary.
pub fn run() -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let report = rt.block_on(drive())?;

    println!("\n===== ScreenLink loopback demo =====");
    println!(
        "Pairing: A↔B trusted = {} / {}",
        report.a_trusts_b, report.b_trusts_a
    );
    println!("Cursor placements on client: {}", report.cursor_sets);
    println!("Input events injected on client: {}", report.injected.len());
    for e in &report.injected {
        println!("   • {e:?}");
    }
    if report.passed() {
        println!("\nRESULT: ✅ PASS — host paired with client and drove its input over the encrypted channel.");
        Ok(())
    } else {
        println!("\nRESULT: ❌ FAIL — see events above.");
        Err(anyhow::anyhow!(
            "loopback demo did not meet success criteria"
        ))
    }
}

/// Build two peers, pair them, and drive scripted input A→B. Returns what B got.
pub async fn drive() -> anyhow::Result<LoopbackReport> {
    screenlink_core::init_crypto();

    let id_a = Arc::new(Identity::generate()?);
    let id_b = Arc::new(Identity::generate()?);
    let fp_a = id_a.fingerprint().to_string();
    let fp_b = id_b.fingerprint().to_string();

    let trust_a = Arc::new(TrustStore::in_memory());
    let trust_b = Arc::new(TrustStore::in_memory());

    // Shared sink so the test can see what the client injected.
    let injected: Arc<Mutex<Vec<InputEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let cursor_sets = Arc::new(AtomicUsize::new(0));

    let (ev_a_tx, ev_a_rx) = tokio::sync::mpsc::unbounded_channel::<NetEvent>();
    let (ev_b_tx, ev_b_rx) = tokio::sync::mpsc::unbounded_channel::<NetEvent>();
    let (cmd_a_tx, cmd_a_rx) = tokio::sync::mpsc::channel::<NetCommand>(32);
    let (cmd_b_tx, cmd_b_rx) = tokio::sync::mpsc::channel::<NetCommand>(32);

    // Host A: controller. Its injector is unused; its capturer is scripted.
    let core_a = AppCore {
        identity: id_a.clone(),
        trust: trust_a.clone(),
        control_port: 47820,
        realtime_port: 47821,
        device_name: "Loopback-A".into(),
        events: ev_a_tx,
        injector: Arc::new(Mutex::new(Box::new(RecordingInjector::new(
            injected.clone(),
            cursor_sets.clone(),
        )) as Box<dyn Injector>)),
        inbound_rt: Arc::new(Mutex::new(None)),
        make_capturer: Arc::new(|| Ok(Box::new(ScriptedCapturer::new()) as Box<dyn Capturer>)),
    };

    // Client B: controlled. Its injector records what it's told to do.
    let core_b = AppCore {
        identity: id_b.clone(),
        trust: trust_b.clone(),
        control_port: 47830,
        realtime_port: 47831,
        device_name: "Loopback-B".into(),
        events: ev_b_tx,
        injector: Arc::new(Mutex::new(Box::new(RecordingInjector::new(
            injected.clone(),
            cursor_sets.clone(),
        )) as Box<dyn Injector>)),
        inbound_rt: Arc::new(Mutex::new(None)),
        make_capturer: Arc::new(|| anyhow::bail!("client never captures")),
    };

    tokio::spawn(net::run(core_a, cmd_a_rx));
    tokio::spawn(net::run(core_b, cmd_b_rx));

    // Auto-confirm pairing on both sides (simulating the user clicking "match").
    spawn_auto_confirm(ev_a_rx, cmd_a_tx.clone(), "A");
    spawn_auto_confirm(ev_b_rx, cmd_b_tx.clone(), "B");

    // Let the listeners bind.
    tokio::time::sleep(Duration::from_millis(400)).await;

    // A controls B (B is to the right of A).
    cmd_a_tx
        .send(NetCommand::Connect {
            addr: "127.0.0.1:47830".parse().unwrap(),
            name: "Loopback-B".into(),
            edge: ScreenEdge::Right,
        })
        .await
        .ok();

    // Let pairing + the scripted input play out.
    tokio::time::sleep(Duration::from_secs(4)).await;

    let captured = injected.lock().unwrap().clone();
    Ok(LoopbackReport {
        injected: captured,
        cursor_sets: cursor_sets.load(Ordering::Relaxed),
        a_trusts_b: trust_a.is_trusted(&fp_b),
        b_trusts_a: trust_b.is_trusted(&fp_a),
    })
}

fn spawn_auto_confirm(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<NetEvent>,
    cmd_tx: tokio::sync::mpsc::Sender<NetCommand>,
    who: &'static str,
) {
    tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            match ev {
                NetEvent::PairingPrompt {
                    fingerprint, pin, ..
                } => {
                    tracing::info!("[{who}] comparison PIN {pin} — auto-confirming");
                    let _ = cmd_tx
                        .send(NetCommand::ConfirmPairing { fingerprint })
                        .await;
                }
                NetEvent::PeerState {
                    state: ConnState::Connected { controlling },
                    ..
                } => {
                    tracing::info!("[{who}] connected (controlling={controlling})");
                }
                NetEvent::Rtt { rtt_ms, .. } => tracing::debug!("[{who}] rtt {rtt_ms:.1}ms"),
                _ => {}
            }
        }
    });
}

// ---- Test doubles implementing the real input traits ----

struct RecordingInjector {
    sink: Arc<Mutex<Vec<InputEvent>>>,
    cursor_sets: Arc<AtomicUsize>,
}

impl RecordingInjector {
    fn new(sink: Arc<Mutex<Vec<InputEvent>>>, cursor_sets: Arc<AtomicUsize>) -> Self {
        Self { sink, cursor_sets }
    }
}

impl Injector for RecordingInjector {
    fn inject(&mut self, ev: InputEvent) -> anyhow::Result<()> {
        self.sink.lock().unwrap().push(ev);
        Ok(())
    }
    fn desktop_rect(&self) -> Rect {
        Rect::new(0, 0, 1920, 1080)
    }
    fn set_cursor_norm(&mut self, _x: f32, _y: f32) -> anyhow::Result<()> {
        self.cursor_sets.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

/// Emits a fixed sequence: cross the right edge, move around, click, type. The
/// initial gate gives pairing + realtime setup time to complete first.
struct ScriptedCapturer {
    queue: Mutex<VecDeque<CapturedEvent>>,
    start: Instant,
}

impl ScriptedCapturer {
    fn new() -> Self {
        let mut q = VecDeque::new();
        // 1) Cross to remote at the right edge (absolute x beyond the desktop).
        q.push_back(CapturedEvent::Move {
            dx: 0,
            dy: 0,
            abs_x: 1_000_000,
            abs_y: 540,
        });
        // 2-3) Relative motion to relay.
        q.push_back(CapturedEvent::Move {
            dx: 25,
            dy: 0,
            abs_x: 1_000_000,
            abs_y: 540,
        });
        q.push_back(CapturedEvent::Move {
            dx: 0,
            dy: 18,
            abs_x: 1_000_000,
            abs_y: 558,
        });
        // 4-5) A left click.
        q.push_back(CapturedEvent::Input(InputEvent::MouseButton {
            button: MouseButton::Left,
            pressed: true,
        }));
        q.push_back(CapturedEvent::Input(InputEvent::MouseButton {
            button: MouseButton::Left,
            pressed: false,
        }));
        // 6-7) Type 'A'.
        q.push_back(CapturedEvent::Input(InputEvent::Key {
            key: Key::A,
            pressed: true,
        }));
        q.push_back(CapturedEvent::Input(InputEvent::Key {
            key: Key::A,
            pressed: false,
        }));
        Self {
            queue: Mutex::new(q),
            start: Instant::now(),
        }
    }
}

impl Capturer for ScriptedCapturer {
    fn poll(&self, timeout: Duration) -> Option<CapturedEvent> {
        std::thread::sleep(timeout.min(Duration::from_millis(50)));
        // Hold off until pairing + realtime channel are ready.
        if self.start.elapsed() < Duration::from_millis(1500) {
            return None;
        }
        self.queue.lock().unwrap().pop_front()
    }
    fn set_suppress(&self, _suppress: bool) {}
    fn park_cursor(&self, _x: i32, _y: i32) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End-to-end Phase 1 slice over localhost: two peers discover nothing
    /// (explicit connect), pair via the comparison PIN, open the encrypted UDP
    /// channel, and the controller drives the controlled side's input. This is
    /// the automated form of `--loopback`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn loopback_pairs_and_relays_input() {
        let report = drive().await.expect("loopback drive");
        assert!(report.a_trusts_b, "host should have paired with client");
        assert!(report.b_trusts_a, "client should have paired with host");
        assert!(
            report
                .injected
                .iter()
                .any(|e| matches!(e, InputEvent::MouseButton { pressed: true, .. })),
            "client should have received a mouse click; got {:?}",
            report.injected
        );
        assert!(
            report
                .injected
                .iter()
                .any(|e| matches!(e, InputEvent::Key { .. })),
            "client should have received a keystroke; got {:?}",
            report.injected
        );
        assert!(report.passed(), "loopback report did not pass: {report:?}");
    }
}
