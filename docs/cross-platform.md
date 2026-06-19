# Cross-platform support

ScreenLink is built Windows-first with clean platform seams so Linux and macOS
can be added without touching the shared core. This document tracks status and
the plan for the remaining backends.

## Status

| Capability | Windows | Linux | macOS |
|---|---|---|---|
| Discovery, pairing, TLS transport, encrypted UDP | ✅ | ✅ | ✅ |
| Clipboard text sync (`arboard`) | ✅ | ✅ | ✅ |
| Settings GUI (egui) | ✅ | ✅¹ | ✅¹ |
| **Portable key model** (`core::protocol::Key`) | ✅ | ✅ | ✅ |
| **Input injection** (be a *client*) | ✅ | ✅¹ | ✅¹ |
| **Input capture** (be a *host*) | ✅ | 🧪² | 🧪² |
| Extend mode (wireless monitor) | ⏳ Phase 2 | ✕ | ✕ |

✅ done · 🧪 experimental (compiles in CI; needs on-device testing) · ⏳ planned · ✕ n/a
¹ Injection on Linux/macOS uses the cross-platform `enigo` backend
(`crates/input/src/unix.rs`); it compiles in CI but its runtime behavior needs
verification on a real Linux/macOS box. Key mapping is best-effort. macOS requires
the user to grant **Accessibility** permission. GUI builds on Linux/macOS but needs
the desktop system libraries below.

² Capture/host on Linux/macOS uses a global input grab (`rdev`,
`crates/input/src/unix.rs`). It compiles in CI but is **experimental**: it needs
real on-device testing, works on **X11** (not Wayland) on Linux, needs macOS
**Accessibility + Input Monitoring** permission, suppression is best-effort on
Linux, and edge detection currently assumes a 1920×1080 desktop until a per-OS
display-size query is added.

With injection + capture on every OS, **any device can in principle control any
other** — Windows↔Windows is verified; the Linux/macOS paths are experimental and
need validation on real hardware.

The shared code (everything except input injection/capture and the GUI system
deps) is compile-checked **and** unit/loopback-tested on Windows, Linux, and
macOS in CI on every push — so cross-platform regressions are caught even without
local machines.

## The portable key model (done)

`core::protocol::Key` names physical key **positions** (US-layout, HID-style),
not any OS's virtual-key codes. Each OS backend maps `Key` ⇆ its native codes:

- Windows: `crates/input/src/keymap_win.rs` (`Key` ⇆ virtual-key, tested).
- Linux/macOS: to be added alongside their backends.

This is what makes cross-OS typing correct: pressing physical **A** on a Windows
host lands as **A** on a Linux client, and `Char('é')` is typed as text where no
positional name exists.

## Adding the Linux backend (next)

Implement `Injector` + `Capturer` (the traits in `crates/input/src/lib.rs`) for
`#[cfg(target_os = "linux")]`:

- **Injection:** X11 `XTEST` (`XTestFakeKeyEvent`/`XTestFakeButtonEvent`/
  `XTestFakeMotionEvent`) via the `x11`/`xcb` crates. Wayland has no portable
  global injection — fall back to `uinput` (`/dev/uinput`, needs the user in the
  `input` group or a udev rule).
- **Capture (with suppression):** `XRecord` or `XInput2` for X11; for Wayland,
  read `/dev/input/event*` via `evdev` and `EVIOCGRAB` to grab (needs permission).
- **Key map:** add `keymap_linux.rs` mapping `Key` ⇆ X11 keysyms / evdev codes.

## Adding the macOS backend (next)

For `#[cfg(target_os = "macos")]`:

- **Injection:** `CGEventCreateKeyboardEvent` + `CGEventPost`, and
  `CGEventCreateMouseEvent` for the mouse (via the `core-graphics` crate).
- **Capture (with suppression):** `CGEventTap` at
  `kCGHIDEventTap` with the tap callback returning `NULL` to swallow events.
- **Permission:** the user must grant **Accessibility** (and, for capture,
  **Input Monitoring**) in System Settings → Privacy & Security. The app should
  detect denial and show instructions.
- **Key map:** add `keymap_macos.rs` mapping `Key` ⇆ macOS virtual key codes
  (`kVK_*`).

## Building per OS

```bash
# Windows (full, incl. GUI + native input)
cargo build --release

# Linux/macOS — shared/headless code today:
cargo build --workspace --no-default-features
cargo test  --workspace --no-default-features   # runs the loopback integration test too

# Linux GUI build (once input backends land) needs desktop dev libraries, e.g.
# Debian/Ubuntu:
sudo apt-get install -y libgtk-3-dev libxdo-dev libxcb1-dev libxcb-render0-dev \
  libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev libssl-dev
```

## Extend mode stays Windows-only

Desktop Duplication, Media Foundation, and the IddCx virtual-display driver are
Windows-only APIs with no portable equivalents, so Phase 2 (wireless monitor)
will not be offered on Linux/macOS.
