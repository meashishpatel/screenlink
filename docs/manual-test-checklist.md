# Manual two-laptop test checklist (Phase 1)

Automated coverage lives in unit tests + the `--loopback` integration test. This
checklist covers what only real hardware can verify: latency *feel*, real input
hooks, multi-monitor/DPI, and reconnect.

## Setup
- [ ] Install ScreenLink on **both** laptops (or `cargo run -p screenlink-app`).
- [ ] Put both on the **same Wi-Fi/LAN**. Prefer a private network (guest/hotel
      Wi-Fi often blocks device-to-device traffic).
- [ ] Allow ScreenLink through the firewall on **Private** networks if prompted
      (the installer adds this rule automatically).

## Discovery & pairing
- [ ] Each laptop appears in the other's **Devices** list automatically (no IP
      typing).
- [ ] Click **Control** on the host → a 6-digit code appears on **both** screens.
- [ ] Codes **match** → click "Codes match — pair" on both → status becomes
      *Connected*.
- [ ] Wrong/mismatched codes path: verify cancelling leaves you unpaired.

## Edge crossing & input
- [ ] Arrange the client on the correct edge (e.g. Right).
- [ ] Move the cursor off that edge → control crosses to the client; the local
      cursor parks at the seam.
- [ ] Typing and clicking land on the **client**, not the host.
- [ ] Pull back across the seam → control returns home (hysteresis: a tiny wobble
      at the edge must **not** flap).
- [ ] **Snap control home** button (and `Ctrl+Alt+Home`) instantly returns control.
- [ ] Latency feels imperceptible on LAN (target < 10 ms added).

## Multi-monitor / DPI
- [ ] Client with two monitors at different scaling: cursor maps correctly across
      both; no drift or offset.
- [ ] Host at 150% scale, client at 100% (or vice versa): movement stays accurate.

## Clipboard
- [ ] Copy text on host → paste on client.
- [ ] Copy text on client → paste on host.
- [ ] No echo loop (clipboard doesn't ping-pong/flicker).

## Reliability
- [ ] Drop Wi-Fi briefly → app reports disconnect, does **not** leave input stuck
      on the client; control is back on the host.
- [ ] Reconnect happens automatically (or via one click) **without** re-pairing.
- [ ] Quit the client app mid-control → host regains control, no frozen input.

## Connectivity self-test
- [ ] On a network with AP/client isolation, the self-test reports
      *Unreachable / isolation*, not a silent failure.
- [ ] On different subnets, it reports *DifferentSubnet* and suggests manual IP.

## Security
- [ ] An **unpaired** third device cannot gain control (it must pair first).
- [ ] Unpair a device → it can no longer connect without pairing again.
