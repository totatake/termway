# Termway 次世代レンダリング＆送信パイプライン開発指示書

本ディレクトリは、`docs/ROADMAP.md` §16「今後の展望 (Future Horizons)」に掲げられた以下の 3 大パイプライン最適化テーマに関する**開発指示書および詳細設計ドキュメント**です。

1. **PTY 送信のベクタード I/O (`writev` / `vmsplice`) によるゼロコピー化**
2. **純粋 Rust JPEG エンコーダの AVX2 / ARM NEON SIMD ベクトル化**
3. **エンコード・PTY 送信の完全非同期パイプライン化 (Double-Buffering Pipeline)**

v0.18.0 で導入されたゼロアロケーション・タイムライントレーシング（`--trace` / Perfetto）の実測解析により、高負荷動画再生時（Firefox YouTube 再生等）のボトルネックが定量的に特定されました。本指示書は、手戻りなく段階的かつ安全にこれら 3 テーマを実装・検証するための設計契約とマイルストーンを定義します。

| 文書 | 内容 |
| :--- | :--- |
| [00-background.md](00-background.md) | **開発背景と実測ボトルネック解析** — Perfetto トレース（Kitty-zlib vs Kitty-JPEG）から判明した直列実行の限界、メモリコピー負荷、および `vmsplice` と `writev` の技術的検証 |
| [01-architecture.md](01-architecture.md) | **アーキテクチャ設計** — 2 段ダブルバッファ・パイプライン（`AsyncEncode` + `AsyncWrite`）、状態を持つ差分エンコーダの順序保証、SIMD JPEG パイプライン、Vectored I/O 排他制御 |
| [02-detail-design.md](02-detail-design.md) | **詳細設計** — `src/backend/terminal.rs`, `src/protocol/jpeg.rs`, `src/trace/` 等の構造体・関数シグネチャ・SIMD 実装仕様・エッジケース契約 |
| [03-roadmap.md](03-roadmap.md) | **実装ロードマップ** — M1〜M4 の段階的コミット分割、完了条件、テスト仕様 ID（`PIP-001`〜`PIP-005`） |

## 設計の基本原則

1. **ゼロアロケーションの維持**:
   ホットパス（毎フレームの描画・エンコード・PTY 送信）において、ヒープ確保（`Vec::new` / `format!` による大容量再確保）を一切行わない。ダブルバッファリングも事前確保された 2 面のバッファのピンポン交換（`std::mem::swap` / `Option::take`）のみで完結させる。
2. **ビット互換性と安全フォールバック**:
   AVX2 (`x86_64`) および NEON (`aarch64`) による JPEG 高速化は、スカラー実装と JFIF 標準準拠の互換性を保ち、非対応 CPU や端数境界ブロックでは自動的に既存スカラー実装へフォールバックする。
3. **差分・状態付きエンコーダの整合性保証**:
   `SixelStreamEncoder`（6px バンドキャッシュ）や `KittyStreamEncoder`（部分プレースメント / `needs_clear`）はフレーム間の内部状態を持つため、**「エンコード前の生フレーム間引き（Drop）」は許容するが、「一度エンコードしたフレームの送信スキップ（Drop）」は行わない**（1 スロット・ランデブー方式）。
4. **可観測性（`--profile` / `--trace`）の完全統合**:
   パイプライン分離後も既存の `BackendMetrics` の後方互換性を保ちつつ、Perfetto タイムライン上で `Main → AsyncRender(Encode) → AsyncWrite(PTY Write)` の 3 レーン非同期フローを `frame_id` で一気通貫に追跡可能にする。
