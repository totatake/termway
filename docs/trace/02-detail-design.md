# 02. 詳細設計（Detail Design）

`src/trace/` の API 契約と、既存モジュールへの組み込み点、Chrome Trace JSON 変換、panic/シグナル連動を示します。概念説明は [01-architecture.md](01-architecture.md) を参照してください。

---

## 1. `src/trace/mod.rs` — グローバルゲートと時刻

```rust
pub mod event;
pub mod ring;
pub mod frame;
pub mod writer;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

pub static ENABLED: AtomicBool = AtomicBool::new(false);
pub static INPUT_ENABLED: AtomicBool = AtomicBool::new(true);
static SESSION_START: OnceLock<Instant> = OnceLock::new();

#[inline]
pub fn init(enabled: bool, input_enabled: bool, buffer_capacity: usize) {
    let _ = SESSION_START.get_or_init(Instant::now);
    INPUT_ENABLED.store(input_enabled, Ordering::Relaxed);
    if enabled {
        ring::init_rings(buffer_capacity);
        ENABLED.store(true, Ordering::Release);
    } else {
        ENABLED.store(false, Ordering::Release);
    }
}

#[inline]
pub fn session_start() -> Instant {
    *SESSION_START.get_or_init(Instant::now)
}

/// セッション開始からの単調 µs。
#[inline]
pub fn now_us() -> u64 {
    session_start().elapsed().as_micros() as u64
}

/// --trace / TERMWAY_TRACE が有効なときのみ true を返す。
#[inline]
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// --trace-no-input で false。入力イベント記録のゲート。
#[inline]
pub fn input_enabled() -> bool {
    enabled() && INPUT_ENABLED.load(Ordering::Relaxed)
}
```

### 1.1 ゼロアロケーション記録ヘルパ関数契約（`enabled` ガード必須）

すべての記録ヘルパは **数値引数 (`u32`, `u16`) のみ** を受け取り、`&str` や `format!` を一切受け取らない：

```rust
// 同期 duration slice（ph:"X"）
#[inline]
pub fn slice(
    thread: ThreadRole,
    event_id: EventId,
    ts_us: u64,
    dur_us: u32,
    id: u32,
    arg1: u32,
    arg2: u32,
    arg3: u16,
) {
    if !enabled() {
        return;
    }
    ring::record(
        thread,
        TraceEvent {
            ts_us,
            dur_us,
            id,
            arg1,
            arg2,
            arg3,
            lane: 0,
            phase: Phase::Complete,
            thread_id: thread,
            event_id,
        },
    );
}

// async flow start/end（ph:"s"/"f"）
#[inline]
pub fn flow_start(thread: ThreadRole, event_id: EventId, ts_us: u64, id: u32, arg1: u32, arg3: u16);
#[inline]
pub fn flow_end(thread: ThreadRole, event_id: EventId, ts_us: u64, id: u32);

// instant marker（ph:"i"）
#[inline]
pub fn instant(thread: ThreadRole, event_id: EventId, ts_us: u64, id: u32, arg1: u32, arg2: u32, arg3: u16);

// counter track（ph:"C"）
#[inline]
pub fn counter(thread: ThreadRole, event_id: EventId, ts_us: u64, id: u32, value: u32);
```

---

## 2. `src/trace/event.rs` — データモデル（32 バイト固定長 `Copy`）

```rust
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TraceEvent {
    pub ts_us: u64,            // 8B
    pub dur_us: u32,           // 4B
    pub id: u32,               // 4B
    pub arg1: u32,             // 4B
    pub arg2: u32,             // 4B
    pub arg3: u16,             // 2B
    pub lane: u8,              // 1B
    pub phase: Phase,          // 1B
    pub thread_id: ThreadRole, // 2B
    pub event_id: EventId,     // 2B
}

const _: () = assert!(std::mem::size_of::<TraceEvent>() == 32);

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Complete = 0,  // "X"
    Begin = 1,     // "B"
    End = 2,       // "E"
    Instant = 3,   // "i"
    FlowStart = 4, // "s"
    FlowEnd = 5,   // "f"
    Counter = 6,   // "C"
    Metadata = 7,  // "M"
}

#[repr(u16)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadRole {
    Main = 0,
    AsyncRender = 1,
}

#[repr(u16)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventId {
    // 合成パイプライン (Main)
    CompositorPoll,
    Render,
    SendFrames,
    DamageScan,
    EnqueueSubmit,
    // バックエンドワーカー (AsyncRender)
    BackendEncode,
    FrameAssemble,
    BackendWrite,
    FrameComplete,
    BackendDrop,
    // 入力 (Main)
    InputRawRecv,
    InputMouse,
    InputMotionCoalesce,
    InputMotionSkip,
    InputKeyBatch,
    WindowDrag,
    // WM / Wayland / X11 (Main)
    WindowMap,
    WindowConfigure,
    WindowStateChange,
    WindowRaise,
    WindowClose,
    ClientCommit,
    X11MapRequest,
    X11ConfigureNotify,
    Sigwinch,
    ResizeApplied,
    Screenshot,
}

impl EventId {
    /// Perfetto の slice name / counter track 名に変換（ヒープ確保なし）。
    pub const fn name(self) -> &'static str { /* match ... */ }
    pub const fn category(self) -> &'static str { /* "compositor" | "backend" | "input" | "wm" | "frame" */ }
}
```

