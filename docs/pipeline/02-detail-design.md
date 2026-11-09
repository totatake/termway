# 02. 詳細設計（Detail Design）

本ドキュメントは、[01-architecture.md](01-architecture.md) の各構成要素について、変更対象ファイル・構造体・関数シグネチャ・アルゴリズム・エッジケース契約を定義します。実装者は本書と [03-roadmap.md](03-roadmap.md) の完了条件を満たす限り、内部実装の細部を自由に最適化して構いません。

---

## 1. Vectored I/O（M1）

### 1.1 新設モジュール `src/backend/vectored.rs`

```rust
use std::io::{self, IoSlice};
use std::os::unix::io::RawFd;

/// `fd` に対して全 `bufs` を `writev(2)` で書き切る。
/// 部分書き込み・EINTR・EAGAIN（O_NONBLOCK 共有時）を吸収する。
/// 戻り値は書き込んだ総バイト数と writev 呼び出し回数。
pub fn writev_all(fd: RawFd, bufs: &mut [IoSlice<'_>]) -> io::Result<WritevStats>;

#[derive(Debug, Default, Clone, Copy)]
pub struct WritevStats {
    pub bytes: usize,
    pub syscalls: u32,
    pub eagain_waits: u32,
}
```

#### アルゴリズム

```text
let mut slices = bufs (空スライスは事前に除外)
while !slices.is_empty():
    n = libc::writev(fd, slices.as_ptr() as *const libc::iovec, min(slices.len(), IOV_MAX))
    if n > 0:  stats.bytes += n; IoSlice::advance_slices(&mut slices, n)   // Rust 1.81+ 安定 API
    elif n == 0: return Err(WriteZero)
    else match errno:
        EINTR  => continue
        EAGAIN => poll(fd, POLLOUT, timeout = 1000ms); eagain_waits += 1; continue
        _      => return Err(last_os_error)
```

- `std::io::IoSlice` は Unix で `struct iovec` と ABI 互換であることが標準ライブラリで保証されているため、ポインタキャストで `libc::writev` に渡せる（追加クレート不要）。
- `IOV_MAX`（Linux: 1024）は本用途（最大 3 本）では到達しないが、汎用関数として `min` を取る。

> [!NOTE]
> **EAGAIN 対策の背景**: `src/terminal/raw.rs` の端末復元処理（L152–L157）は stdin に一時的に `O_NONBLOCK` を設定します。stdin と stdout が同一 TTY の**同一 open file description**を共有している場合（通常のシェル起動時）、このフラグは stdout にも波及します。現行は Write スレッドの `join` 後に復元処理が走るため実害はありませんが、`writev_all` は防御的に `EAGAIN` を `poll(POLLOUT)` で待機して再試行します。

### 1.2 `EncodedFrame`（M1 で導入、M3 でスロット化）

```rust
/// エンコード済み 1 フレーム分の送出データ。Write 側からは不変として扱う。
pub(crate) struct EncodedFrame {
    pub frame_id: u32,
    pub prefix: Vec<u8>,   // "\x1b[?2026h" + clear_seq (+ Sixel 時 "\x1b[H")
    pub payload: Vec<u8>,  // エンコーダ出力 (Kitty APC 列 / Sixel DCS)
    pub suffix: Vec<u8>,   // taskbar + overlay + hud + "\x1b[?2026l"
    pub damage_rects: u32,
    pub is_partial: bool,
    pub show_hud: bool,
}

impl EncodedFrame {
    pub fn io_slices(&self) -> [IoSlice<'_>; 3];
    pub fn total_len(&self) -> usize;
    pub fn clear(&mut self); // 3 つの Vec を clear()（容量維持）
}
```

#### `kitty_out: String` の扱い

`KittyStreamEncoder::encode_frame_into` は `&mut String` に出力します。M1 では **`String` を `Vec<u8>` へ変換するコピーを発生させない**ため、以下のいずれかを採用します。

