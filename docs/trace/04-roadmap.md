# 04. 実装ロードマップ

トレーシングは手戻りのないよう `src/trace/` コア基盤から段階導入し、各ステップを独立したコミットとして実装・検証します。既存集計（`--profile`）の後方互換性を常に維持します。

---

## M1 — `src/trace/` コア基盤（32B イベント・スレッド独立リング・Chrome JSON / `.gz` 出力）

**目的**：32 バイト固定長 `TraceEvent`、スレッド独立 `ThreadRing`（`drop-oldest` + `overflow` カウンタ）、および Chrome Trace Event JSON（`.gz` 圧縮対応）出力器を構築し、まずはパイプライン基本スライス（`Render`, `DamageScan`, `BackendEncode`, `BackendWrite`）とレイテンシカウンタを出力可能にする。

**変更点**：
- `src/trace/`（`mod.rs`, `event.rs`, `ring.rs`, `frame.rs`, `writer.rs`）を新設。
- `TERMWAY_TRACE=1` / `TERMWAY_TRACE_FILE=<path>` / `TERMWAY_TRACE_BUFFER=<N>` の初期化と、`CompositorMetrics::print_summary` での `trace_file` 連携を実装。
- `TRC-001`（32B サイズ検証、ring 容量超過時の `overflow` カウンタと `drop-oldest` 順序保証、`.gz` 出力）の単体テストを追加。

**完了条件**：
- `TERMWAY_TRACE_FILE=/tmp/t.json` 実行で `/tmp/t.json`（および `.gz`）が有効な Chrome Trace JSON として出力される。
- トレース無効時、リングが未確保であり既存 `--profile` 出力が不変。

**コミット**：`feat(trace): add zero-allocation timeline tracer core and Chrome Trace JSON writer`

---

## M2 — `frame_id` による跨スレッド async flow と `FrameAssemble` / `BackendWrite` (`TIOCOUTQ`) 分離

**目的**：「メッセージのやり取り」とフレームドロップ原因・PTY 詰まりを可視化。`Render → EnqueueSubmit(s) → BackendEncode → FrameAssemble → BackendWrite → FrameComplete(f)` を 1 本の線で追跡可能にする。

**変更点**：
- `RenderJob` に `frame_id: u32` と `enqueue_ts_us: u64` を追加。
- メインスレッドの `render_pixels_with_damage` で `EnqueueSubmit`（`ph:"s"`）を発行し、`pending_job` 上書き破棄時には `BackendDrop`（`ph:"i"`, `dropped_fid`, `by_fid`, `pending_age_us`）を発行。
- ワーカースレッドで `BackendEncode`（`is_partial` 含む）、`frame_buf` 組み立ての `FrameAssemble`、および `ioctl(STDOUT_FILENO, TIOCOUTQ)` 付き `BackendWrite` + `FrameComplete`（`ph:"f"`）を発行。

**完了条件**：
- `TRC-002`：フレームドロップ時に `BackendDrop` に破棄された `frame_id` と待機時間 `pending_age_us` が記録され、`EnqueueSubmit` から `FrameComplete` までの flow イベントが対になって出力されることを単体テストで検証。

**コミット**：`feat(trace): add frame_id async flow, drop tracking, and PTY write TIOCOUTQ instrumentation`

---

## M3 — 入力経路の可視化と高頻度マウス motion coalescing

**目的**：入力受信からディスパッチまでの可視化と、`SGR-Pixels (1016h)` 高頻度マウス移動時のリング溢れ防止。

**変更点**：
- `src/compositor/mod.rs` / `src/compositor/input.rs` に `InputRawRecv`, `InputMouse`, `InputMotionCoalesce`, `InputMotionSkip`, `InputKeyBatch`, `WindowDrag` (`B`/`E`) を追加。
- 連続するポインタ移動を `ThreadRing::push` 内で直前の `InputMotionCoalesce` スロットにインプレース畳み込み（`dur_us` 延長 + `arg1` カウント加算）。
- `--trace-no-input` / `TERMWAY_TRACE_NO_INPUT=1` による入力トレース抑制に対応。

