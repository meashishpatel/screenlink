# Phase 2 — Extend mode architecture & implementation plan

Extend mode turns the client into a wireless extended monitor. This document is
the concrete plan for filling in the stubs in `crates/video` and the
`driver/` directory.

## Current status

**Done (compiles + tested, platform-independent):**
- `crates/video/src/transport.rs` — frame ⇆ chunk packetization with reordering
  tolerance and incomplete-frame handling (unit-tested).
- `crates/video/src/pipeline.rs` — the trait seams (`VirtualDisplay`,
  `FrameSource`, `VideoEncoder`, `VideoDecoder`, `Presenter`), the `Frame` /
  `EncodedFrame` types, and a working pure-Rust `TestPatternSource` so the
  encode/transport path can be developed without a driver or second machine.

**Stubbed (return "not implemented"):** the four native pieces below.

> **The gating blocker is driver signing, not code.** Windows will not install an
> IddCx driver unless it's signed by a Microsoft-trusted root (attestation via the
> Windows Hardware Dev Program — EV cert + Partner Center — or WHQL). Plan to
> *drive an existing signed driver*, not ship our own. See `driver/README.md`.

## Data flow

```
HOST                                                      CLIENT
VirtualDisplay.enable(mode)                               
   └─ IddCx virtual monitor appears                       
FrameSource.next_frame()  (Desktop Duplication)           
   └─ Frame (BGRA)                                        
VideoEncoder.encode(frame) (Media Foundation HW MFT)      
   └─ EncodedFrame (H.264/HEVC)                           
transport::split_frame → RealtimeCrypto.seal → UDP  ───►  RealtimeCrypto.open → transport::Reassembler
                                                             └─ EncodedFrame
                                                          VideoDecoder.decode (MF/DXVA)
                                                             └─ Frame
                                                          Presenter.present (D3D11 flip swapchain)
```

The UDP path reuses `screenlink_core::realtime::RealtimeCrypto` (same encrypted
channel as input), so video inherits the AEAD + replay protection for free. Add a
separate epoch/stream-id if running input and video concurrently.

## Implementation steps

### 1. Virtual display (`VirtualDisplay`)
- **Driver:** integrate a maintained, **already-signed** open-source IddCx driver
  (a community VirtualDisplayDriver, or MS `IddSampleDriver` as a base — but that
  needs signing for distribution). Do **not** write/ship an unsigned driver.
- **Control shim:** the driver typically exposes add/remove-monitor + set-mode via
  a named pipe / registry / its own IOCTL. Implement `enable/disable` against it.
- **Windows APIs (Rust):** `windows::Win32::Devices::Display`,
  `SetupAPI` for device enumeration if needed.

### 2. Capture (`FrameSource`) — Desktop Duplication
Self-contained; needs no driver to prototype (capture the *primary* display first,
switch to the virtual output once the driver works).
- **APIs:** `IDXGIOutputDuplication` via
  `windows::Win32::Graphics::Dxgi` (`IDXGIOutput1::DuplicateOutput`) +
  `windows::Win32::Graphics::Direct3D11` (`D3D11CreateDevice`).
- Loop: `AcquireNextFrame` → map the `ID3D11Texture2D` (copy to a CPU-readable
  staging texture via `CopyResource` + `Map`) → fill `Frame { bgra, stride, .. }`
  → `ReleaseFrame`. Handle `DXGI_ERROR_ACCESS_LOST` (recreate duplication) and the
  "no new frame" timeout (return `Ok(None)`).
- Cargo: add `windows` to `crates/video` with features `Win32_Graphics_Dxgi`,
  `Win32_Graphics_Dxgi_Common`, `Win32_Graphics_Direct3D11`,
  `Win32_Graphics_Direct3D`, `Win32_Foundation`, gated behind `extend`.

### 3. Encode (`VideoEncoder`) — Media Foundation hardware MFT
- **APIs:** `windows::Win32::Media::MediaFoundation`. Use the H.264 (or HEVC)
  encoder MFT in async/hardware mode. Set `MF_MT_SUBTYPE = H264`, input
  `NV12` (convert BGRA→NV12 on GPU via a shader or use the Video Processor MFT),
  bitrate, low-latency (`CODECAPI_AVLowLatencyMode`), GOP, and request IDR on
  `request_keyframe()` (`CODECAPI_AVEncVideoForceKeyFrame`).
- Works across Intel QSV / AMD AMF / NVIDIA NVENC with no vendor SDK.

### 4. Decode + present (`VideoDecoder`, `Presenter`)
- **Decode:** MF H.264 decoder MFT (DXVA2/D3D11 hardware), output to an
  `ID3D11Texture2D`.
- **Present:** `IDXGISwapChain` with `DXGI_SWAP_EFFECT_FLIP_DISCARD`, present the
  decoded texture, vsync off for latency. Window via `winit` (already a dep) or a
  borderless fullscreen `HWND`.

### 5. Pacing / adaptive bitrate
- Measure RTT/loss from the existing control-channel pings + missing chunks in
  `Reassembler`. Lower bitrate / raise keyframe frequency under loss. Pace sends to
  avoid bursts.

## Wiring into the app
- `screenlink_video::start_extend_host(mode)` / `start_extend_client()` are the
  entry points; switch them from the `NotImplemented` stub to construct the real
  trait objects above once each lands.
- The app already exchanges `Capabilities.extend_mode` in the hello, and the UI
  has the per-device mode concept reserved. Add an Extend/Control toggle per device
  and a `SetMode { Extend }` handler that spawns the pipeline instead of the input
  loop.

## Licensing caution
Keep all integrations permissive (MIT/Apache-2.0). Reference GPL encoders (e.g.
**Sunshine**) for *ideas only* — never link them, or the project must adopt GPL.