---

## 3. `src/trace/ring.rs` — スレッド独立リングバッファ

```rust
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use super::event::{EventId, Phase, ThreadRole, TraceEvent};

pub struct ThreadRing {
    slots: Vec<TraceEvent>,
    cap: usize,
    head: usize,
    len: usize,
    overflow: u64,
}

pub static TOTAL_OVERFLOW: AtomicU64 = AtomicU64::new(0);

static MAIN_RING: Mutex<Option<ThreadRing>> = Mutex::new(None);
static WORKER_RING: Mutex<Option<ThreadRing>> = Mutex::new(None);

impl ThreadRing {
    pub fn with_capacity(cap: usize) -> Self {
        let cap = cap.max(64);
        Self {
            slots: Vec::with_capacity(cap),
            cap,
            head: 0,
            len: 0,
            overflow: 0,
        }
    }

    #[inline]
    pub fn push(&mut self, ev: TraceEvent) {
        if self.cap == 0 {
            return;
        }
        // 高頻度マウス移動の共結成（直前イベントが InputMotionCoalesce ならインプレース合算）
        if ev.event_id == EventId::InputMotionCoalesce && self.len > 0 {
            let last_idx = (self.head + self.len - 1) % self.cap;
            let last = &mut self.slots[last_idx];
            if last.event_id == EventId::InputMotionCoalesce {
                last.dur_us = ev.ts_us.saturating_sub(last.ts_us) as u32;
                last.arg1 = last.arg1.saturating_add(1);
                last.arg2 = ev.arg2; // 最新座標
                return;
            }
        }

        if self.len < self.cap {
            let idx = (self.head + self.len) % self.cap;
            if idx == self.slots.len() {
                self.slots.push(ev);
            } else {
                self.slots[idx] = ev;
            }
            self.len += 1;
        } else {
            // Ring full: overwrite oldest slot (drop-oldest) and advance head
            self.slots[self.head] = ev;
            self.head = (self.head + 1) % self.cap;
            self.overflow += 1;
            TOTAL_OVERFLOW.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 古い順（FIFO）に全イベントを取り出し、リングを空にする。
    pub fn drain_ordered(&mut self) -> (Vec<TraceEvent>, u64) {
        let mut out = Vec::with_capacity(self.len);
        for i in 0..self.len {
            let idx = (self.head + i) % self.cap;
            out.push(self.slots[idx]);
        }
        let ovf = self.overflow;
        self.head = 0;
        self.len = 0;
        (out, ovf)
    }
}
```

- `MAIN_RING` は平時 `Main` スレッドのみがロックし、`WORKER_RING` は平時 `AsyncRender` スレッドのみがロックする。
- これにより平時はスレッド間競合が一切発生せず（uncontended mutex lock）、かつ `drain_ordered()` 呼び出し時には排他ロックによって Torn Read や Data Race UB が 100% 防止される（`unsafe` ブロック 0 個）。

---

## 4. `src/trace/frame.rs` — `frame_id` とパイプライン記録ヘルパ

```rust
use std::sync::atomic::{AtomicU32, Ordering};

/// セッション中にフレームごとに採番する単調増加 ID。
pub static NEXT_FRAME_ID: AtomicU32 = AtomicU32::new(1);

#[inline]
pub fn alloc_frame_id() -> u32 {
    NEXT_FRAME_ID.fetch_add(1, Ordering::Relaxed)
}
```

---

## 5. `src/trace/writer.rs` — Chrome Trace Event JSON / `.gz` 出力

```rust
pub fn dump_to_file(path: &str) -> std::io::Result<()>;
pub fn build_json() -> String;
```

マッピング表：