| 案 | 内容 | 推奨 |
| :--- | :--- | :---: |
| A | `payload` を `String` 型にし、`io_slices` で `as_bytes()` を返す | — |
| B | `std::mem::take(&mut kitty_out).into_bytes()` で所有権ごと移し、送信後に `String::from_utf8_unchecked` で戻す | — |
| **C** | `KittyStreamEncoder` の出力型を `Vec<u8>` に変更（Base64 / APC は ASCII のみのため型安全上の損失なし） | ✅ |

案 C は `src/protocol/kitty.rs` と `tests/unit_kitty_encoder.rs` の型変更を伴いますが、Sixel と出力型が揃い、M3 のスロット交換（`std::mem::swap(&mut enc_out, &mut slot.payload)`）が単純になります。

### 1.3 `TerminalBackend` ワーカーの変更（M1 時点：スレッド分離前）

```rust
// 旧: frame_buf.extend_from_slice(...) × 8 → write_all(&frame_buf)
// 新:
frame.prefix.clear();
frame.prefix.extend_from_slice(b"\x1b[?2026h");
frame.prefix.extend_from_slice(clear_seq.as_bytes());
if use_sixel { frame.prefix.extend_from_slice(b"\x1b[H"); }
std::mem::swap(&mut frame.payload, &mut encoder_out);   // コピーなし
frame.suffix.clear();
frame.suffix.extend_from_slice(taskbar_seq.as_bytes());
frame.suffix.extend_from_slice(overlay_seq.as_bytes());
frame.suffix.extend_from_slice(hud_seq.as_bytes());
frame.suffix.extend_from_slice(b"\x1b[?2026l");

{
    let mut guard = stdout.lock();   // INV-3: 他の stdout 書き込みと排他
    let _ = guard.flush();           // LineWriter 内の未送出 OSC を先に吐き出す
    let mut slices = frame.io_slices();
    let stats = writev_all(libc::STDOUT_FILENO, &mut slices);
}   // ロック解放

std::mem::swap(&mut frame.payload, &mut encoder_out);   // バッファを返却
```

- `taskbar_seq` / `overlay_seq` / `hud_seq` / `clear_seq` の `format!` による一時 `String` 生成は現行と同等であり、M1 のスコープ外とする（いずれも数百バイト未満）。ただし `write!(&mut frame.suffix, ...)` で直接書き込む形へ置き換えることは推奨（ゼロアロケーション化のついで改善）。
- `frame_buf`（256KB 事前確保）は削除する。

### 1.4 エラー時の挙動

現行の `let _ = out_guard.write_all(...)` と同じく、書き込みエラーはフレーム単位で無視して継続する（端末切断時は Main スレッドの stdin EOF / SIGHUP 経路で終了する）。ただし `--trace` 有効時は `BackendWrite` の `arg3` 上位ビットにエラーフラグを立てる（§4）。

---

## 2. SIMD JPEG エンコーダ（M2）

### 2.1 ファイル構成

```text
src/protocol/
├── jpeg.rs            # 既存: 公開 API・マーカー出力・スカラー実装・ディスパッチ
├── jpeg_simd/
│   ├── mod.rs         # Backend enum と選択ロジック
│   ├── avx2.rs        # #[cfg(target_arch = "x86_64")]
│   └── neon.rs        # #[cfg(target_arch = "aarch64")]
```

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JpegBackend { Scalar, Avx2, Neon }

