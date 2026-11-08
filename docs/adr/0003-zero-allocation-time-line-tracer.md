# ADR-0003: ゼロアロケーション・タイムライン・トレーサ（Perfetto 対応）

* **ステータス**: 承認済 (Accepted)
* **日付**: 2026-11-02
* **対象コンポーネント**: `src/trace/`（新規: `mod.rs` / `event.rs` / `ring.rs` / `frame.rs` / `writer.rs`）、`src/compositor/profile.rs`（`CompositorMetrics`）、`src/backend/terminal.rs`（`BackendMetrics` / ワーカー）、`src/compositor/input.rs`、`src/compositor/window_manager.rs` / `window.rs`、`src/compositor/handlers/`
* **関連文書**: [docs/trace/](../trace/README.md)（背景・アーキ・詳細・CLI・ロードマップ）、[ADR-0001](0001-hybrid-dbus-session-routing.md)、[ADR-0002](0002-fixed-image-id-partial-placement.md)

---

## 1. コンテキストと課題 (Context)

`termway` の既存の可観測性は `--profile`（`CompositorMetrics` / `BackendMetrics` / `LatencyHistogram` の対数ヒストグラム）であり、パイプライン各段（Pixman 合成・damage 走査・プロトコルエンコード・PTY 書き込み）の avg / p95 / p99 / max と帯域・フレームドロップ件数を集計する。

しかしこの集計には構造上の 2 つの空白がある：

1. **時間軸がない**：p99 が大きいことは分かるが、その spike がどの時間帯（スクロール直後、ウィンドウ開いた直後、SSH 回線の瞬間劣化）に起きたか切り出せない。
2. **因果関係がない**：`frames_dropped` は個数のみで、「どのフレームがなぜ破棄されたか」「encode 詰まりか PTY write 詰まりか」を追えない。さらに現状の `terminal.rs` では `write_start` の計測区間に `frame_buf` のメモリ組み立て・コピー時間と実際の PTY `write_all + flush` システムコール時間が混在しており、かつ書き込み時のカーネル PTY 送信キュー残量（`TIOCOUTQ`）も観測できない。

主要な診断要望は「**タイムチャートで主要コンポーネントの動作とメッセージのやり取りが分かる情報を生成したい。リングバッファにトレースをため込み、Perfetto などで可視化可能にしたい**」である。一方でこのプロジェクトには、以下のような強い既存制約がある（[ADR-0001](0001-hybrid-dbus-session-routing.md) / [ADR-0002](0002-fixed-image-id-partial-placement.md)）：

* **ゼロアロケーションのホット経路**：合成〜damage〜enqueue は sub-ms budget、SIMD 量子化・固定長再利用・純 Rust JPEG でアロケートを排している。
* **依存最小**：`Cargo.toml` が極小で、外部バイナリ・重いクレートを避ける。
* **デフォルト無効**：`--profile` と同様に、有効化時のみコスト発生。
* **panic 安全端末復元**：診断回収は復元後に済ませる。

この制約下で「跨スレッドのメッセージフローを可視化するタイムライン計器」をどう実装するかが本決定の論点である。

---

## 2. 決定事項 (Decision)

### 2.1 出力形式：Chrome Trace Event Format JSON を自前で書く

Perfetto UI / Chrome DevTools がそのまま開ける **Chrome Trace Event Format JSON**（`[ {...}, {...} ]` の 1 event 1 line、`.gz` は既存の `flate2` クレートで圧縮対応）を出力する。変換ロジックは自前実装し、`serde` などの新規クレートを追加しない（既存の `dump_state_json()` / `print_summary` 手書き JSON と一貫）。

採用 `ph`（phase）：

| `ph` | `Phase` Enum | 意味 | 用途 |
| :--- | :--- | :--- | :--- |
| `X` | `Complete` | 同期 duration slice (`ts` + `dur`) | `Render` / `DamageScan` / `BackendEncode` / `BackendWrite` などの計測 |
| `B`/`E` | `Begin`/`End` | 継続スパン（同一スレッド） | ウィンドウのドラッグ移動・リサイズ状態 |
| `s`/`f` | `FlowStart`/`FlowEnd` | 非同期 flow | `EnqueueSubmit(s)` → `FrameComplete(f)`、`frame_id` で相関 |
| `C` | `Counter` | counter track | レイテンシ、WM 状態番号、ring overflow |
| `i` | `Instant` | instant marker | `WindowMap` / `BackendDrop` / `SendFrames` |
| `M` | `Metadata` | metadata | `process_name` / `thread_name` / `settings` |

### 2.2 記録機構：スレッド毎の独立固定長リングと 32B 構造化イベント

`termway` の実稼働スレッドは `Main`（イベントループ・合成・入力）と `AsyncRender`（`termway-async-render` エンコード・PTY 出力ワーカー）の **2 スレッド**である。スレッド間でのロック競合や Data Race UB（Torn Read）を排除するため、**スレッド毎に独立した固定長リング (`ThreadRing`)** を設ける。

