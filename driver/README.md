# ScreenLink virtual display driver (Phase 2)

Extend mode needs a Windows **IddCx Indirect Display Driver** to create the
virtual monitor that the host captures and streams. This directory holds the
integration notes and (later) the driver shim. **It is not built or shipped in
Phase 1.**

## Why this is special

A kernel-adjacent display driver is the one component that **cannot be written in
Rust** — it uses the Windows Driver Kit (WDK) and the IddCx (C/C++) model. It also
has a hard distribution constraint:

> Windows requires the IddCx driver to be **signed by a certificate chained to a
> Microsoft-trusted root** to install on end-user machines.

For public distribution you must obtain one of:

- **Attestation signing** via the Windows Hardware Developer Program — requires an
  **EV code-signing certificate** + a **Partner Center** account. (Lightest path.)
- **WHQL** certification — heavier, with full HLK testing.

Without either, users would have to enable test-signing mode
(`bcdedit /set testsigning on`), which is a poor UX **and a security downgrade**.
We do not want to require that.

## The plan: drive an existing, already-signed driver

Rather than ship our own unsigned driver, ScreenLink will **drive an existing,
maintained, already-signed open-source IddCx virtual display driver**. Candidates
to evaluate (check current license + signing status before integrating):

- A community **VirtualDisplayDriver** (IddCx-based), or
- Microsoft's **IddSampleDriver** as a base (would still need signing for
  distribution).

ScreenLink talks to the chosen driver to add/remove a virtual monitor at a
selected resolution/refresh, then captures it via the Desktop Duplication API.

## License caution

Keep the integration permissive (MIT/Apache-2.0 compatible). Some reference
encoders (e.g. **Sunshine**) are GPL — use them only as *references*, never as a
linked dependency, or the whole project would have to adopt that license. See the
root `README.md` license note.

## Status

- [ ] Select & vendor/reference a signed IddCx driver
- [ ] Define the host↔driver control shim (add/remove virtual display, set mode)
- [ ] Wire Desktop Duplication capture of the virtual output
- [ ] Media Foundation hardware encode → `screenlink-core::realtime` packets
- [ ] Client decode + D3D11 present
