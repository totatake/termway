# 00. 開発背景と実測ボトルネック解析（Background）

本ドキュメントは、v0.18.0 で導入した Perfetto タイムライントレーシング（`--trace`）を用いて Firefox での YouTube 動画再生時を実測した結果と、そこから導き出された 3 つの最適化課題の技術的背景を記録します。

---

## 1. Perfetto タイムライン実測から判明した事実

Firefox で YouTube 動画を再生した際の Perfetto トレースにより、以下の 2 つのモードにおけるパイプライン挙動が明確に可視化されました。

### 1.1 Kitty-zlib (`f=32, o=z`) 再生時の挙動：PTY I/O 帯域飽和
- **観測事実**:
  - `AsyncRender` スレッドの大半（1 フレームあたり 100ms〜150ms）が赤色の `BackendWrite`（PTY `write_all` ブロッキング）で埋め尽くされていた。
  - その間に `Main` スレッドは約 50ms 周期で新しいフレームをレンダリングして `EnqueueSubmit` を試みるが、ワーカーが `BackendWrite` から戻らないため、3 フレーム中 2 フレームが `BackendDrop` として破棄されていた。
- **原因**:
  - 自然動画（YouTube）は空間エントロピーが高く、RGB565 量子化 + zlib でも 1 フレームあたり数百 KB〜1MB 超のペイロードが発生し、PTY / SSH バッファのドレイン速度が完全にボトルネックとなっていた。

### 1.2 Kitty-JPEG (`f=101`) 再生時の挙動：CPU エンコードと PTY 送信の直列律速
- **観測事実**:
  - JPEG 圧縮によりペイロードサイズが劇的に縮小した結果、`BackendWrite` は約 15〜20ms まで短縮され、フレームドロップ（`BackendDrop`）は **0 件** となった。
  - 各フレームで「次のフレームが到着するまでの白い空き時間（アイドル余裕）」も確保できている。
  - **しかし、`AsyncRender` スレッドの 1 サイクル（約 30〜35ms）の内訳を見ると、茶色の `BackendEncode`（約 12〜15ms）と赤色の `BackendWrite`（約 15〜20ms）が同一スレッド上で完全に直列（シリアル）に実行されている**。

```text
現行の単一ワーカー (AsyncRender):
Frame N:   [ BackendEncode (~15ms) ][ FrameAssemble ][ BackendWrite (~18ms) ]
Frame N+1:                                                                   [ BackendEncode (~15ms) ][ BackendWrite (~18ms) ]
           |<------------------------ 合計 ~33ms --------------------------->|
```

この直列構造には以下の 3 つの構造的ロスが存在します。

---

## 2. 解決すべき 3 つの技術的課題

### 課題 1：`FrameAssemble` における毎フレームの大容量メモリコピーと `vmsplice` / `writev` の選択
現在の `src/backend/terminal.rs`（L314–L327）では、アトミックなフレーム出力を保証するために以下の処理を行っています：

```rust
frame_buf.clear();
frame_buf.extend_from_slice(b"\x1b[?2026h");
frame_buf.extend_from_slice(clear_seq.as_bytes());
if use_sixel {
    frame_buf.extend_from_slice(b"\x1b[H");
    frame_buf.extend_from_slice(&sixel_out);
} else {
    frame_buf.extend_from_slice(kitty_out.as_bytes());
}
frame_buf.extend_from_slice(taskbar_seq.as_bytes());
frame_buf.extend_from_slice(overlay_seq.as_bytes());
frame_buf.extend_from_slice(hud_seq.as_bytes());
frame_buf.extend_from_slice(b"\x1b[?2026l");
```

1. **問題点**:
   たかだか数十〜数百バイトの制御シーケンス（`\x1b[?2026h`, タスクバー, HUD, `\x1b[?2026l`）で前後を挟むためだけに、既に連続メモリ上に完成している `kitty_out` や `sixel_out`（数万〜百数十万バイト）を毎フレーム `frame_buf` へ丸ごと `memcpy` している。これは L2/L3 キャッシュを無意味に汚染し、`FrameAssemble` のレイテンシとメモリ帯域消費を増大させる。
