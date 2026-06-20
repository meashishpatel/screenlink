# Claude cowork / computer-use test prompt for ScreenLink

## Important scope note

Computer-use (Claude cowork) drives **one** desktop. ScreenLink's core feature —
moving the cursor/keyboard from one laptop to **another** — needs **two physical
machines**, so the real "cursor crosses to the second laptop and I can type
there" test **cannot** be done by a single-desktop agent. That part stays a
manual two-laptop test (see `docs/manual-test-checklist.md`).

What computer-use *can* verify on one machine:
- the app builds and launches, the window/tray render,
- the `--loopback` self-test passes (this exercises the full pair → cross-edge →
  relay input → inject pipeline in-process),
- the settings UI works (device list, arrangement, connectivity self-test).

Paste the prompt below into Claude cowork (computer-use enabled) on the Windows
dev machine.

---

## Prompt to paste

> You are testing a Windows app called **ScreenLink** (a software KVM). Please run
> these checks and report what you observe, with screenshots.
>
> **1. Loopback self-test (most important).** Open a terminal in the project
> folder and run:
> ```
> set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
> cargo run -p screenlink-app -- --loopback
> ```
> Wait ~10 seconds. Report the final block — it should end with
> `RESULT: ✅ PASS — host paired with client and drove its input over the
> encrypted channel`, and list ~6 injected input events. If it says FAIL or
> panics, capture the full output.
>
> **2. Launch the GUI.** Run `cargo run -p screenlink-app` (or launch the
> installed **ScreenLink** from the Start menu). Confirm:
> - a window titled **ScreenLink** opens with the two-screens app icon,
> - the header shows "This device: … (id …)" and "Reachable at <ip>:47820",
> - a system-tray icon appears.
> Screenshot the window.
>
> **3. Explore the settings UI.** In the window:
> - expand **"Devices on this network"**, **"Screen arrangement"**,
>   **"Manual connection & diagnostics"**, and **"Paired devices"**.
> - In *Manual connection*, type IP `127.0.0.1` and port `1`, click **Self-test**,
>   and report the message it shows (it should explain the port is closed /
>   unreachable — that proves the diagnostic works).
> - In *Screen arrangement*, change the **Edge** dropdown (Left/Right/Top/Bottom)
>   and confirm the little diagram moves the "peer" box accordingly.
> Screenshot each.
>
> **4. Report.** Summarize: did loopback PASS? Did the window and tray render? Did
> the self-test and arrangement controls respond? Note anything broken or visually
> off.
>
> Do **not** attempt to test controlling a second computer — that needs two
> machines and is out of scope here.

---

## After cowork's single-machine checks

The two-laptop behaviors that just got bug-fixed (cursor crossing direction,
typing into the remote textbox) must be confirmed by hand with two machines on
the same LAN — follow `docs/manual-test-checklist.md`, focusing on:
- cursor moves in the **same** direction you push (no inversion),
- control **stays** on the remote while you type (doesn't snap back),
- text appears in the focused textbox on the second laptop.