| `Phase` | `ph` | 出力 JSON 形式（1 event 1 line） |
| :--- | :--- | :--- |
| `Complete` | `"X"` | `{"name":"...","cat":"...","ph":"X","pid":1,"tid":T,"ts":TS,"dur":DUR,"args":{...}}` |
| `Begin` | `"B"` | `{"name":"...","cat":"...","ph":"B","pid":1,"tid":T,"ts":TS,"args":{...}}` |
| `End` | `"E"` | `{"name":"...","cat":"...","ph":"E","pid":1,"tid":T,"ts":TS}` |
| `Instant` | `"i"` | `{"name":"...","cat":"...","ph":"i","s":"t","pid":1,"tid":T,"ts":TS,"args":{...}}` |
| `FlowStart` | `"s"` | `{"name":"EnqueueSubmit","cat":"frame","ph":"s","id":F,"pid":1,"tid":0,"ts":TS,"args":{...}}` |
| `FlowEnd` | `"f"` | `{"name":"FrameComplete","cat":"frame","ph":"f","bp":"e","id":F,"pid":1,"tid":1,"ts":TS}` |
| `Counter` | `"C"` | `{"name":"...","cat":"...","ph":"C","pid":1,"tid":T,"ts":TS,"args":{"value":V}}` |
| `Metadata` | `"M"` | `{"name":"thread_name","ph":"M","pid":1,"tid":T,"args":{"name":"..."}}` |

- パスが `.gz` で終わる場合は、既存の `flate2::write::GzEncoder` を用いて gzip 圧縮して書き出す。

---

## 6. 既存モジュールへの組み込み点

### 6.1 `src/compositor/mod.rs`（メインループ）

```rust
let fid = if trace::enabled() { trace::frame::alloc_frame_id() } else { 0 };
let r_start_us = if trace::enabled() { trace::now_us() } else { 0 };
let render_res = self.renderer.render(&self.state);
let r_dur = t_r0.elapsed();
let r_us = r_dur.as_micros() as u64;
if trace::enabled() {
    trace::slice(
        ThreadRole::Main,
        EventId::Render,
        r_start_us,
        r_us as u32,
        fid,
        self.state.wm.windows.len() as u32,
        w,
        h as u16,
    );
}
```

### 6.2 `src/backend/terminal.rs`（ワーカーループ & エンキュー）

1. `RenderJob` に `frame_id: u32` と `enqueue_ts_us: u64` を追加する。
2. `render_pixels_with_damage` で `guard.pending_job` が既に存在していた場合（フレームドロップ発生時）、古い `job.frame_id` と経過時間 `now_us().saturating_sub(job.enqueue_ts_us)` を `EventId::BackendDrop` として記録する。
3. ワーカー内で：
   - エンコード区間を `EventId::BackendEncode`（`arg1 = bytes_encoded`, `arg2 = damage_rects`, `arg3 = is_partial as u16`）として記録。
   - `frame_buf` のメモリ組み立て区間を `EventId::FrameAssemble`（`arg1 = frame_buf.len() as u32`）として記録し、`write_start` は **`frame_buf` 組み立て完了後（`stdout.lock()` の直前）** に移動する（これにより `--profile` の `Terminal Write` も純粋な PTY I/O 時間となる）。
   - `trace::enabled()` 時、`libc::ioctl(libc::STDOUT_FILENO, libc::TIOCOUTQ, &mut outq)` で PTY 送信キュー滞留バイト数を取得し、`stdout.lock().write_all(&frame_buf)` + `flush()` 完了後に `EventId::BackendWrite`（`arg1 = frame_buf.len() as u32`, `arg2 = outq as u32`）と `FlowEnd` (`EventId::FrameComplete`, `id = job.frame_id`) を記録する。

---

## 7. シグナル / panic ハンドリングとの連携

- **`SIGUSR1` / `SIGUSR2`**：`src/compositor/env.rs` のシグナルハンドラは従来通り `CAPTURE_REQUESTED` / `STATE_DUMP_REQUESTED` の `AtomicBool` のみをセットする（Async-Signal-Safe）。`src/compositor/mod.rs` のステップ G でこれらが処理される際、`trace::enabled()` であれば `trace::writer::dump_snapshot()` を呼び出し、連番ファイル（例: `/tmp/termway.trace.1.json`）へスナップショットを書き出す。
- **panic フック**：`src/terminal/raw.rs` の panic フックにて、`restore_emergency()` による端末復元と `\x1b[?2026l` 出力が完了した直後に、トレースが有効かつ出力先ファイルが指定されていれば `Once` ガード付きでダンプを試みる。

---

## 8. テスト設計

```rust
#[cfg(test)]
mod tests {
    // 1. size_of::<TraceEvent>() == 32 の静的・動的検証
    // 2. ring 容量超過時に overflow counter が立ち、drop-oldest で直近イベントが順序通り保持されること
    // 3. InputMotionCoalesce が連続投入時に 1 スロットへ畳み込まれること
    // 4. frame_id 採番および FlowStart -> FlowEnd を含む Chrome Trace JSON が有効な JSON 配列として出力されること
    // 5. .gz パス指定時に flate2 で圧縮出力され、解凍すると元の JSON と一致すること
}
```
