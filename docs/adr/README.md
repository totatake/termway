# Architecture Decision Records (ADR)

本ディレクトリは、`termway` における重要なアーキテクチャ上の設計決定（Architecture Decision Records: ADR）を記録・管理する場所です。

各決定の背景（Context）、採択された方針（Decision）、検討された代替案（Alternatives）、およびトレードオフ・背反（Consequences）を明確に記録することで、設計の意図と将来の拡張方針を透明化します。

---

## 一覧

| ADR 番号 | タイトル | ステータス | 日付 |
| :--- | :--- | :--- | :--- |
| [ADR-0001](0001-hybrid-dbus-session-routing.md) | ハイブリッド D-Bus セッションルーティングと Snap アプリケーション対応 | Accepted | 2026-10-09 |
| [ADR-0002](0002-fixed-image-id-partial-placement.md) | 固定画像IDによる部分画面転送アーキテクチャと端末エミュレータ保護 | Accepted | 2026-10-18 |
| [ADR-0003](0003-zero-allocation-time-line-tracer.md) | ゼロアロケーション・タイムライン・トレーサ（Perfetto 対応） | Accepted | 2026-11-02 |
| [ADR-0004](0004-wayland-pointer-frame-separation-and-horizontal-scroll.md) | Wayland ポインタフレーム分離と水平スクロール対応による GTK/Tauri 互換性確保 | Accepted | 2026-11-09 |
| [ADR-0005](0005-double-buffering-simd-jpeg-and-vectored-io.md) | 完全非同期パイプライン・SIMD JPEG・ベクタード I/O による描画スループット最適化 | Accepted | 2026-11-09 |
