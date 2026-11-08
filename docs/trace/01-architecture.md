# 01. アーキテクチャ設計（Architecture）

本ドキュメントはタイムライン・トレーシング機構の全体像を示します。API の詳細契約は [02-detail-design.md](02-detail-design.md)、採択根拠は [ADR-0003](../adr/0003-zero-allocation-time-line-tracer.md) です。

---

## 1. 設計原則（既存の思想との一貫性）

1. **デフォルト無効・有効化時のみコスト**：無効時は `AtomicBool` 1 回の分岐で return。既存の `--profile` と同一契約。
2. **ホット経路の完全ゼロアロケーション**：記録は「パディングなし 32 バイトの固定長 `Copy` slot 1 個をスレッド毎の独立リングに書く」のみ。ホットパスでは `format!` や文字列参照を一切使わず、JSON 変換は**オフライン（flush 時）**に行う。
3. **`Instant::now()` の取得を最小限に**：セッション開始時刻を `OnceLock` で固定し、時刻は既存の計測点または呼び出し側で取得した単調マイクロ秒 `ts_us` を渡す。
4. **既存集計を消さない**：`CompositorMetrics` / `BackendMetrics` / `LatencyHistogram` は残し、タイムラインは独立モジュール `src/trace/` として併設（無効時はリング未確保）。
5. **`frame_id` を中核に据える**：`Render → Enqueue → Encode → Write → Complete` を 1 本の async flow で跨スレッド追跡可能にする。

---

## 2. データモデル

### 2.1 トレースイベント（固定長 `Copy`、slot 32 バイト・パディング 0）

```rust
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TraceEvent {
    pub ts_us:     u64,        // 8B: セッション開始からの単調 µs（now_us() 由来）
    pub dur_us:    u32,        // 4B: Complete ("X") slice の継続時間（他 phase では 0）
    pub id:        u32,        // 4B: 相関キー（frame_id / input_id / window_id）
    pub arg1:      u32,        // 4B: 構造化数値引数 1（damage_tiles / bytes_encoded / was_frame 等）
    pub arg2:      u32,        // 4B: 構造化数値引数 2（window_count / tiocoutq / pending_age_us 等）
    pub arg3:      u16,        // 2B: 構造化数値引数 3（is_partial / reason_code / window_state 等）
    pub lane:      u8,         // 1B: サブレーンまたは予約
    pub phase:     Phase,      // 1B: Phase::* (#[repr(u8)])
    pub thread_id: ThreadRole, // 2B: ThreadRole::* (#[repr(u16)])
    pub event_id:  EventId,    // 2B: EventId::* (#[repr(u16)])
}
// 合計: 8 + 4 + 4 + 4 + 4 + 2 + 1 + 1 + 2 + 2 = 32 バイトちょうど（アライメントパディング 0）
```

- `phase` と `event_id` は `const fn` で文字列名へ変換可能。
- `arg1`, `arg2`, `arg3` の意味は `event_id` ごとに定まっており、オフラインの `writer.rs` が JSON 出力時に意味のあるキー名（例: `{"bytes": 12340, "tiocoutq": 0}`）へ整形する。これによりホット経路での文字列アロケーション（`format!`）を完全に排除する。

### 2.2 Phase（Chrome Trace の `ph` に対応）

| Phase | Chrome `ph` | 意味 | 用途例 |
| :--- | :--- | :--- | :--- |
| `Complete` | `X` | 同期 duration slice (`ts` + `dur`) | `Render`, `DamageScan`, `BackendEncode`, `FrameAssemble`, `BackendWrite` |
| `Begin` / `End` | `B`/`E` | 継続するスパン（同一スレッド） | ウィンドウのドラッグ移動・リサイズ状態 |
| `Instant` | `i` | 瞬間マーカー | `SendFrames`, `WindowMap`, `WindowClose`, `BackendDrop` |
| `FlowStart` / `FlowEnd` | `s`/`f` | 非同期 flow の開始・終了 | `EnqueueSubmit`（`s`）→ `FrameComplete`（`f`） |
| `Counter` | `C` | 数値 track | レイテンシ、WM 状態番号 (`win_{id}_state`)、ring overflow |
| `Metadata` | `M` | プロセス/スレッド/設定情報 | `process_name`, `thread_name` |

