# テスト実行結果レポート (`termway`)

本レポートは、[../testing.md](../testing.md) に定義された全テストスイートの `v0.24.0` 実行検証結果です。

## 計測・実行環境

| 項目 | 値 |
| :--- | :--- |
| 対象バージョン | `v0.24.0` |
| 実行日 | `2026-11-22` |
| 実行コマンド | `cargo test --locked` |
| 合計 | **156 passed / 0 failed / 0 ignored** |

## テストスイート別実行結果

| テストターゲット | 通過 (Passed) | 失敗 (Failed) | 除外 (Ignored) | 所要時間 |
| :--- | :---: | :---: | :---: | :---: |
| `src/lib.rs` (unittests) | 8 | 0 | 0 | 1.21s |
| `tests/integration_pipeline.rs` | 11 | 0 | 0 | 1.52s |
| `tests/perf_pipeline.rs` | 2 | 0 | 0 | 20.95s |
| `tests/system_e2e_gui.rs` | 2 | 0 | 0 | 0.05s |
| `tests/unit_compositor.rs` | 44 | 0 | 0 | 0.08s |
| `tests/unit_damage_tracker.rs` | 9 | 0 | 0 | 0.33s |
| `tests/unit_jpeg_simd.rs` | 5 | 0 | 0 | 3.94s |
| `tests/unit_keyboard.rs` | 6 | 0 | 0 | 0.00s |
| `tests/unit_kitty_encoder.rs` | 20 | 0 | 0 | 1.69s |
| `tests/unit_pipeline.rs` | 4 | 0 | 0 | 0.33s |
| `tests/unit_secret_bridge.rs` | 6 | 0 | 0 | 0.05s |
| `tests/unit_session.rs` | 7 | 0 | 0 | 0.02s |
| `tests/unit_sgr_mouse.rs` | 7 | 0 | 0 | 0.00s |
| `tests/unit_sixel_encoder.rs` | 7 | 0 | 0 | 0.50s |
| `tests/unit_terminal.rs` | 6 | 0 | 0 | 0.00s |
| `tests/unit_trace.rs` | 8 | 0 | 0 | 0.00s |
| `tests/unit_vectored.rs` | 4 | 0 | 0 | 0.05s |
| **合計 (17 スイート)** | **156** | **0** | **0** | — |
