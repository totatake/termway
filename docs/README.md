# termway ドキュメント一覧

本ディレクトリは **termway**（Smithay ベースの端末内 Wayland/X11 GUI ブリッジ）の設計・仕様・検証・利用ガイドを集約する技術ドキュメントハブです。

## 基本ドキュメント

| ドキュメント | 内容 |
| :--- | :--- |
| [overview.md](overview.md) | 開発背景・なぜ端末内 GUI か・従来手法（`ssh -X` / VNC / RDP）との比較・ユースケース |
| [requirements.md](requirements.md) | 機能要件（`FR-1`〜`FR-44`）および非機能要件（`NFR-1`〜`NFR-6`） |
| [architecture.md](architecture.md) | システムアーキテクチャ詳細設計書（静的/動的構造・WM 状態遷移・ゼロアロケーショントレーサ） |
| [testing.md](testing.md) | テスト方針・要件トレーサビリティマトリクス・全 17 テストスイート仕様一覧 |
| [roadmap.md](roadmap.md) | 達成済みマイルストーン（`v0.1.0`〜`v0.24.0`）と今後の展望 |

## サブディレクトリ

| ディレクトリ | 用途 |
| :--- | :--- |
| [guides/usage.md](guides/usage.md) | 利用ガイド（端末互換性・CLI オプション・`TERMWAY_*` 環境変数・ショートカット・Perfetto 手順・WSL2 FAQ） |
| [adr/README.md](adr/README.md) | Architecture Decision Records（`ADR-0001`〜`ADR-0005`） |
| [design/README.md](design/README.md) | 大型サブシステム設計パック（[pipeline/](design/pipeline/README.md) / [trace/](design/trace/README.md)） |
| [reports/README.md](reports/README.md) | テスト実行ログ・ベンチマーク評価レポート（[test-results.md](reports/test-results.md) 等） |
| `assets/` | README 等で参照するデモ GIF（`assets/demo.gif`）等のメディア資産 |