impl JpegBackend {
    pub fn detect() -> Self;            // JpegEncoder::new で 1 回だけ呼ぶ
    pub fn from_env_or_detect() -> Self; // TERMWAY_JPEG_SIMD=0 でスカラー強制（検証・切り分け用）
}
```

`JpegEncoder` に `backend: JpegBackend` フィールドを追加し、`encode_subimage_into` の MCU ループ内で `match self.backend` により分岐する（`match` は分岐予測で実質ゼロコスト。関数ポインタ化は任意）。

### 2.2 段①：MCU 単位の融合色変換

```rust
/// 16x16 MCU の RGBA を 1 回だけ読み、Y 4 ブロックと Cb/Cr 各 1 ブロックを同時に生成する。
/// in-bounds（start_x + 16 <= max_x && start_y + 16 <= max_y）のときのみ呼ばれる。
unsafe fn extract_mcu_avx2(
    rgba: &[u8], stride_bytes: usize, base_offset: usize,
    y_out: &mut [[i32; 64]; 4], cb_out: &mut [i32; 64], cr_out: &mut [i32; 64],
);
```

**AVX2 実装指針**:

| 処理 | 命令 |
| :--- | :--- |
| 8px (32B) ロード | `_mm256_loadu_si256` |
| R/G/B 抽出 (i32 × 8) | `_mm256_and_si256(v, 0xFF)`, `_mm256_srli_epi32(v, 8)`, `_mm256_srli_epi32(v, 16)` |
| Y = (19595R + 38470G + 7471B + 32768) >> 16 − 128 | `_mm256_mullo_epi32` × 3, `_mm256_add_epi32`, `_mm256_srai_epi32` |
| 2x2 平均用の行和 | 上下 2 行の R/G/B を `_mm256_add_epi32` |
| 水平ペア和 | `_mm256_hadd_epi32` → `_mm256_permute4x64_epi64(_, 0xD8)` で 128bit レーン順を補正 |
| (sum + 2) >> 2 → Cb/Cr | `_mm256_srai_epi32` + 上記と同形の積和 |

**NEON 実装指針**:

| 処理 | 命令 |
| :--- | :--- |
| 8px デインターリーブロード | `vld4_u8` → `r, g, b, a: uint8x8_t`（RGBA 分離が 1 命令） |
| u8 → i32 拡張 | `vmovl_u8` → `vmovl_u16` (lo/hi) → `vreinterpretq_s32_u32` |
| 積和 | `vmulq_n_s32`, `vmlaq_n_s32` |
| 水平ペア和 | `vpaddq_s32` |

> [!IMPORTANT]
> `Y` の係数積 `38470 * 255 = 9,809,850` と和は `i32` に収まり、スカラーと同一の `i32` 演算・同一の算術右シフト（`srai`）を用いるため **INV-4（ビット完全一致）** を満たします。`_mm256_mulhi_epi16` / `_mm256_mulhrs_epi16` 等の 16bit 近似命令は丸めが異なるため使用禁止とします。

### 2.3 段②：2D FDCT（転置方式）

現行 `fdct_2d` は「行方向 1D × 8 → 列方向 1D × 8」です。SIMD では **レジスタ = 要素インデックス、レーン = 行（または列）** と配置し、1D バタフライを 8 本同時に実行します。

```text
block (row-major, 8 x __m256i: r0..r7)
  │ transpose_8x8_epi32          // レジスタ k = 全行の第 k 要素
  ▼
fdct_1d_lanes(v0..v7)            // 8 行分の行方向 1D DCT を同時計算
  │ transpose_8x8_epi32          // レジスタ k = 全列の第 k 行目
  ▼
fdct_1d_lanes(v0..v7)            // 8 列分の列方向 1D DCT を同時計算
  ▼