### 2.3 ThreadRole（実稼働スレッドの識別）

Termway の実稼働スレッドは以下の 2 本である（シグナルフラグ検知および F12 スクリーンショット保存はすべて `Main` スレッドのイベントループ内で同期実行される）：

```rust
#[repr(u16)]
pub enum ThreadRole {
    Main        = 0, // TermwayCompositor メインループ（合成・入力・Wayland/X11 ディスパッチ・シグナル処理）
    AsyncRender = 1, // termway-async-render ワーカー（プロトコルエンコード・フレーム構築・PTY 出力）
}
```

### 2.4 EventId（計測点の列挙）

合成/入出力の主要イベントを列挙します（詳細契約は [02-detail-design.md](02-detail-design.md)）：
- **合成パイプライン (`Main`)**: `CompositorPoll`, `Render`, `SendFrames`, `DamageScan`, `EnqueueSubmit`
- **バックエンドワーカー (`AsyncRender`)**: `BackendEncode`, `FrameAssemble`, `BackendWrite`, `FrameComplete`, `BackendDrop`
- **入力 (`Main`)**: `InputRawRecv`, `InputMouse`, `InputMotionCoalesce`, `InputMotionSkip`, `InputKeyBatch`, `WindowDrag`
- **ウィンドウ管理・プロトコル (`Main`)**: `WindowMap`, `WindowConfigure`, `WindowStateChange`, `WindowRaise`, `WindowClose`, `ClientCommit`, `X11MapRequest`, `X11ConfigureNotify`, `Sigwinch`, `ResizeApplied`, `Screenshot`

---

## 3. スレッド独立リングバッファの構成

### 3.1 スレッド毎の独立リング（ロック競合ゼロ・`unsafe` ゼロ）

`Main` スレッドと `AsyncRender` ワーカーが同時にトレースを記録するため、単一リングを共有するとキャッシュライン競合が生じる。また、`unsafe` なロックフリー配列上書きは実行中スナップショット（`SIGUSR1` や定期フラッシュ）での `drain` と重なった際に 32B 構造体の Torn Read（Data Race UB）を引き起こす。

そこで **各スレッド（`Main` = index 0, `AsyncRender` = index 1）に独立した `Mutex<ThreadRing>` を割り当てる**：

```text
ThreadRing (1 thread = 1 independent ring):
  slots:     Vec<TraceEvent>   // トレース有効化時に 1 回だけ確保（デフォルト 65536 slot = 2MB/thread）
  cap:       usize             // リング容量
  head:      usize             // 最古要素インデックス
  len:       usize             // 現在の有効要素数 (0..=cap)
  overflow:  u64               // ring 満杯による drop-oldest 累計回数
```

- **なぜ `Mutex<ThreadRing>` でホットパスが高速か**：
  - 平時、`Main` のリングには `Main` スレッドしかアクセスせず、`AsyncRender` のリングには `AsyncRender` スレッドしかアクセスしない。
  - Linux futex ベースの `std::sync::Mutex` は、競合相手がいない場合（uncontended lock）ユーザ空間の CAS 1 回（**約 15ns**）で完結し、カーネル遷移もメモリアロケーションも一切発生しない。
  - 一方、セッション終了時や `SIGUSR1` スナップショット時にはロックを取得して `drain` するため、**`unsafe` コードを 1 行も書かずに Torn Read や Data Race UB を 100% 防止**できる。

### 3.2 overflow ポリシー

`len == cap` の状態で新規イベントが記録されると、`slots[head]` を上書きして `head = (head + 1) % cap` とし（`drop-oldest`）、`overflow += 1` を記録する。満杯時は常に「直近の N 秒」を保持する形になる。`overflow` カウンタは JSON 出力時に **必ず Counter track として出力**し、また `--hud` 有効時には HUD にも表示する。

---

## 4. `frame_id` による async flow event（中核機能）

