# サブシステム・大型機能設計パック (`termway`)

大型機能・サブシステムごとの設計検討パック（背景 `00-` 〜 設計 `01-`/`02-` 〜 ロードマップ・検証結果 `04-`）を集約しています。システム全体構成は [../architecture.md](../architecture.md) を参照してください。

| 設計パック | 内容 | 関連 ADR |
| :--- | :--- | :--- |
| [pipeline/README.md](pipeline/README.md) | **次世代レンダリング＆送信パイプライン** — ベクタード I/O (`writev`)・SIMD JPEG (AVX2/NEON)・Double-Buffering 非同期パイプライン | [ADR-0005](../adr/0005-double-buffering-simd-jpeg-and-vectored-io.md) |
| [trace/README.md](trace/README.md) | **ゼロアロケーション・タイムライン・トレーシング** — 32B 固定長リング、`frame_id` flow、`TIOCOUTQ`、Perfetto Chrome JSON | [ADR-0003](../adr/0003-zero-allocation-time-line-tracer.md) |