store (row-major, 係数配置はスカラーと同一)
```

- `fdct_1d_lanes` はスカラー `fdct_1d` の各行を `_mm256_add_epi32` / `_mm256_sub_epi32` / `_mm256_mullo_epi32` / `_mm256_slli_epi32` / `_mm256_srai_epi32` へ **1 対 1 で機械的に置換**する。演算順序・丸め定数（`1 << 10`）・シフト量（`11`, `<< 2`）を一切変えないこと。
- `transpose_8x8_epi32` は `unpacklo/hi_epi32` → `unpacklo/hi_epi64` → `permute2x128_si256` の標準 3 段（24 命令）。
- NEON は 8x8 を 4x4 × 4 象限に分け、`vtrnq_s32` + `vcombine` で転置し、各 1D 変換を `int32x4_t` × 2 で実行する。

> [!NOTE]
> **オーバーフロー解析**: 列方向パスの入力は行方向パス出力（最大 |4096| 程度）で、`tmp` 和は最大 ±32768、最大係数 `FIX_3_072711026 = 25172` との積は約 8.2×10⁸ と `i32::MAX`（2.1×10⁹）未満です。スカラー実装と同一の `i32` 演算であるため、SIMD 化によって新たなオーバーフローは生じません（`_mm256_mullo_epi32` は下位 32bit を返し、スカラーと同じ値域で一致）。

### 2.4 段③：量子化・ジグザグ・`last_nonzero`

```rust
/// raster 順 DCT 係数を量子化し、zigzag 順で out に格納。
/// 戻り値: 最後の非ゼロ係数の zigzag インデックス（全ゼロなら 0）。
fn quantize_zigzag_lnz(dct: &[i32; 64], tables: &RasterQuantTables, out: &mut [i16; 64]) -> u8;
```

- `QuantizationTables` に **raster 順**の `y_mult_r` / `y_half_r` / `c_mult_r` / `c_half_r` を追加で事前計算する（`mult_r[ZIGZAG[k]] = mult[k]`）。DQT 出力用の zigzag 順テーブルは現状のまま維持。
- 量子化式 `((|v| + half) as u64 * mult) >> 33` は 64bit 積が必要なため、AVX2 では `_mm256_mul_epu32` を偶数レーン / 奇数レーン（`srli_epi64(_, 32)` 後）の 2 回適用し、`srli_epi64(_, 33)` 後にブレンドして 8 レーンに戻す。符号は `_mm256_abs_epi32` / `_mm256_sign_epi32` で復元。NEON では `vmull_u32` (lo/hi) を用いる。
- `i16` クランプとジグザグ並べ替えは、raster 順の量子化結果（`i32` × 64）をスタックに書き出した後、**スカラーの 64 回ギャザー**で行い、同時に `last_nonzero` を求める（SIMD ギャザーより高速かつ単純）。
- スカラー経路も同関数シグネチャに統一し、`last_nonzero` を返すよう改修する。

### 2.5 段④：ハフマン符号化の EOB 早期打ち切り

```rust
fn encode_block(block: &[i16; 64], last_nonzero: u8, last_dc: &mut i32,
                dc_lut: &HuffmanLut, ac_lut: &HuffmanLut, writer: &mut BitWriter);
```

```text
DC 符号化（現行どおり）
for k in 1..=last_nonzero:   // 現行は常に 1..64
    （ゼロラン / ZRL / AC 符号化は現行どおり）
if last_nonzero < 63: EOB を出力
```

現行実装は「末尾にゼロランが残れば EOB」であり、`last_nonzero < 63` と同値です。したがって**出力ビット列は完全に一致**し、平坦ブロック（`last_nonzero == 0`）では AC ループが 0 回になります。

### 2.6 段⑤：`BitWriter` の 32bit 一括フラッシュ

```text
write_bits(code, len):
    buffer = (buffer << len) | (code & mask(len)); bits += len
    if bits >= 32:
        bits -= 32
        w = (buffer >> bits) as u32               // 上位 4 バイト
        if !has_ff_byte(w):  out.extend_from_slice(&w.to_be_bytes())
        else:                4 バイトを順に push し、0xFF の直後に 0x00 を挿入
```

```rust
#[inline(always)]
fn has_ff_byte(w: u32) -> bool {
    let x = !w; // 0xFF バイト → 0x00
    (x.wrapping_sub(0x0101_0101) & !x & 0x8080_8080) != 0
}
```

- `bits` の最大値は `31 + 16 = 47` で `u64` に収まる。
- `flush_pad` は残り 0〜31 bit をバイト単位で吐き出す既存ロジックに、1 バイト単位ループを追加して対応。

### 2.7 段別の期待効果（1920x1080、目安）

| 段 | 現行比 | 根拠 |
| :--- | :--- | :--- |
| ① 色変換 | 1/4〜1/6 | 8 レーン並列 + RGBA 二重読み解消 |
| ② FDCT | 1/3〜1/5 | 8 レーン並列（転置オーバーヘッド込み） |
| ③ 量子化 | 1/2〜1/3 | 64bit 積が 2 回必要なため伸びは控えめ |
| ④⑤ | 1/1.5〜1/3 | 平坦部の EOB 打ち切り効果が支配的（コンテンツ依存） |
| **合計** | **約 15ms → 5〜8ms** | ROADMAP §16 の目標値 |

---

## 3. Double-Buffering Pipeline（M3）

### 3.1 共有状態

```rust
struct WriteHandoff {
    ready: Option<Box<EncodedFrame>>,   // Encode → Write（容量 1, Drop 禁止）
    free:  Vec<Box<EncodedFrame>>,      // Write → Encode（起動時に 2 面投入）
    shutdown: bool,
}

