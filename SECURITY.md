# Security Policy

ScreenLink lets one computer inject keyboard/mouse input into another. That is a
powerful capability, so security is a first-class concern. This document explains
the threat model, what is and isn't protected, known limitations, and how to
report vulnerabilities.

## Reporting a vulnerability

Please **do not** open a public issue for security problems. Instead, report
privately via GitHub Security Advisories ("Report a vulnerability" on the
Security tab) or email the maintainers listed in `Cargo.toml`. We aim to
acknowledge within 72 hours and to coordinate a fix and disclosure timeline.

## Threat model

**Goal:** an attacker on the same LAN must never be able to control a device they
have not been explicitly paired with, and must never be able to read input or
clipboard traffic in transit.

### What is protected

- **Authentication.** Every device has a long-term self-signed certificate
  generated on first run. Its identity *is* the SHA-256 fingerprint of that
  certificate. Pairing records the peer's fingerprint; reconnection requires the
  peer to (a) present a certificate with that exact fingerprint and (b) prove
  possession of the matching private key — the latter is enforced by the TLS 1.3
  handshake signature, which ScreenLink **does** verify (only chain/PKI
  validation is replaced by fingerprint pinning).
- **First-pairing MITM resistance.** Pairing uses Bluetooth-style **numeric
  comparison**: both devices independently derive the same 6-digit code from the
  two certificate fingerprints and the user confirms they match. A
  man-in-the-middle would terminate two different TLS sessions with two different
  fingerprints, producing two different codes — the user sees the mismatch and
  cancels. No PIN is ever transmitted.
- **Confidentiality + integrity in transit.** The control channel is **TLS 1.3
  only** (1.2 is disabled). The realtime channel (UDP) uses
  **ChaCha20-Poly1305** with a 32-byte key exported from the established TLS
  session (`export_keying_material`) — the UDP key is bound to the authenticated
  control channel and is never sent on the wire. Each direction uses a distinct
  nonce salt; packets carry sequence numbers and are checked against a sliding
  **anti-replay window**, so captured packets can't be re-injected.
- **No action before trust.** A controlled device does not inject input or apply
  clipboard data until the peer is trusted: the input/clipboard handlers run only
  *after* the pairing handshake completes and the fingerprint is in the trust
  store. An unpaired peer that never completes pairing can do neither.
- **Revocation.** Unpairing removes the fingerprint from the trust store; that
  device must pair again (PIN + confirmation) to reconnect.

### What is NOT protected / out of scope (v1)

- **A paired, trusted host is fully trusted.** By design, a device you've paired
  can type and click anything on the controlled machine. Only pair devices you
  control. (Subject to OS limits below.)
- **UIPI / elevation.** `SendInput` cannot drive User Account Control prompts or
  windows owned by a higher-integrity (elevated/admin) process unless ScreenLink
  on the controlled side is itself running elevated. This is a Windows protection,
  not a ScreenLink one.
- **Local attackers / malware on the device.** ScreenLink does not defend against
  code already running on your machine. The long-term private key is stored
  unencrypted under `%LOCALAPPDATA%\ScreenLink` (like an SSH key); it is protected
  only by the OS file permissions of your user account.
- **Denial of service.** v1 does not rate-limit inbound connections or pairing
  attempts, and there is no handshake/pairing timeout. A malicious LAN peer could
  open many connections or spam pairing prompts. This is annoyance-level, not a
  control bypass (control still requires PIN confirmation), but it is a known gap.
- **Single active inbound session.** The realtime inject path tracks one inbound
  key at a time; multiple simultaneous controllers are Phase 3.
- **Network exposure.** The control/realtime sockets bind to all interfaces
  (`0.0.0.0`). Restricting to chosen interfaces is a planned option.
- **Internet/relay.** v1 is same-LAN only; there is no relay/NAT-traversal path.

## Hardening roadmap

- Rate-limit + time-bound the pairing handshake; cap concurrent pending pairings.
- Optional bind-to-interface selection in the UI.
- Optional at-rest encryption of the identity key (e.g. DPAPI on Windows).
- Restrict realtime injection to the currently focused/connected session only.

## Supported versions

ScreenLink is pre-1.0. Security fixes target the latest `main`/release only.

## A note for users

Remote input injection is exactly as dangerous as letting someone sit at your
keyboard. Treat pairing like pairing a Bluetooth device: only confirm the PIN for
a device you physically recognize and trust on a network you trust.
