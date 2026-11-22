# Changelog

All notable changes to **termway** will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.24.0] - 2026-11-22

### Changed
- Bumped version to `0.24.0` and verified all 156 tests across 17 Rust test suites.
- Standardized OSS repository structure: added `CHANGELOG.md`, `CONTRIBUTING.md`, `SECURITY.md`, `AGENTS.md`, and `.github/` Issue/PR templates.
- Normalized `docs/` layout to lowercase `kebab-case`: moved `docs/demo.gif` to `docs/assets/demo.gif`, moved `docs/pipeline/` and `docs/trace/` under `docs/design/`, renamed `ARCHITECTURE.md` / `ROADMAP.md` to `architecture.md` / `roadmap.md`, and split `SPECIFICATION.md` into `requirements.md`, `testing.md`, and `reports/test-results.md`.
- Added `docs/README.md`, `docs/overview.md`, `docs/guides/usage.md`, and `docs/reports/README.md`.
- Aligned `docs/architecture.md`, `docs/roadmap.md`, `docs/testing.md`, `docs/reports/`, and README test counts with `v0.24.0`.

## [0.23.0] - 2026-11-14

### Security
- Hardened D-Bus Secret Service bridge, OSC 52 clipboard handling, runtime directory (`XDG_RUNTIME_DIR`) checks, and input size limits.

## [0.22.0] - 2026-11-12

### Changed
- Prevented Kitty false-positives in VS Code integrated terminal and preferred Sixel protocol.
- Removed obsolete `--wayland` CLI option.

## [0.21.0] - 2026-11-11

### Added
- Added 8-direction window resizing and partial off-screen window dragging.

### Fixed
- Resolved pointer mutex self-deadlock in cursor image handling and immediately updated resize arrow cursors on hover.
- Fixed CSD resize hit geometry, SSD titlebar drag, and client grab release.

## [0.20.0] - 2026-11-09

### Added
- Split encode and PTY write into a double-buffered asynchronous pipeline (`writev` scatter-gather output, AVX2 & ARM NEON SIMD baseline JPEG encoder, ADR-0005).

## [0.19.0] - 2026-11-09

### Fixed
- Separated Wayland `wl_pointer.frame` events and added horizontal scroll support (`WheelLeft` / `WheelRight`, ADR-0004).

## [0.18.0] - 2026-11-08

### Added
- Added zero-allocation timeline tracer (`--trace`, Chrome Trace Event JSON for Perfetto, `frame_id` async flows, WM state tracks, snapshot rotation, panic flush, ADR-0003).

## [0.17.0] - 2026-11-07

### Fixed
- Improved multi-batch XKB Unicode keymap rollover tests and popup focus handling.

## [0.16.1] - 2026-11-07

### Changed
- Updated documentation for Windows Terminal Sixel and Noctty `1.3.136`.

## [0.16.0] - 2026-11-03

### Added
- Added signal-safe and panic-safe terminal restoration, `--query` capability diagnostics, and p99 latency profiling.

## [0.1.0] - [0.15.0] - 2026-10-08 to 2026-11-01

### Added
- Built Smithay headless Wayland compositor + XWayland backend (replacing legacy Xvfb engine), Kitty & Sixel graphics encoders with partial placement and dirty-band caching, AVX2/NEON damage tracking, hybrid D-Bus session routing, Polkit integration, OSC 52 / Primary Selection clipboard sync, OSC 2 title sync, OSC 22 cursor shapes, SGR-Pixels (`1016h`), `zlib-ng` SIMD compression, and Kitty `f=101` JPEG support.