### 4.1 「メッセージのやり取り」の可視化

Chrome Trace の async flow（`ph:"s"`/`ph:"f"`）を使い、`frame_id` を共通の `id` と `cat:"frame"` にして跨スレッド追跡可能にします。

```text
frame_id = F（メインスレッドのフレーム送信時に採番）

[Main]        [X slice] Render         (t0, dur=1.25ms, arg1=windows)
[Main]        [i]       SendFrames     (t1)
[Main]        [X slice] DamageScan     (t1, dur=0.18ms, arg1=damage_rects)
[Main]        [flow "s"] EnqueueSubmit (t2, id=F, arg1=damage_rects)
              ─────────────────────────── cross-thread ───────────────────────────
[AsyncRender] [X slice] BackendEncode  (t3, dur=2.10ms, id=F, arg1=bytes_encoded, arg2=damage_rects, arg3=is_partial)
[AsyncRender] [X slice] FrameAssemble  (t4, dur=0.05ms, id=F, arg1=frame_buf_bytes)
[AsyncRender] [X slice] BackendWrite   (t5, dur=0.85ms, id=F, arg1=frame_buf_bytes, arg2=tiocoutq_bytes)
[AsyncRender] [flow "f"] FrameComplete (t6, id=F)
```

Perfetto UI で `frame_id=F` を選択すると、跨スレッドの矢印が 1 本に繋がり「どのフレームがどの段階で詰まったか」をドリルダウンできます。
また、メインスレッドが `render_pixels_with_damage` を呼んだ際にワーカーがまだ前フレームを処理中で `pending_job` が上書き破棄された場合は、`BackendDrop`（`ph:"i"`, `id=dropped_fid`, `arg1=new_fid`, `arg2=pending_age_us`）を発行します。

### 4.2 各イベントの構造化引数 (`arg1`, `arg2`, `arg3`)

すべて 32B `TraceEvent` 内の整数フィールドに格納され、`writer.rs` で以下の JSON `args` に展開されます：

| `EventId` | `id` | `arg1` | `arg2` | `arg3` | JSON 出力時の `args` |
| :--- | :--- | :--- | :--- | :--- | :--- |
| `Render` | `frame_id` | `window_count` | `phys_w` | `phys_h` | `{"frame_id":F,"windows":W,"w":PW,"h":PH}` |
| `DamageScan` | `frame_id` | `damage_rects` | `damaged_pixels` | `needs_clear` | `{"frame_id":F,"rects":R,"damaged_px":P,"clear":C}` |
| `EnqueueSubmit` | `frame_id` | `damage_rects` | `0` | `needs_clear` | `{"rects":R,"clear":C}` |
| `BackendDrop` | `dropped_fid` | `by_frame_id` | `pending_age_us` | `0` | `{"dropped_frame":D,"by_frame":B,"pending_age_us":A}` |
| `BackendEncode` | `frame_id` | `bytes_encoded` | `damage_rects` | `is_partial` | `{"frame_id":F,"bytes":B,"rects":R,"partial":P}` |
| `FrameAssemble` | `frame_id` | `frame_buf_bytes` | `0` | `show_hud` | `{"frame_id":F,"frame_bytes":FB,"hud":H}` |
| `BackendWrite` | `frame_id` | `frame_buf_bytes` | `tiocoutq_bytes` | `0` | `{"frame_id":F,"bytes":FB,"tiocoutq":Q}` |

---

## 5. `BackendWrite` の正確な分離（`FrameAssemble` vs `BackendWrite` + `TIOCOUTQ`）

Rust の `std::io::Stdout` は内部に 1024B の `LineWriter` を持つため、1KB を超える `frame_buf` の `write_all(&frame_buf)` はバッファをバイパスして直接 OS の `write(2)` システムコールを呼び出す。したがって、SSH/tmux 経由の PTY 送信バッファ枯渇によるブロッキングは **100% `write_all()` の中で発生**し、直後の `flush()` は常に `≈ 0 µs` となる。

