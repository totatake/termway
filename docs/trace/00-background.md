# 00. 開発背景（Background）

本ドキュメントは、なぜ Termway に新たなトレース機構が必要になったかを記録します。既存の `--profile` 集計統計が「どこが遅いか」は答えましたが、「なぜその時点で遅くなったか」は答えられませんでした。

---

## 1. 問題の所在：集計統計には「時間軸」と「因果関係」がない

`src/compositor/profile.rs` と `src/backend/terminal.rs` の `--profile` 出力は、セッション末尾に以下のような**集計値**を出します。

```text
--- [Compositor] ---
Frames: 872 (3.9 FPS) | Avg: 12.10 ms | p95: 34.50 ms | p99: 62.10 ms | Max: 118.40 ms
Damage:  Avg: 0.84 ms | Max: 4.20 ms
Skipped (no damage): 1,024

--- [Backend] ---
Sent: 872 | Encode: Avg: 7.11 ms | p95: 18.00 ms | p99: 32.00 ms | Max: 45.20 ms
Write: Avg: 66.19 ms | p95: 120.00 ms | p99: 280.00 ms | Max: 401.30 ms
Bytes: 251.1 MB | KB/s: 2119.3 | Dropped: 104 (10.6%)
```

この状態から、実運用で直面する以下の症状が**原理的に診断できません**。

| 症状 | 集計統計で分かること | 集計統計で見えないこと |
| :--- | :--- | :--- |
| 「p99 が 62ms と大きい」 | 数値だけ | spike がどの時間帯に集中しているか |
| 「フレームを 10% 落とした」 | 件数だけ | どのフレームがなぜ落ちたか（encode 詰まりか PTY write 詰まりか） |
| 「SSH/tmux 経由で操作がもどかしい」 | write 平均が 66ms | `frame_buf` 組み立て時間か、PTY 送信バッファ詰まり（`TIOCOUTQ` 滞留による `write(2)` 阻塞）か |
| 「xeyes 動作中に一瞬カクつく」 | Max 値だけ | Stationary Bounding Box 移動による全画面フォールバックがどのフレームで起きたか |
| 「マウスをドラッグ中にフレームが落ちる」 | 無し | 入力 raw 受信〜Wayland 送信完了の遅延内訳と描画フレームとの重なり |

`LatencyHistogram` は対数バケットのヒストグラムなので「分布の形」は取れますが、**サンプル時刻を保持しない**ため「p99 spike がスクロール直後に集中していた」のような相関を切り出せません。`BackendMetrics` の `frames_dropped` も個数のみで、**何が dropped の引き金かを因果として追えません**。

また、現在の `src/backend/terminal.rs` では `write_start` の計測区間に「`frame_buf` へのスライスコピー・メモリ組み立て処理」と「実際の `stdout.lock().write_all(&frame_buf) + flush()`」が混在しています。なお、Rust の `std::io::Stdout` は内部が 1024B の `LineWriter` であるため、1KB を超える `frame_buf` の `write_all()` は直接 OS の `write(2)` を呼び出し、SSH/tmux の PTY バッファ枯渇によるブロッキングは 100% `write_all()` 内で発生します（直後の `flush()` はカーネルドレイン待ちを行わないため常に `≈ 0 µs`）。そのため、バッファ組み立てと純粋な PTY 書き込みシステムコールを分離した上で、書き込み直前のカーネル PTY 送信キュー残量（`ioctl(STDOUT_FILENO, TIOCOUTQ)`）を記録する必要があります。

---

## 2. 既存の可観測性の「良い部分」

新機構を設計する前に、既存の思想で残すべき前提を整理します。

1. **デフォルト無効・有効化時のみコスト発生**（README §5）— トレースも同じ契約を守る。
2. **ゼロアロケーションのホット経路**（SIMD 量子化、固定長リング、アロケーション 0 の思想 — ADR-0002 §4.6）— トレースの記録処理は固定長 32B `Copy` スロットの書き込みのみとし、ホットパスでの `format!` やヒープ確保を一切行わない。
3. **依存最小**（`Cargo.toml` が極小、Xvfb 廃止、純 Rust JPEG — ADR-0001/0002）— `tracing` / `tracing-subscriber` / `perfetto-rs` / protobuf runtime を新規に持ち込まない（`.gz` 出力も既存の `flate2` を再利用）。
4. **panic 安全端末復元**（`RawModeGuard`、`?2026l`）— 診断情報の回収は復元完了後に済ませる。

これらの思想と衝突する可観測性は採用できません。

---

## 3. なぜ Flamegraph / tracing / perfetto-rs では足りなかったか

「タイムラインを可視化」という目的で検討した既存の手段と、このプロジェクトへの不適合理由を記録します（詳細な採択根拠は [ADR-0003](../adr/0003-zero-allocation-time-line-tracer.md)）。

### 3.1 Flamegraph / `samply` / `perf record`

CPU が **1 秒内でどこを食っているか**（関数別の自己時間）には最適ですが、目的である「**主要コンポーネントの動作とメッセージのやり取りが分かる**」は flamegraph の対象外です。

