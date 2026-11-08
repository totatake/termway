# 03. CLI / 環境変数仕様

`--profile` と対称なオプション設計にします。README §5 / §7 と同じ文体で追記してください。

---

## 1. CLI オプション

| オプション | 引数 | 説明 | デフォルト |
| :--- | :--- | :--- | :--- |
| `--trace` | - | トレースイベントをスレッド毎の固定長リングバッファに蓄積し、終了時に Perfetto で開ける Chrome Trace Event JSON を出力（`TERMWAY_TRACE=1` でも有効化可） | 無効 |
| `--trace-file` | `<PATH>` | トレース JSON の出力先（指定時は暗黙に `--trace` も有効化）。`<PATH>` が `.gz` で終わる場合は gzip 圧縮する。未指定時は `/tmp/termway_trace.json` に出力 | `TERMWAY_TRACE_FILE` |
| `--trace-buffer` | `<N>` | スレッド毎のリング容量（スロット数）。`4k/64k/256k/1m` のように suffix 可 | `65536` |
| `--trace-flush-secs` | `<N>` | セッション中に N 秒毎にリングを連番ファイルへローテート dump する。`0` で無効 | `0` |
| `--trace-no-input` | - | 入力系イベント（マウス motion / キー入力）の記録を無効化。マウス高頻度で ring が溢れる場合の回避策 | 無効 |

### 1.1 `--profile` / `--hud` との統合

- `--profile --trace`：集計サマリー（stderr + `TERMWAY_PROFILE_FILE`）とタイムライン（Chrome Trace JSON）を**同時採取**。問題再現時の推奨。`TERMWAY_PROFILE_FILE` の出力 JSON にも `"trace_file"` フィールドが追記される。
- `--hud --trace`：HUD に `Ovf`（トレースリング overflow 数）を追加表示。
- `--query`：起動診断のみで終了するため、`--trace` と同時指定時は warning を出して無効化。

---

## 2. 環境変数

| 変数 | 対応オプション | 値 |
| :--- | :--- | :--- |
| `TERMWAY_TRACE=1` | `--trace` | `1/true` で有効 |
| `TERMWAY_TRACE_FILE=<path>` | `--trace-file` | パス（指定時は暗黙にトレース有効化）。`.gz` で gzip |
| `TERMWAY_TRACE_BUFFER=64k` | `--trace-buffer` | 数値 + 任意 suffix（`k`, `m`） |
| `TERMWAY_TRACE_FLUSH_SECS=10` | `--trace-flush-secs` | 秒数 |
| `TERMWAY_TRACE_NO_INPUT=1` | `--trace-no-input` | `1/true` で入力トレース無効化 |

### 2.1 `SIGUSR1` / `SIGUSR2` による時点スナップショット連動

```bash
# 長時間 SSH/tmux セッションで、今この時点のスクリーンショット＋トレーススナップショットを取りたい場合
kill -USR1 $(pgrep termway)
```

既存の `SIGUSR1`（スクリーンショット保存）または `SIGUSR2`（GUI 状態 JSON ダンプ）をメインループが検知した際、`--trace` が有効であれば併せて `/tmp/termway_trace.1.json`（または `<trace-file>.1.gz`）のように連番スナップショットを出力する。`--trace-flush-secs` もこれと同一経路を使用する。

---

## 3. README / `docs/ARCHITECTURE.md` 反映方針

### 3.1 README への追記

1. 「性能プロファイリングと JSON 出力」の直後に「タイムライン・トレーシングと Perfetto 可視化 (`--trace`)」を追加。
2. コマンドラインオプション表に `--trace-*` を追加。
3. 出力 JSON 例の末尾に `trace_file` フィールド追記。
4. 診断手順のコードブロックを追加：

```bash
# 問題再現時に集計 + タイムラインを同時採取
TERMWAY_PROFILE_FILE=/tmp/profile.json \
TERMWAY_TRACE_FILE=/tmp/termway.trace.gz \
  ./target/release/termway --profile --trace -- firefox
# → ui.perfetto.dev で /tmp/termway.trace.gz をドラッグ＆ドロップ
# → Search で frame_id を検索、Timeline で Render→Encode→Write を追跡
```

5. `docs/trace/` と `docs/adr/0003-zero-allocation-time-line-tracer.md` へのリンクを README 末尾「ドキュメント」節に追加。

### 3.2 `docs/ARCHITECTURE.md` への追記

- §1.1 ディレクトリツリーに `src/trace/` を追加。
- §2.2 「イベント駆動メインループと非同期レンダリングパイプライン」にトレース機構の連携（`frame_id` による `Render → EnqueueSubmit → BackendEncode → FrameAssemble → BackendWrite → FrameComplete` の async flow）を追記。
- §3 に「タイムライン・トレーシング（`src/trace/`）」を新設。

### 3.3 `docs/SPECIFICATION.md` への追記

- 要件定義およびテスト仕様に `--trace`（テスト ID: TRC-001〜TRC-005）を追加。

### 3.4 ADR

- `docs/adr/README.md` 一覧に [ADR-0003](../adr/0003-zero-allocation-time-line-tracer.md) を追加済み。