2. **`vmsplice(2)` と `writev(2)` の技術的比較・検証**:
   - **`vmsplice(2)` の制約と危険性**:
     1. Linux の `vmsplice(2)` は **パイプ（pipe FD）専用**のシステムコールであり、出力先が疑似端末（PTY: `/dev/pts/N`）である場合はカーネルが `EINVAL` / `EBADF` を返して失敗する。`termway` は通常 PTY に直接接続して実行されるため、`vmsplice` は主経路で動作しない。
     2. 仮にパイプ経由（SSH 等）であっても、`vmsplice` はカーネルのパイプバッファがページ参照を保持している間にユーザ空間で該当バッファ（`kitty_out` / `sixel_out`）を次フレームで書き換えると、**送信中のデータが破壊される（Page-Cache Race）**。これを防ぐにはフレームごとに `mmap` ページを新規確保・破棄する必要があり、「ゼロアロケーション・バッファ再利用」の原則に逆行する。
   - **`writev(2)` (Vectored I/O / Scatter-Gather I/O) の優位性**:
     1. `libc::writev`（Rust の `std::io::IoSlice`）は、PTY（`/dev/pts/N`）・パイプ・ソケット・通常ファイルの**すべてで完全に動作する** POSIX 標準システムコールである。
     2. ユーザ空間での `frame_buf` への結合コピー（`FrameAssemble` における巨大 `memcpy`）を **100% 排除（ゼロコピー化）** し、最大 8 個の `iovec` ポインタ配列を 1 回のシステムコールでカーネルの `tty_write` に渡すことができる。
     3. ただし、Rust 標準ライブラリの `StdoutLock::write_vectored` は内部の `LineWriter`（1024B バッファと改行スキャン `memchr('\n')`）を経由するため、巨大ペイロードに対してオーバーヘッドが発生する。したがって、`stdout.lock()` でプロセス内排他ロックを獲得・フラッシュした上で、`STDOUT_FILENO` に対して直接 `libc::writev` ループ（部分書き込み・`EINTR` 対応）を発行する設計が最適となる。

---

### 課題 2：純粋 Rust JPEG エンコーダ（`src/protocol/jpeg.rs`）のスカラー演算ボトルネック
Kitty-JPEG モードにおいて、`BackendEncode` の大半（約 12〜15ms）は `src/protocol/jpeg.rs` のスカラー実装で消費されています。1920x1080 解像度では 1 フレームあたり $120 \times 68 = 8,160$ 個の MCU（16x16 マクロブロック）、すなわち **$48,960$ 個の 8x8 ブロック**を処理しており、以下の 4 箇所が CPU ボトルネックとなっています。

1. **RGBA バッファの二重走査とスカラー色空間変換**:
   - 現在、`extract_luma_8x8` を 4 回呼んで 16x16 領域の RGBA ピクセルを読み込んだ直後に、`extract_chroma_8x8` で**同じ 16x16 領域の RGBA ピクセルをもう一度メモリから読み直している**。
   - また、ITU-R BT.601 の固定小数点内積（`19595 * r + 38470 * g + 7471 * b` 等）を 1 ピクセルずつスカラーで計算している。
2. **2D 8x8 離散コサイン変換 (`fdct_2d`) のスカラー逐次実行**:
   - `fdct_1d` を行方向に 8 回、列方向に 8 回スカラーでループ実行している。
   - 8x8 ブロックの 1 行（`i32` × 8 要素 = 256 bit）は、**x86_64 AVX2 の `__m256i` レジスタ 1 本（または aarch64 NEON の `int32x4_t` 2 本）に完全に収まる**にもかかわらず、1 レーンずつ計算している。
3. **量子化・ジグザグ走査と高周波ゼロ係数の無駄なハフマン走査**:
   - GUI 画面や動画の平坦部では、量子化後の 64 個の係数のうち後半の大多数が `0` になるが、`encode_block` は毎回必ず `k = 1..64` まで 63 回ループしている。
   - 量子化時に「最後の非ゼロ係数のインデックス（`last_nonzero`）」を特定すれば、`1..=last_nonzero` のみのループで直ちに `EOB (0x00)` を出力でき、特に DC のみの平坦ブロック（`last_nonzero == 0`）では AC ループ自体を 0 回でスキップできる。
4. **`BitWriter::write_bits` の 1 バイト単位ループ出力**:
   - `write_bits` のたびに `while self.bits_in_buffer >= 8` で 1 バイトずつ `out.push(byte)` と `0xFF` チェックを行っており、分岐予測ミスと関数コール負荷が高い。64-bit ビットバッファに 32 bit 以上溜まった時点で 4 バイト一括出力（SWAR による `0xFF` バイト有無判定付き）を行うことで高速化できる。

---

### 課題 3：`BackendEncode` と `BackendWrite` の同一スレッド直列実行
課題 1・2 によってエンコードとメモリコピーを高速化しても、`BackendEncode`（CPU 計算）と `BackendWrite`（カーネル PTY I/O 待ち）が同じ `AsyncRender` スレッドで直列に動く限り、1 フレームのサイクルタイムは $T_{\text{encode}} + T_{\text{write}}$ の和になります。

```text
目標の 2 段ダブルバッファ・パイプライン:
AsyncEncode (CPU):  [ Encode N ]      [ Encode N+1 ]        [ Encode N+2 ]
AsyncWrite  (I/O):              [ Write N (writev) ]  [ Write N+1 (writev) ]
                    |<-- max(Encode, Write) -->|
```

- `BackendEncode` と `BackendWrite` を別スレッドに分離し、2 面のエンコード済みペイロードバッファ（Slot A / Slot B）をピンポン交換（Double-Buffering）することで、**フレーム $N$ を PTY へ `writev` 送信している裏で、並列にフレーム $N+1$ のエンコードを進める**ことができる。
- これにより実効サイクルタイムは $T_{\text{encode}} + T_{\text{write}}$ から $\max(T_{\text{encode}}, T_{\text{write}})$ へと短縮され、30fps（33.3ms 周期）および 60fps（16.6ms 周期）におけるスループットとドロップ耐性が飛躍的に向上する。
