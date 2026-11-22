# Contributing to termway

Thank you for considering contributing to **termway**!

## Prerequisites (Linux)

Install the Rust `stable` toolchain and native Wayland / XWayland / Pixman build dependencies (matching `.github/workflows/ci.yml`):

```bash
sudo apt-get update
sudo apt-get install -y --no-install-recommends \
  build-essential pkg-config libx11-dev libxtst-dev libxfixes-dev \
  libpixman-1-dev libxkbcommon-dev libwayland-dev xwayland xvfb openbox
```

## Build & Test

```bash
# Build
cargo build

# Run full unit, integration, and E2E test suite
cargo test

# Run release-mode benchmarks & suite
cargo test --release
```

### Optional `aarch64` Cross-Compilation & QEMU NEON Verification

```bash
sudo ./scripts/setup_cross_aarch64.sh
# See .github/workflows/ci.yml for PKG_CONFIG_PATH and qemu-aarch64 runner flags
```

## Workflow & Conventions

1. Create a topic branch from `main` (`feat/<topic>`, `fix/<topic>`, `perf/<topic>`, `docs/<topic>`).
2. Prefer [Conventional Commits](https://www.conventionalcommits.org/) (`feat(compositor): ...`, `perf(jpeg): ...`, `fix(wm): ...`, `docs: ...`).
3. Preserve critical hot-path invariants (zero heap allocations in render/trace paths, bit-exact SIMD JPEG parity, safe terminal restoration via `RawModeGuard`).
4. Update documentation under `docs/` (`docs/requirements.md`, `docs/architecture.md`, `docs/testing.md`, and an ADR under `docs/adr/` for significant architectural choices) and update `[Unreleased]` in `CHANGELOG.md`.
5. Keep `README.md` and `README.ja.md` synchronized as 1:1 structural twins.
6. Fill out `.github/PULL_REQUEST_TEMPLATE.md` when opening a PR.
