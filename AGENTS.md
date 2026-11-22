# AGENTS.md — AI Agent Operational Guide for termway

This file provides concise repository structure, build/test commands, and architectural invariants for AI coding agents. Human-oriented background and comparisons live in `docs/overview.md`.

## 1. Repository Map (Rust / Smithay / XWayland)

- `src/main.rs` — CLI entrypoint, argument parsing, `--query`, signal/panic setup
- `src/lib.rs` — Core library exports & public pipeline/JPEG modules
- `src/error.rs` — Unified error types
- `src/compositor/` — Smithay Wayland compositor + XWayland (`state.rs`, `window_manager.rs`, `window.rs`, `renderer.rs`, `input.rs`, `decoration.rs`, `taskbar.rs`, `xwayland.rs`, `env.rs`, `profile.rs`, `screenshot.rs`, `handlers/`)
- `src/display/` — Framebuffer & SIMD tile damage tracking (`framebuffer.rs`, `damage.rs`)
- `src/protocol/` — Kitty (`kitty.rs`), Sixel (`sixel.rs`), pure-Rust SIMD JPEG (`jpeg.rs`, `jpeg_simd/{mod,avx2,neon}.rs`), SGR mouse (`sgr_mouse.rs`), Kitty keyboard (`keyboard.rs`)
- `src/backend/` — Double-buffered async encode/write threads & `writev` scatter-gather I/O (`terminal.rs`, `vectored.rs`)
- `src/session/` — Hybrid D-Bus session routing, Snap shim, Polkit, Secret Service bridge (`mod.rs`, `secret_bridge.rs`)
- `src/terminal/` — Raw mode RAII guard, terminal capability query, winsize, text sanitization (`raw.rs`, `query.rs`, `winsize.rs`, `sanitize.rs`)
- `src/trace/` — Zero-allocation 32B ring-buffer timeline tracer & Chrome Trace JSON writer (`event.rs`, `frame.rs`, `ring.rs`, `writer.rs`)
- `tests/` — Unit (`unit_*.rs`), integration (`integration_pipeline.rs`), E2E (`system_e2e_gui.rs`), benchmarks (`perf_pipeline.rs`), `gui_test_driver.py`
- `scripts/` — `setup_cross_aarch64.sh`, demo GIF recording (`record_demo_gif.*`)
- `docs/` — Engineering docs (Japanese default, lowercase `kebab-case`)

## 2. Build & Test Commands

```bash
# Build debug & release
cargo build
cargo build --release --locked

# Run all unit, integration, benchmark, and E2E tests
cargo test
cargo test --release

# Targeted test suites (e.g., cross/QEMU SIMD-safe subset)
cargo test --test unit_compositor --test unit_trace --test unit_pipeline --test unit_jpeg_simd --test unit_vectored
```

## 3. Critical Architectural Invariants

1. **Zero-Allocation Hot Paths**: Do not allocate heap memory on the render/encode/PTY-write loops or inside `src/trace/` ring recording. Reuse pre-allocated ping-pong buffers (`WriteHandoff`).
2. **State Integrity (`INV-1`)**: Stateful encoders (`SixelStreamEncoder`, `KittyStreamEncoder`) may drop raw frames *before* encoding, but **never** drop an already-encoded frame before PTY transmission.
3. **Bit-Exact SIMD JPEG (`INV-4`)**: `avx2.rs` and `neon.rs` must remain bit-exact with scalar `jpeg.rs` (`i32` arithmetic; no lossy 16-bit rounding shortcuts) and fall back cleanly on edge blocks or non-SIMD CPUs (`TERMWAY_JPEG_SIMD=0`).
4. **Safe Shutdown & Terminal Restoration (`INV-5`)**: Always join `Encode` thread before `Write` thread so `?2026h`/`?2026l` synchronized output brackets match, and never bypass `RawModeGuard` terminal restoration on exit/panic.
5. **Security Boundaries**: Preserve allow-lists, size limits, and opt-out flags (`--no-host-keyring`, `--no-osc52`, `--no-polkit`) in `src/session/`, `src/terminal/`, and `src/compositor/`.

## 4. Documentation Rules

- Keep `README.md` and `README.ja.md` as 1:1 structural twins.
- Keep `docs/` in Japanese (by default) and lowercase `kebab-case` (except `README.md` hubs).
- Record architectural trade-offs under `docs/adr/NNNN-<title>.md` + update `docs/adr/README.md`; place major subsystem packs under `docs/design/<feature>/`; place benchmarks/reports under `docs/reports/`; place media under `docs/assets/`.
