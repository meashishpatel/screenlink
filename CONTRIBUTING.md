# Contributing to ScreenLink

Thanks for your interest! ScreenLink is a Rust workspace targeting Windows.

## Getting set up

1. Install [Rust](https://rustup.rs) (stable, `x86_64-pc-windows-msvc`).
2. Install Visual Studio Build Tools with the **Desktop development with C++** workload.
3. `cargo build --workspace`
4. `cargo test --workspace`

For day-to-day work without a second machine, use the loopback dev mode:

```powershell
cargo run -p screenlink-app -- --loopback
```

## Before you open a PR

Please make sure these pass locally — CI enforces them:

```powershell
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

- Keep modules small and interfaces clean — Phase 2 (video) should plug in without
  rewrites.
- Add tests alongside new logic (transport framing, coordinate/DPI mapping, edge
  transitions, crypto handshake are the high-value areas).
- If a dependency or platform API doesn't behave as documented, open an issue and
  propose options rather than working around it silently.

## Commit / PR conventions

- One logical change per PR; describe what and why.
- Reference related issues.
- By contributing you agree your work is dual-licensed under MIT OR Apache-2.0.

## Code layout

See the workspace table in the [README](README.md#workspace-layout). The dependency
direction is one-way: `app` depends on the feature crates; the feature crates depend
on `core`; `core` depends on none of them.