pub struct TerminalBackend {
    // 既存フィールド ...
    async_state: Arc<(Mutex<AsyncRenderState>, Condvar)>,   // ① 既存
    write_state: Arc<(Mutex<WriteHandoff>, Condvar)>,        // ②③ 新設
    encode_handle: Option<JoinHandle<()>>,
    write_handle: Option<JoinHandle<()>>,
}
```

- `Box<EncodedFrame>` で受け渡すのは、スロット交換を 8 バイトのポインタ移動にするため（`EncodedFrame` 本体は 3 本の `Vec` ヘッダで 72 バイト程度だが、将来のフィールド追加に備える）。
- Condvar は 1 本で `ready` / `free` 双方の変化を `notify_all` で通知する（待機者は高々 2 スレッドで、スプリアス起床のコストは無視できる）。

### 3.2 Encode スレッド（`termway-async-render`）

```text
loop:
    job = wait(pending_job or shutdown)          // ① 既存ロジック
    slot = wait(write_state.free.pop())          // ③ 空きスロット獲得
           ※ 待機時間を EncodeStall として記録
    encode(job) → encoder_out
    slot.prefix/suffix を構築, swap(slot.payload, encoder_out)
    spare_buffer = job.pixels を Main へ返却
    wait(write_state.ready.is_none())            // ② ランデブー（INV-1）
    write_state.ready = Some(slot); notify_all
on shutdown: write_state.shutdown = true; notify_all; return
```

> [!IMPORTANT]
> **スロット 2 面で並列性が成立する理由**: Write スレッドが Slot A を送信中（`ready = None`, Slot A は Write が所有）でも、Encode スレッドは `free` から Slot B を取り出してエンコードを進められます。Slot B のエンコード完了時に Slot A がまだ送信中なら、Encode は `ready.is_none()` を待って停止します（これが `EncodeStall` = PTY 律速の可視化指標）。

### 3.3 `swap` による payload とエンコーダ出力の交換

```text
Encode: encoder_out (容量 C1) ⇄ slot.payload (容量 C2)
```

交換後、`encoder_out` は**前回このスロットで送信済みのバッファ**を受け取り、次回のエンコードでそのまま `clear()` して再利用されます。2 面のバッファが Encode と Write の間を循環するため、定常状態での再確保は発生しません（ピーク容量に達するまでの最初の数フレームのみ `Vec` 拡張が起きる）。

> [!WARNING]
> `SixelStreamEncoder` は内部に `band_cache: Vec<Vec<u8>>`（6px バンド単位のエンコード済みキャッシュ）と `quantized` / `band_colors` を持ち、`sixel_out` はそのキャッシュから組み立てられる一時出力です。`sixel_out` を swap で入れ替えても、バンドキャッシュ自体は Encode スレッド内に残るため INV-2 を満たします。一方、**`KittyStreamEncoder` の内部バッファ（`compressor`、`compressed_buf`、`sub_image_buf`、`quantized_buf`、`jpeg_encoder`）は swap 対象にしてはならない**（最終出力 `kitty_out` のみを交換する）。また `has_full_frame` / `subimage_active` / `last_partial_bbox` は「直前フレームが端末に表示済み」を前提とする状態であり、これが INV-1（エンコード後の Drop 禁止）の直接の根拠です。

### 3.4 Write スレッド（`termway-async-write`）

```text
loop:
    slot = wait(ready.take() or shutdown)     // 待機時間を WriteIdle として記録
    if slot is None && shutdown: break
    tiocoutq = ioctl(STDOUT, TIOCOUTQ)        // trace 有効時のみ
    lock stdout; flush; writev_all(slot.io_slices())   // INV-3
    metrics.record_write_us(...); last_write_us.store(...)
    trace: BackendWrite, FrameComplete(flow_end)
    free.push(slot); notify_all