* **32 バイト `#[repr(C)] Copy` スロット（パディング 0）**：
  * `ts_us: u64` (8B) / `dur_us: u32` (4B) / `id: u32` (4B) / `arg1: u32` (4B) / `arg2: u32` (4B) / `arg3: u16` (2B) / `lane: u8` (1B) / `phase: Phase` (1B) / `thread_id: ThreadRole` (2B) / `event_id: EventId` (2B) ＝ **計 32 バイトちょうど**。
  * ホット経路で `format!` や `&str` を一切使わず、`arg1` / `arg2` / `arg3` の 3 つの数値スロットに構造化パラメータ（バイト数・タイル数・`is_partial` フラグ・破棄フレーム ID・PTY `TIOCOUTQ` 残量等）を直接格納し、オフラインの `writer.rs` で `EventId` に応じた JSON キー名へ展開する。
* **安全な排他とゼロアロケーション**：
  * 各スレッド（`Main` / `AsyncRender`）が自身の `Mutex<ThreadRing>` に書き込む。平時は他スレッドが一切ロックしないため **100% uncontended lock（約 15ns）** で動作し、`unsafe` コードを 1 行も使わずに `SIGUSR1` 等での実行中 `drain` 時の Torn Read / Data Race UB を完全に防止する。
  * 容量は起動時 1 回確保（デフォルト 65,536 slot = 2MB/thread、`--trace-buffer` で可変）。**デフォルト無効時はリング自体を確保しない**。
* **overflow 時**：`drop-oldest`（古いスロットを上書きし `head` を進める）+ `overflow` カウンタをインクリメントし、JSON 出力時に **必ず Counter track として出力**（診断の信頼性を担保）。

### 2.3 `frame_id` による async flow event

`render_pixels_with_damage()` で `frame_id` を atomic 採番し、Chrome async flow（`ph:"s"`/`"f"`、`cat:"frame"`）で以下を 1 本の線として追跡可能にする：

```text
[Main]        X Render → X DamageScan → s EnqueueSubmit(id=F)
                                         ─── cross-thread ───
[AsyncRender] X BackendEncode(partial, rects) → X FrameAssemble → X BackendWrite(bytes, tiocoutq) → f FrameComplete(id=F)
```

`BackendDrop` は submit 時に `pending_job` が未消費だった場合に発生するため、`ph:"i"`（instant）で `arg1 = dropped_frame_id`, `arg2 = pending_age_us` を記録する。また `xeyes` 等での Stationary Bounding Box 移動による全画面フォールバック判定はワーカー内の `kitty_stream.encode_frame_into()` で行われるため、`BackendEncode` の `arg2`（`is_partial` フラグ）として記録する。

### 2.4 `BackendWrite` の正確な分離（バッファ構築 vs PTY システムコール + `TIOCOUTQ`）

Linux PTY および Rust の `std::io::Stdout`（内部容量 1024B の `LineWriter`）では、数 KB〜数百 KB の `frame_buf` に対する `write_all(&frame_buf)` は 1KB バッファをバイパスして直接 `write(2)` システムコールを発行し、SSH/tmux の PTY 送信バッファ枯渇によるブロッキングは **100% `write_all()` 内で発生する**（直後の `flush()` はカーネルドレイン待ちを行わないため常に `≈ 0 µs` となる）。

一方で現状の `src/backend/terminal.rs` では、`write_start = Instant::now()` の計測区間に **`frame_buf` のメモリ組み立て・スライスコピー（303〜315行目）** と **実際の `stdout.lock().write_all(&frame_buf) + flush()`** が混在している。
そこで本設計では：
1. `frame_buf` の組み立てを計測外（または `FrameAssemble` スライス）として分離し、純粋な PTY 書き込みシステムコール時間を **`BackendWrite`** として計測する。
2. 書き込み直前に `libc::ioctl(STDOUT_FILENO, libc::TIOCOUTQ, &mut outq)` でカーネル PTY 送信キューの未送信残量バイト数を取得し、`BackendWrite` の `arg2` に記録する。これにより、Perfetto 上で「エンコード負荷か、PTY 送信バッファ詰まり（SSH/tmux バックプレッシャー）か」を確実に判別可能にする。

### 2.5 入力系の coalescing と抑制

`Any-event 1003h` + `SGR-Pixels 1016h` はマウス 1px 毎にイベントが来る。無条件 record は ring を瞬時に溢れさせ計測器が対象負荷になる。`motion` は直前 motion と**共結成**し `InputMotionCoalesce`（`arg1=count`）として 1 slice 化、背景 motion は `InputMotionSkip`（counter のみ）。`drag`/`resize` は `phase=B`/`E` で記録。`--trace-no-input` で入力系を丸ごと抑制可能にする。

### 2.6 集計統計との共存

既存 `CompositorMetrics` / `BackendMetrics` / `LatencyHistogram` は**消さず残す**。集計は日常運用・CI・長期監視、タイムラインは問題再現時の診断という 2 層構造。`print_summary` の JSON レポートに `trace_file` フィールドを追加し両者を紐づける（既存 JSON キーは維持し後方互換）。

### 2.7 flush トリガーと既存シグナルとの共存