**完了条件**：
- `TRC-003`：連続するマウス移動イベントが 1 つの `InputMotionCoalesce` スライス（`count = N`）に畳み込まれ、リングスロットを消費しないことを単体テストで検証。

**コミット**：`feat(trace): add input path tracing with in-place motion coalescing`

---

## M4 — ウィンドウ管理 (WM) 状態遷移 counter track とプロトコルイベント

**目的**：`ARCHITECTURE.md` §2.3 のウィンドウ状態遷移（Minimized=0, Floating=1, Maximized=2）とライフサイクルイベントをタイムライン上で相関可能にする。

**変更点**：
- `window_manager.rs` / `window.rs` / `state.rs` / `handlers/` にて、`WindowStateChange`（`ph:"C"`, `win_{id}_state`）、`WindowMap`, `WindowConfigure`, `WindowRaise`, `WindowClose`, `ClientCommit`, `ResizeApplied` を記録。

**完了条件**：
- `TRC-005`：ウィンドウの生成・最大化・最小化・復元・クローズに伴って `win_{id}_state` カウンタトラックが `1 -> 2 -> 0 -> 1` と正しく遷移することを単体テストで検証。

**コミット**：`feat(trace): add window manager state counter tracks and lifecycle events`

---

## M5 — 診断 UX 仕上げ（CLI オプション・`SIGUSR1` / 定期スナップショット・panic ダンプ・HUD・ドキュメント）

**変更点**：
- `src/main.rs` に `--trace`, `--trace-file`, `--trace-buffer`, `--trace-flush-secs`, `--trace-no-input` CLI オプションと `--query` 同時指定時の警告を追加。
- `SIGUSR1` / `SIGUSR2` 検知時および `--trace-flush-secs` 経過時の連番スナップショット出力を `src/compositor/mod.rs` に統合。
- `src/terminal/raw.rs` の panic フックにて、端末安全復元完了後の `Once` トレースダンプを追加。
- `--hud` 有効時の HUD 表示にトレースリング `overflow` カウンタを統合。
- `README.md`, `README.ja.md`, `docs/ARCHITECTURE.md`, `docs/SPECIFICATION.md`, `docs/ROADMAP.md` を更新。

**完了条件**：
- `TRC-004`：連番スナップショット出力、CLI 引数パース、HUD 表示を含む全テストが通過すること。

**コミット**：`feat(trace): add CLI flags, snapshot rotation, panic dump, HUD overflow, and docs`

---

## 検証シナリオ ↔ テスト ID 対応表

`docs/SPECIFICATION.md` の可観測性要件に以下を登録する。

| ID | シナリオ | 検証内容 | ステップ |
| :--- | :--- | :--- | :--- |
| TRC-001 | 32B イベント & ring overflow 健全性 | `size_of::<TraceEvent>() == 32`、容量超過で `overflow` counter が立ち `drop-oldest` で直近イベント保持 | M1 |
| TRC-002 | `frame_id` flow & frame drop 原因分解 | `EnqueueSubmit(s)` → `FrameComplete(f)` の相関と、`BackendDrop` の `{dropped_frame, by_frame, pending_age_us}` | M2 |
| TRC-003 | 入力遅延経路 & motion coalescing | 連続マウス移動のインプレース共結成（1 スロットへの畳み込み）と `--trace-no-input` 抑制 | M3 |
| TRC-004 | 時点スナップショット & CLI 統合 | `SIGUSR1` / 定期フラッシュでの連番 dump、panic 時の端末復元後 dump、`.gz` 圧縮 | M5 |
| TRC-005 | WM 状態相関 | ウィンドウの Floating(1) / Maximized(2) / Minimized(0) 遷移の counter track 出力 | M4 |
