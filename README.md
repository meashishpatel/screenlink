# ScreenLink

**Low-latency, zero-config software KVM for Windows — control a second laptop with
your keyboard, mouse, and clipboard over the local network. Later: use it as a
wireless extended monitor.**

ScreenLink is an open-source Windows utility in the spirit of Synergy / Barrier /
Mouse Without Borders, written in Rust as a single small native binary.

> **Status: Phase 1 (Control mode) — in development.**
> Extend mode (wireless second monitor) is Phase 2 and is currently feature-gated
> off (`--features extend` builds a stub). See [Roadmap](#roadmap).

---

## Two modes

| Mode | What it does | Status |
|------|--------------|--------|
| **Control mode** (software KVM) | One *host* laptop's keyboard / mouse / clipboard control one or more *client* laptops over the LAN. Move the cursor past a screen edge to "cross" onto the other machine. | Phase 1 |
| **Extend mode** (wireless monitor) | The client becomes an extended display of the host: a virtual monitor is captured, hardware-encoded, and streamed; you drag windows onto it. | Phase 2 (stubbed) |

Both modes share discovery, pairing, security, transport, and UI. You pick a mode
per connected device.

## Key idea: same-LAN, zero-config

When both laptops are on the same Wi-Fi or wired into the same router/switch,
ScreenLink finds the other machine **automatically** (mDNS / DNS-SD, with a UDP
broadcast fallback) — no typing IP addresses in the common case. First connection
asks for a short numeric **PIN** on both devices; after that, devices remember each
other and reconnect automatically.

## Architecture

```
HOST (kbd/mouse + optional virtual display)          CLIENT
 ┌───────────────┐    TLS 1.3 control (TCP)  ┌───────────────┐
 │ Input engine  │◀─────────────────────────▶│ Pairing/cfg   │
 │ Edge detect   │                           │ Clipboard     │
 │ Capture/encode│   encrypted UDP (AEAD)    │ Inject input  │
 │ (extend mode) │══════════════════════════▶│ Decode/present│
 └───────────────┘  input events / video     └───────────────┘
```

Two channels per peer:

- **Control channel** — TCP + **TLS 1.3** (rustls). Pairing, capability negotiation,
  config, mode switching, clipboard, edge-transition handshakes. Reliable + encrypted.
- **Realtime channel** — **UDP, encrypted** with ChaCha20-Poly1305 using a key derived
  from the established TLS session. Carries input events (Control mode) and, later,
  video packets (Extend mode). Sequence numbers + jitter buffer.

### Workspace layout

```
screenlink/
  crates/
    core/        # transport, pairing, security, shared protocol types
    discovery/   # mDNS + manual peers + connectivity self-test
    input/       # capture (Raw Input/hooks) + injection (SendInput) + edge logic
    clipboard/   # text sync (Phase 1); images/files (Phase 3)
    video/        # Phase 2: capture/encode/decode/present  (feature = "extend")
    app/         # tray + egui UI, wiring, config, --loopback dev mode
  driver/        # Phase 2: IddCx virtual display (integration notes, see below)
  installer/     # Inno Setup script (adds firewall rule)
  .github/workflows/
```

## App icon

The tray and window icons are rendered from code ([crates/app/src/icon.rs](crates/app/src/icon.rs)),
so the app always has a proper icon with no asset files to ship. The vector source
is [assets/icon.svg](assets/icon.svg) — two cascading "screens" linked on a blue tile.

To give the **`.exe` itself** an icon in Explorer/taskbar, generate a multi-size
`assets/icon.ico` from the SVG and rebuild (the build script picks it up
automatically):

```bash
# with ImageMagick:
magick assets/icon.svg -background none -define icon:auto-resize=256,64,48,32,16 assets/icon.ico
```

## Platform support

Windows-first, with portable seams. The shared core (discovery, pairing, TLS
transport, encrypted UDP, clipboard, and the **portable key model**) is
compile-checked and tested on **Windows, Linux, and macOS** in CI. Native input
**capture/injection** is implemented on Windows today; Linux (X11/`uinput`) and
macOS (`CGEvent`) backends are next and plug into the same traits. See
[docs/cross-platform.md](docs/cross-platform.md) for the status matrix and plan.

## Build & run

### Prerequisites

- Windows 10 (2004+) or Windows 11, x64
- [Rust](https://rustup.rs) (stable, `x86_64-pc-windows-msvc`)
- Visual Studio Build Tools with the **"Desktop development with C++"** workload
  (provides the MSVC linker + Windows SDK)

### Build

```powershell
cargo build --release
```

The app binary lands at `target\release\screenlink.exe`.

### Run

```powershell
# Normal: start the tray app (advertises on the LAN, opens the settings window)
cargo run -p screenlink-app

# Dev / single-machine: run host + client in one process over loopback
cargo run -p screenlink-app -- --loopback
```

On first launch, Windows may prompt to allow ScreenLink through the firewall —
**allow it on Private networks**. The installer adds this rule automatically.

### Test

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --all -- --check
```

## How a connection works (Phase 1)

1. Both machines run ScreenLink. Each advertises an `_screenlink._tcp` mDNS service.
2. On the host's settings window, the other device appears in the device list.
3. Click **Pair** → a 6-digit PIN shows on both screens → confirm → a long-term
   trusted-device key is stored. Reconnection is automatic thereafter.
4. Arrange the screens (drag the client left/right/top/bottom of the host).
5. Move the mouse off that edge → control crosses to the client; keystrokes and
   clicks go there. Press the **snap-home hotkey** (default `Ctrl+Alt+Home`) to
   yank control back instantly.
6. Copy text on either machine → it's available to paste on the other.

## Security

- **No unauthenticated control, ever.** An unpaired device on the LAN cannot gain
  input control. Pairing requires a PIN confirmed on both devices; reconnection
  requires the stored per-peer key (verified against the peer's TLS certificate
  fingerprint).
- All traffic is encrypted: TLS 1.3 on the control channel, ChaCha20-Poly1305 on
  the realtime channel. No plaintext input or clipboard on the wire.
- The UI shows who is connected and offers one-click **revoke / unpair**.

> ⚠️ **Threat note.** Remote input injection is powerful — a paired host can type
> and click anything on a client. Only pair devices you control. Treat the PIN
> exchange like pairing a Bluetooth device.

The control channel is **TLS 1.3 only**. For the full threat model, what is and
isn't protected, and how to report vulnerabilities, see [SECURITY.md](SECURITY.md).

## Connectivity troubleshooting

"Same Wi-Fi" doesn't always mean "can talk to each other." ScreenLink includes a
**self-test** (Settings → *Test connection*) that reports the actual problem:

- **AP / client isolation** — many guest/hotel/corporate networks block
  device-to-device traffic even on one SSID. Use a private network, Ethernet, or a
  phone hotspot.
- **Different subnet / VLAN** — same SSID, different subnet. Use manual IP entry;
  each peer's IP/subnet is shown in the UI.
- **Windows firewall / Public profile** — on a new network Windows defaults to the
  *Public* profile and blocks inbound connections. Allow ScreenLink, or set the
  network to *Private*. The installer adds the inbound rule.

## Roadmap

- **Phase 1 — Control mode (software KVM).** ← *current.* Discovery, PIN pairing,
  trusted devices, TLS control + encrypted UDP input, edge crossing with hysteresis,
  multi-monitor + DPI, bidirectional text clipboard, tray + settings UI, loopback dev mode.
- **Phase 2 — Extend mode (wireless second monitor).** IddCx virtual display →
  Desktop Duplication capture → Media Foundation hardware H.264/HEVC encode → UDP →
  client DXVA decode → D3D11 flip-model present. Adaptive bitrate, frame pacing.
- **Phase 3 — Polish.** Clipboard images + file transfer, multiple clients, richer
  hotkeys, auto-update, opt-in crash reporting, internet/relay transport hooks.

### ⚠️ Extend mode & driver signing (read before Phase 2)

Extend mode needs an **IddCx Indirect Display Driver**. Windows requires such a
driver to be signed by a certificate chained to a Microsoft-trusted root to install
on end-user machines. For public distribution you must obtain **attestation signing**
via the Windows Hardware Developer Program (EV code-signing cert + Partner Center) or
full WHQL. Without it, users would have to enable Windows test-signing mode
(`bcdedit /set testsigning on`) — poor UX and a security downgrade.

**ScreenLink's plan:** drive an existing, already-signed open-source virtual display
driver rather than ship our own unsigned one, and keep Extend mode an **optional
component** so Phase 1 ships cleanly regardless. See [`driver/README.md`](driver/README.md).

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. All dependencies are permissive
(non-copyleft). If we ever reference copyleft projects (e.g. Sunshine for encoding
ideas), they stay as *references*, not linked dependencies. Contributions are dual-licensed
under these terms.