1. **セッション終了**（`print_summary` と同タイミング）。
2. **`SIGUSR1` / `SIGUSR2` 連動**：既存の `src/compositor/env.rs` では `SIGUSR1` がスクリーンショット保存 (`CAPTURE_REQUESTED`)、`SIGUSR2` が状態 JSON ダンプ (`STATE_DUMP_REQUESTED`) に割り当てられている。シグナルハンドラ内は Async-Signal-Safe（`AtomicBool` セットのみ）を厳守し、メインループ側（`mod.rs`）で `CAPTURE_REQUESTED` または `STATE_DUMP_REQUESTED` を処理する際、`trace::enabled()` であれば併せてトレースの時点スナップショット（連番ローテート）を出力する。
3. **`--trace-flush-secs=N`**：メインループ内で N 秒経過毎に連番ローテート出力。
4. **panic フック**：`Once` で 1 回、**既存の panic 安全端末復元完了後**に出力。`--query` とは排他（warning）。`.gz` 指定時は既存の `flate2` で gzip 圧縮。

---

## 3. 検討した代替案 (Alternatives Considered)

### 代替案 A: `tracing` + `tracing-perfetto`
* **内容**：`tracing` の span 設計と `tracing-subscriber` / `tracing-perfetto` で Perfetto 出力。実装量少。
* **却下理由**：`SpanData` のヒープ確保、`tracing-subscriber` の registry + mutex、マクロ展開が 1 event 数百 ns。毎フレーム数十〜数百イベントの経路で計測器が対象負荷になる（sub-ms budget に衝突）。`tracing-subscriber` 自体が依存を増やし依存最小の思想に反する。async flow（跨スレッドメッセージ）を span で表現しづらい。

### 代替案 B: `perfetto-rs`（protobuf 出力）
* **内容**：`.perfetto-trace` バイナリをネイティブ生成。圧縮率が高い。
* **却下理由**：protobuf ランタイム + codegen で依存が重く `Cargo.toml` の極小性が損なわれる。出力がバイナリで CLI 自己完結の思想と不一致。ゼロアロ設計と protobuf 中間バッファが相性が悪い。

### 代替案 C: `flamegraph` / `samply` / `perf record`
* **内容**：計装不要で関数別 CPU 自己時間を可視化。
* **却下理由**：「主要コンポジターの動作とメッセージのやり取り」を答えない。`Render → Enqueue → Encode → Write` の跨スレッド同期と async 境界を捕捉できない。`frame_id` / WM 状態遷移 / PTY backpressure という termway 固有の意味論を載せられない。

### 代替案 D: eBPF / `perf` + スクリプト
* **内容**：計装ゼロで syscall 含む可視化。
* **却下理由**：`frame_id` / WM 状態遷移 / Stationary Bounding Box フォールバックを載せられない。プロセス内部構造を知らない外部計器では「どのフレームが破棄されたか」を復元できない。

### 代替案 E: 単一 global ring + マルチ生産者キュー
* **内容**：全スレッドが 1 つの ring に書く。
* **却下理由**：単一 ring を全スレッドで共有するとキャッシュライン競合が発生する。Termway は `Main` と `AsyncRender` の 2 スレッド構成であるため、スレッド毎に独立した `ThreadRing` を持たせれば平時はスレッド間の競合が完全にゼロとなり、`frame_id` と単調時刻 `ts_us` でオフライン出力時にマージできる。

---

## 4. トレードオフと背反 (Consequences & Trade-offs)

### 肯定的な結果 (Positive)
1. **集計では見えない「いつ・なぜ」に答える**：時間軸 + `frame_id` flow により、frame drop 原因分解・PTY 停滞切り分け（`TIOCOUTQ` 併用）・入力遅延経路分解・WM 状態相関が可能。
2. **既存思想を一切壊さない**：32B `Copy` 構造体による完全ゼロアロケーション・`unsafe` ゼロ・デフォルト無効・新規クレート追加 0 で `--profile` は回帰不変。
3. **外部ツール不要の診断**：Perfetto UI に `.json` / `.gz` をドラッグするだけで跨スレッドタイムラインを解析可能。集計 JSON の `trace_file` と紐づく。
4. **診断 UX**：HUD に ring overflow を表示、`SIGUSR1` 時点 dump、panic 時 dump で再現環境を回収。

### 否定的な結果・背反 (Negative / Trade-offs)
1. **有効化時は ring 分のメモリ**：スレッド毎に約 2MB（2 スレッドで計 4MB）。overflow 時の drop-oldest で「直近 N 秒」を保持する。
2. **手書き JSON の文法管理**：`ph` の意味・`s`/`f` の `bp:"e"` など Chrome Trace の仕様に沿う必要がある（単体テストおよび Perfetto UI での検証をゲートとする）。
3. **`Instant` の取得コスト**：`now_us()` 呼出がイベントあたり最小回数になるよう、既存の計測用 `Instant::now()` と共有または呼び出し側で受け渡す設計とする。
4. **WM 状態 track の動的生成**：ウィンドウ毎に `win_{id}_state` カウンタを出すため、ウィンドウ数に応じて track が増える（一般的な利用枚数では負荷は無視できる）。