```

- Write スレッドは `ready` が `None` かつ `shutdown == true` になるまで終了しない（INV-5：送信途中での終了禁止）。

### 3.5 `BackendMetrics` の後方互換

| フィールド | 計測スレッド | 変更 |
| :--- | :--- | :--- |
| `jobs_submitted` / `frames_dropped` | Main | 変更なし |
| `jobs_rendered` / `total_encode_us` / `encode_buckets` / `partial_frames` | Encode | 変更なし |
| `total_write_us` / `max_write_us` / `write_buckets` / `total_bytes_written` | **Write** | 計測スレッドのみ移動。意味は同じ |
| `total_encode_stall_us` / `max_encode_stall_us` | Encode | **新規** |
| `total_write_idle_us` | Write | **新規** |
| `frames_written` | Write | **新規**（`percentile_write` の分母を `jobs_rendered` から本値へ変更） |

`src/compositor/profile.rs` の `print_summary` に `Stall` / `Idle` 行を追加し、JSON レポートにも同名フィールドを追加する（既存キーは削除・改名しない）。

### 3.6 フォールバック（安全弁）

`TERMWAY_PIPELINE=serial` 環境変数で M1 時点の単一スレッド直列経路に戻せるようにする。端末エミュレータ側の想定外挙動の切り分けと、性能比較（A/B）に用いる。

---

## 4. トレーシング変更（M3）

### 4.1 `src/trace/event.rs`

```rust
pub enum ThreadRole { Main = 0, AsyncRender = 1, AsyncWrite = 2 }

pub enum EventId {
    // ...
    FramePrepare,   // FrameAssemble を置換（M1）
    EncodeStall,    // M3
    WriteIdle,      // M3
    // FrameAssemble は Perfetto 既存トレースとの互換のため enum 値は残し、記録を停止する
}
```

> [!NOTE]
> `EventId` は `#[repr(u16)]` の連番であり、writer はオフラインで `name()` 文字列へ変換するため、**末尾への追加**であれば既存のトレース出力・テストに影響しません。途中挿入は避けてください。

### 4.2 `src/trace/ring.rs`

- `static WRITE_RING: Mutex<Option<ThreadRing>>` を追加し、`init_rings` / `reset_rings` / `record` / `drain_all` を 3 リング対応にする。
- `writer.rs` の thread_name メタデータ（`ph:"M"`）に `AsyncWrite` を追加。

### 4.3 イベント引数割り当て

| EventId | スレッド | `id` | `arg1` | `arg2` | `arg3` |
| :--- | :--- | :--- | :--- | :--- | :--- |
| `FramePrepare` | AsyncRender | frame_id | prefix+suffix bytes | payload bytes | show_hud |
| `EncodeStall` | AsyncRender | frame_id | 待機 µs | — | 0=free 待ち / 1=ready 待ち |
| `BackendWrite` | **AsyncWrite** | frame_id | total bytes | TIOCOUTQ | iovcnt (下位 8bit) / syscalls (上位 7bit) / error (最上位 1bit) |
| `WriteIdle` | AsyncWrite | 次の frame_id | 待機 µs | — | — |
| `FrameComplete` | **AsyncWrite** | frame_id | — | — | — |

---

## 5. 変更対象ファイル一覧

| ファイル | M1 | M2 | M3 |
| :--- | :---: | :---: | :---: |
| `src/backend/vectored.rs`（新設） | ✅ | | |
| `src/backend/terminal.rs` | ✅ | | ✅ |
| `src/backend/mod.rs` | ✅ | | |
| `src/protocol/kitty.rs`（出力型 `Vec<u8>` 化） | ✅ | | |
| `src/protocol/jpeg.rs` | | ✅ | |
| `src/protocol/jpeg_simd/{mod,avx2,neon}.rs`（新設） | | ✅ | |
| `src/trace/{event,ring,writer}.rs` | ✅ (`FramePrepare`) | | ✅ |
| `src/compositor/profile.rs` | | | ✅ |
| `tests/unit_vectored.rs`（新設） | ✅ | | |
| `tests/unit_kitty_encoder.rs` | ✅ | | |
| `tests/unit_jpeg_simd.rs`（新設） | | ✅ | |
| `tests/unit_pipeline.rs`（新設） | | | ✅ |
| `tests/unit_trace.rs` | | | ✅ |
| `tests/perf_pipeline.rs` | | ✅ | ✅ |
