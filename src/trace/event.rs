//! Event definitions and types for zero-allocation timeline tracing.

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TraceEvent {
    pub ts_us: u64,            // 8B: microseconds from session start
    pub dur_us: u32,           // 4B: duration for Complete ("X") slices
    pub id: u32,               // 4B: correlation key (frame_id / input_id / window_id)
    pub arg1: u32,             // 4B: structured numerical arg 1
    pub arg2: u32,             // 4B: structured numerical arg 2
    pub arg3: u16,             // 2B: structured numerical arg 3
    pub lane: u8,              // 1B: sub-lane or reserved
    pub phase: Phase,          // 1B: Phase enum
    pub thread_id: ThreadRole, // 2B: ThreadRole enum
    pub event_id: EventId,     // 2B: EventId enum
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

impl Phase {
    pub const fn as_char(self) -> char {
        match self {
            Phase::Complete => 'X',
            Phase::Begin => 'B',
            Phase::End => 'E',
            Phase::Instant => 'i',
            Phase::FlowStart => 's',
            Phase::FlowEnd => 'f',
            Phase::Counter => 'C',
            Phase::Metadata => 'M',
        }
    }
}

#[repr(u16)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadRole {
    Main = 0,
    AsyncRender = 1,
}

impl ThreadRole {
    pub const fn name(self) -> &'static str {
        match self {
            ThreadRole::Main => "Main",
            ThreadRole::AsyncRender => "AsyncRender",
        }
    }

    pub const fn tid(self) -> u32 {
        self as u32
    }
}

#[repr(u16)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventId {
    // Compositor Pipeline (Main)
    CompositorPoll,
    Render,
    SendFrames,
    DamageScan,
    EnqueueSubmit,

    // Backend Worker (AsyncRender)
    BackendEncode,
    FrameAssemble,
    BackendWrite,
    FrameComplete,
    BackendDrop,

    // Input (Main)
    InputRawRecv,
    InputMouse,
    InputMotionCoalesce,
    InputMotionSkip,
    InputKeyBatch,
    WindowDrag,

    // Window Management & Protocol (Main)
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

    // Counters
    CounterRenderLatency,
    CounterEncodeLatency,
    CounterWriteLatency,
    CounterRingOverflow,
    CounterWindowState,
}

impl EventId {
    pub const fn name(self) -> &'static str {
        match self {
            EventId::CompositorPoll => "CompositorPoll",
            EventId::Render => "Render",
            EventId::SendFrames => "SendFrames",
            EventId::DamageScan => "DamageScan",
            EventId::EnqueueSubmit => "EnqueueSubmit",

            EventId::BackendEncode => "BackendEncode",
            EventId::FrameAssemble => "FrameAssemble",
            EventId::BackendWrite => "BackendWrite",
            EventId::FrameComplete => "FrameComplete",
            EventId::BackendDrop => "BackendDrop",

            EventId::InputRawRecv => "InputRawRecv",
            EventId::InputMouse => "InputMouse",
            EventId::InputMotionCoalesce => "InputMotionCoalesce",
            EventId::InputMotionSkip => "InputMotionSkip",
            EventId::InputKeyBatch => "InputKeyBatch",
            EventId::WindowDrag => "WindowDrag",

            EventId::WindowMap => "WindowMap",
            EventId::WindowConfigure => "WindowConfigure",
            EventId::WindowStateChange => "WindowStateChange",
            EventId::WindowRaise => "WindowRaise",
            EventId::WindowClose => "WindowClose",
            EventId::ClientCommit => "ClientCommit",
            EventId::X11MapRequest => "X11MapRequest",
            EventId::X11ConfigureNotify => "X11ConfigureNotify",
            EventId::Sigwinch => "Sigwinch",
            EventId::ResizeApplied => "ResizeApplied",
            EventId::Screenshot => "Screenshot",

            EventId::CounterRenderLatency => "render_ms",
            EventId::CounterEncodeLatency => "encode_ms",
            EventId::CounterWriteLatency => "write_ms",
            EventId::CounterRingOverflow => "ring_overflow",
            EventId::CounterWindowState => "window_state",
        }
    }

    pub const fn category(self) -> &'static str {
        match self {
            EventId::CompositorPoll
            | EventId::Render
            | EventId::SendFrames
            | EventId::DamageScan => "compositor",

            EventId::EnqueueSubmit
            | EventId::FrameComplete => "frame",

            EventId::BackendEncode
            | EventId::FrameAssemble
            | EventId::BackendWrite
            | EventId::BackendDrop => "backend",

            EventId::InputRawRecv
            | EventId::InputMouse
            | EventId::InputMotionCoalesce
            | EventId::InputMotionSkip
            | EventId::InputKeyBatch
            | EventId::WindowDrag => "input",

            EventId::WindowMap
            | EventId::WindowConfigure
            | EventId::WindowStateChange
            | EventId::WindowRaise
            | EventId::WindowClose
            | EventId::ClientCommit
            | EventId::X11MapRequest
            | EventId::X11ConfigureNotify => "wm",

            EventId::Sigwinch
            | EventId::ResizeApplied
            | EventId::Screenshot => "system",

            EventId::CounterRenderLatency
            | EventId::CounterEncodeLatency
            | EventId::CounterWriteLatency
            | EventId::CounterRingOverflow
            | EventId::CounterWindowState => "counter",
        }
    }
}