一方で現状の `src/backend/terminal.rs` では、`write_start = Instant::now()` の後に `frame_buf` のクリアと各シーケンスの `extend_from_slice`（メモリコピー）を行ってから `stdout.lock().write_all(&frame_buf)` を呼んでいる。

そこで本設計では：
1. `frame_buf` のメモリ組み立て時間を **`FrameAssemble`** として切り出す。
2. 書き込み直前に `libc::ioctl(libc::STDOUT_FILENO, libc::TIOCOUTQ, &mut outq)` でカーネル PTY 送信キューの未送信バイト数を取得し、純粋な `stdout.lock().write_all(&frame_buf) + flush()` の所要時間を **`BackendWrite`**（`arg1 = frame_buf.len()`, `arg2 = outq`）として記録する。

これにより、Perfetto 上で「エンコード時間」「バッファ組み立て」「PTY 書き込み遅延と送信キュー詰まり (`tiocoutq`)」を正確に切り分けることができる。

---

## 6. 入力イベントの可視化と高頻度対策

- **高頻度イベントの共結成（coalescing）**：
  - `Any-event 1003h` + `SGR-Pixels 1016h` ではマウス移動 1px 毎にイベントが到着する。
  - 連続するポインタ `Motion` は直前の `InputMotionCoalesce` スロットの `dur_us` と `arg1` (`count`) をインプレース更新して 1 スライスに畳み込み、リング消費を抑制する。
  - ウィンドウ移動・リサイズドラッグ中は `WindowDrag`（`Phase::Begin` / `Phase::End`）で区間を記録する。
  - `--trace-no-input`（または `TERMWAY_TRACE_NO_INPUT=1`）で入力系トレースを完全に無効化できる。

---

## 7. WM 状態遷移の counter track

`ARCHITECTURE.md` §2.3 のステートマシンを、ウィンドウ単位の counter track（`ph:"C"`、`id = window_id`、`arg3 = 0(Minimized) | 1(Floating) | 2(Maximized)`）として出力する。Perfetto 上で各ウィンドウがいつ最大化・最小化・復元されたかがステップグラフとして可視化される。

---

## 8. flush トリガーと既存シグナルの共存

1. **セッション終了**：`CompositorMetrics::print_summary` と並べてリングを JSON（または `.gz`）へ出力。`print_summary` の JSON レポートにも `"trace_file"` フィールドを追記して紐づける。
2. **`SIGUSR1` / `SIGUSR2` 連動**：既存の `src/compositor/env.rs` では `SIGUSR1` が `CAPTURE_REQUESTED`（スクリーンショット保存）、`SIGUSR2` が `STATE_DUMP_REQUESTED`（GUI 状態 JSON ダンプ）をセットする。シグナルハンドラ自体は Async-Signal-Safe（`AtomicBool` のみ）を保ち、`src/compositor/mod.rs` のメインループでこれらが検知された際、`trace::enabled()` であれば併せてトレースの時点スナップショット（連番ファイル）を出力する。
3. **`--trace-flush-secs=N`**：メインループ内で N 秒ごとに連番スナップショットを出力する。
4. **panic フック**：既存の panic 安全端末復元（`RawModeGuard` による termios 復元 + `\x1b[?2026l`）が完了した**後**に、`Once` ガード付きでトレースを保存する。

---

## 9. `--hud` との統合

`--hud` と `--trace` が同時に有効な場合、HUD の末尾行に `Drop/Ovf`（フレームドロップ数 / トレースリング overflow 数）を表示し、トレースリングの溢れをリアルタイムに把握できるようにする。

---

## 10. モジュール構成

```text
src/trace/
├── mod.rs      # ENABLED / INPUT_ENABLED フラグ、session_start()、now_us()、記録ヘルパ関数
├── event.rs    # TraceEvent (#[repr(C)] 32B, Copy), Phase, ThreadRole, EventId
├── ring.rs     # ThreadRing（スレッド独立リングバッファ）、グローバルレジストリ
├── frame.rs    # NEXT_FRAME_ID 採番、フレームパイプライン用トレースヘルパ
└── writer.rs   # Chrome Trace JSON / gzip (.gz) 変換・ファイル出力・連番スナップショット
```
