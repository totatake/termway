# Termway タイムライン・トレーシング

Termway は「集計統計（`--profile`）」と「タイムライン（`--trace`）」の二層構造で可観測性を提供します。本ディレクトリは後者、Perfetto で開ける Chrome Trace Event JSON を出力するトレース機構に関する設計ドキュメントです。

| 文書 | 内容 |
| :--- | :--- |
| [00-background.md](00-background.md) | **開発背景** — 既存の集計統計で何が見えなかったか、どんな症状が診断不能だったか、なぜ Flamegraph / tracing / perfetto-rs で足りなかったか |
| [01-architecture.md](01-architecture.md) | **アーキテクチャ設計** — 32B 構造化データモデル、スレッド独立リング、`frame_id` flow event、PTY 書き込み計測と `TIOCOUTQ`、入力共結成、WM 状態レーンの全体像 |
| [02-detail-design.md](02-detail-design.md) | **詳細設計** — `src/trace/` の API 契約、既存モジュールへの組み込み点、Chrome Trace JSON 変換、panic/SIGUSR1 ハンドリング |
| [03-cli-and-env.md](03-cli-and-env.md) | **CLI / 環境変数仕様** — `--trace-*` オプションと `TERMWAY_TRACE_*` の意味、`--profile` との統合、README 反映方針 |
| [04-roadmap.md](04-roadmap.md) | **実装ロードマップ** — M1〜M5 の分割方針、各ステップの完了条件と検証シナリオ |
| [../adr/0003-zero-allocation-time-line-tracer.md](../adr/0003-zero-allocation-time-line-tracer.md) | **ADR-0003** — 上記の主要決定（Chrome JSON 自前・スレッド独立リング・32B 構造化イベント・flow event・PTY 書き込み分離）を ADR 形式で採択記録 |

## 前提知識

- 既存の可観測性は [docs/SPECIFICATION.md](../SPECIFICATION.md) と [docs/ARCHITECTURE.md §4](../ARCHITECTURE.md) で `CompositorMetrics` / `BackendMetrics` / `LatencyHistogram`（対数ヒストグラム）として解説されています。本トレーシングは**これを置き換えず補完**します。
- Perfetto UI は <https://ui.perfetto.dev/> でホストされており、Chrome Trace Event JSON（`[ {...}, {...} ]` の 1 event 1 line 形式、`.gz` 自動解凍対応）をそのままドラッグ＆ドロップで開けます。protobuf 形式 `.perfetto-trace` は本実装では出力しません。

## 「集計」と「タイムライン」の使い分け

| 観点 | 集計統計（`--profile`） | タイムライン（`--trace`） |
| :--- | :--- | :--- |
| 主な用途 | 日常運用、CI ベンチマーク、長期負荷監視、リリース判断 | **問題再現時の診断**、フレームドロップの原因解析、PTY 停滞の切り分け |
| 出力 | stderr サマリー + JSON | Chrome Trace Event JSON（`.gz` 可） |
| オーバーヘッド | 低（カウンタ増分のみ、デフォルト無効時ほぼゼロ） | ring 容量内の 32B slot 書き込みのみ（overflow 時は counter だけ立つ） |
| 時間軸 | **無し**（avg/p95/p99/max のみ） | **あり**（どの時間帯に p99 spike が出たかドリルダウン可） |
| 因果関係 | **無し**（frame drop は件数のみ） | `frame_id` で追跡（`Render→Enqueue→Encode→Write→Complete` を 1 本の線として追える） |

両者は排他的ではなく、`--profile --trace` で同時に有効化できます。`CompositorMetrics::print_summary` の JSON レポートに `trace_file` フィールドを追加して、両者を紐づけます。