- `render()` 内で Pixman 呼び出しが多いことは分かるが、**`Render → EnqueueSubmit → Encode → Write` の跨スレッド同期**は捕捉できない。
- async（worker）境界の「どのフレームが待たされたか」を流せない。
- 集計の p99 spike に対して「どの時間帯のどのフレームか」を特定できない（flamegraph は時間軸を圧縮するため）。

### 3.2 `tracing` + `tracing-perfetto`

Span 設計と Perfetto 変換が既にあるが、**ホット経路に持ち込めない**。

- `SpanData` のヒープ確保、`tracing-subscriber` の registry + mutex、マクロ展開が 1 event 数百 ns クラス。毎フレーム数十〜数百イベントの経路で計測器が対象の負荷になる。
- `tracing-subscriber` 自体が依存クレートを増やし、依存最小の思想に反する。
- 「メッセージのやり取り」を表現する async flow が span ベースでは表現しづらい。

### 3.3 `perfetto-rs`（protobuf 出力）

- protobuf ランタイム + 依存が重く、`Cargo.toml` の極小性が損なわれる。
- 出力がバイナリなので、テストや CLI で直接中身を検証しづらく、既存の「`--profile` は stderr + JSON で自己完結」という方針と不一致。

### 3.4 eBPF / `perf` + スクリプト

- 計装ゼロだが、`frame_id` / WM 状態遷移 / Stationary Bounding Box 全画面フォールバックという **termway 固有の意味論**を載せられない。

---

## 4. なぜ Chrome Trace Event Format（JSON 自前）にしたか

- **文法が単純**：`ph:"X"|"B"|"E"|"i"|"s"|"f"|"C"|"M"` と `ts`(µs) `pid` `tid` `name` `dur` `args` 程度。1 event 1 line で出力できる。
- **Perfetto UI と Chrome DevTools の両方で開ける**。ユーザが追加ツールを必要としない。
- **JSON は追加クレート不要**：既存の `dump_state_json()` / `print_summary` 手書き JSON 手法と一貫し、`.gz` 圧縮も既存の `flate2` でそのまま出力できる。
- **ゼロアロと相性が良い**：記録側は 32B `Copy` 固定長 slot（数値引数 `arg1`/`arg2`/`arg3` 内蔵）をリングに書くだけ。JSON 文字列への変換は**オフライン**（flush 時）でまとめて行うため、ホット経路に文字列確保を一切持ち込まない。
- **集計との統合が自然**：既存 JSON レポートに `trace_file` フィールドを足すだけで紐づけ可能。

---

## 5. トレース機構で解きたい「診断の型」

新機構が成立しているかを判断する、具体的な診断シナリオを先に定義します（テストは [04-roadmap.md](04-roadmap.md) に紐づく）。

### シナリオ A：frame drop の原因分解
現状：集計では `Dropped: 104 (10.6%)` のみ。
期待：`BackendDrop` イベント（`ph:"i"`）の args に `{was_frame: 1298, by_frame: 1299, pending_age_us: 2340}` が乗り、Perfetto で「ワーカーが Encode と Write のどちらで長引いている最中に drop したか」が分かる。

### シナリオ B：PTY バックプレッシャーの切り分け
現状：集計では `Write: Avg 66.19 ms` のみ。メモリ上の `frame_buf` 構築なのか PTY `write(2)` 阻塞なのか区別がつかない。
期待：`FrameAssemble`（バッファ構築）と `BackendWrite`（PTY `write_all + flush`）が分離され、かつ `BackendWrite` の args に `{bytes: ..., tiocoutq: ...}`（書き込み直前のカーネル PTY 送信キュー滞留バイト数）が記録されることで、SSH/tmux 経路での PTY 詰まりが一目で確認できる。

### シナリオ C：入力遅延の経路分解
現状：集計なし。
期待：マウス raw 受信 → SGR パース → WM hit-test → Wayland `motion` 送信完了までの区間が `InputMouse` slice で追える。`--pixel-mouse` 時の coalesce 倍率（何イベントが畳み込まれたか）も分かる。

### シナリオ D：xeyes 残像フォールバックの可視化
現状：不可視。`Stationary Bounding Box` 検出（ADR-0002 §8.4）で全画面フォールバックがいつ起きたか分からない。
期待：ワーカー内の `BackendEncode` slice の args に `{bytes: ..., partial: 0, damage_rects: ...}` が乗り、部分更新（`partial: 1`）から全画面フォールバック（`partial: 0`）へ切り替わったフレームが特定できる。

### シナリオ E：WM 状態遷移の相関
現状：不可視。
期待：「最小化→復元直後に p99 が来ている」「ドラッグ中は全画面更新が連続している」が counter track (`win_{id}_state`) と `B`/`E` スパンで確認できる。

---

## 6. まとめ：新機構の目的

集計統計を廃止するのではなく、**集計が答えられない「いつ」「なぜ」に答える第二の計器**を、既存のゼロアロ / 依存最小思想のまま載せること。これが本設計の出発点です。
