use termway::trace::event::{EventId, Phase, ThreadRole, TraceEvent};
use termway::trace::ring::{self, ThreadRing};
use termway::trace::writer::{build_json_string, dump_to_file};
use termway::trace;
use std::sync::Mutex;

static TRACE_TEST_MUTEX: Mutex<()> = Mutex::new(());

#[test]
fn test_trace_event_size_is_exactly_32_bytes() {
    assert_eq!(std::mem::size_of::<TraceEvent>(), 32);
    assert_eq!(std::mem::align_of::<TraceEvent>(), 8);
}

#[test]
fn test_thread_ring_push_overflow_drop_oldest() {
    let mut ring = ThreadRing::with_capacity(4);
    assert_eq!(ring.len(), 0);
    assert_eq!(ring.overflow(), 0);

    for i in 1..=6 {
        ring.push(TraceEvent {
            ts_us: i * 1000,
            dur_us: 10,
            id: i as u32,
            arg1: 0,
            arg2: 0,
            arg3: 0,
            lane: 0,
            phase: Phase::Complete,
            thread_id: ThreadRole::Main,
            event_id: EventId::Render,
        });
    }

    // Capacity is 4, 6 items pushed -> 2 overflowed, 4 remain (3, 4, 5, 6)
    assert_eq!(ring.len(), 4);
    assert_eq!(ring.overflow(), 2);

    let (drained, ovf) = ring.drain_ordered();
    assert_eq!(ovf, 2);
    assert_eq!(drained.len(), 4);
    assert_eq!(drained[0].id, 3);
    assert_eq!(drained[1].id, 4);
    assert_eq!(drained[2].id, 5);
    assert_eq!(drained[3].id, 6);

    // After drain, ring is empty
    assert_eq!(ring.len(), 0);
    assert_eq!(ring.overflow(), 0);
}

#[test]
fn test_chrome_trace_json_output_and_file_dump() {
    let _lock = TRACE_TEST_MUTEX.lock().unwrap();
    trace::init(true, true, 64);
    assert!(trace::enabled());

    let ts = trace::now_us();
    trace::slice(
        ThreadRole::Main,
        EventId::Render,
        ts,
        1500,
        1,
        2,
        800,
        600,
    );
    trace::slice(
        ThreadRole::AsyncRender,
        EventId::BackendEncode,
        ts + 2000,
        800,
        1,
        4096,
        3,
        1,
    );

    let (events, ovf) = ring::drain_all();
    assert!(events.len() >= 2);

    let json = build_json_string(&events, ovf);
    assert!(json.starts_with("[\n"));
    assert!(json.ends_with("]\n"));
    assert!(json.contains("\"process_name\""));
    assert!(json.contains("\"Render\""));
    assert!(json.contains("\"BackendEncode\""));
    assert!(json.contains("\"ph\":\"X\""));

    // Test writing to a temporary file
    let tmp_path = format!("/tmp/test_termway_trace_{}.json", std::process::id());
    // Record more events and dump
    trace::slice(
        ThreadRole::Main,
        EventId::DamageScan,
        trace::now_us(),
        100,
        0,
        5,
        1200,
        0,
    );
    dump_to_file(&tmp_path).expect("Failed to dump trace to file");

    let read_back = std::fs::read_to_string(&tmp_path).expect("Failed to read back dumped trace");
    assert!(read_back.contains("\"DamageScan\""));
    let _ = std::fs::remove_file(&tmp_path);

    // Test .gz writing
    let tmp_gz_path = format!("/tmp/test_termway_trace_{}.json.gz", std::process::id());
    trace::slice(
        ThreadRole::Main,
        EventId::DamageScan,
        trace::now_us(),
        100,
        0,
        5,
        1200,
        0,
    );
    dump_to_file(&tmp_gz_path).expect("Failed to dump .gz trace");
    assert!(std::path::Path::new(&tmp_gz_path).exists());
    let metadata = std::fs::metadata(&tmp_gz_path).unwrap();
    assert!(metadata.len() > 0);
    let _ = std::fs::remove_file(&tmp_gz_path);

    ring::reset_rings();
}

#[test]
fn test_trace_disabled_zero_cost() {
    let _lock = TRACE_TEST_MUTEX.lock().unwrap();
    trace::init(false, true, 64);
    assert!(!trace::enabled());

    trace::slice(
        ThreadRole::Main,
        EventId::Render,
        trace::now_us(),
        1000,
        0,
        0,
        0,
        0,
    );

    let (events, ovf) = ring::drain_all();
    assert_eq!(events.len(), 0);
    assert_eq!(ovf, 0);

    ring::reset_rings();
}

#[test]
fn test_frame_id_async_flow_and_backend_drop() {
    let _lock = TRACE_TEST_MUTEX.lock().unwrap();
    trace::init(true, true, 64);
    assert!(trace::enabled());

    let fid1 = trace::frame::alloc_frame_id();
    let fid2 = trace::frame::alloc_frame_id();
    assert_eq!(fid2, fid1 + 1);

    let ts = trace::now_us();
    // 1. Submit frame 1 (FlowStart)
    trace::flow_start(ThreadRole::Main, EventId::EnqueueSubmit, ts, fid1, 4, 0);

    // 2. Drop frame 1 due to frame 2 arriving before worker picked it up
    trace::instant(
        ThreadRole::Main,
        EventId::BackendDrop,
        ts + 500,
        fid1,
        fid2,
        500,
        0,
    );

    // 3. Frame 2 starts and completes on AsyncRender worker
    trace::flow_start(ThreadRole::Main, EventId::EnqueueSubmit, ts + 600, fid2, 1, 0);
    trace::slice(
        ThreadRole::AsyncRender,
        EventId::BackendEncode,
        ts + 1000,
        500,
        fid2,
        2048,
        1,
        0,
    );
    trace::slice(
        ThreadRole::AsyncRender,
        EventId::FrameAssemble,
        ts + 1500,
        50,
        fid2,
        2200,
        0,
        0,
    );
    trace::slice(
        ThreadRole::AsyncRender,
        EventId::BackendWrite,
        ts + 1550,
        400,
        fid2,
        2200,
        1024,
        0,
    );
    trace::flow_end(ThreadRole::AsyncRender, EventId::FrameComplete, ts + 1950, fid2);

    let (events, _ovf) = ring::drain_all();
    assert_eq!(events.len(), 7);

    let json = build_json_string(&events, 0);
    assert!(json.contains("\"ph\":\"s\""));
    assert!(json.contains("\"ph\":\"f\""));
    assert!(json.contains("\"BackendDrop\""));
    assert!(json.contains("\"FrameAssemble\""));
    assert!(json.contains("\"tiocoutq\":1024"));
    assert!(json.contains(&format!("\"dropped_frame\":{}", fid1)));
    assert!(json.contains(&format!("\"by_frame\":{}", fid2)));

    ring::reset_rings();
}

#[test]
fn test_motion_coalescing_and_input_no_input_flag() {
    let _lock = TRACE_TEST_MUTEX.lock().unwrap();
    trace::init(true, true, 64);
    assert!(trace::input_enabled());

    let ts = trace::now_us();
    // Record 5 rapid motion events at ts, ts+10, ts+20, ts+30, ts+40
    for i in 0..5 {
        trace::instant(
            ThreadRole::Main,
            EventId::InputMotionCoalesce,
            ts + i * 10,
            0,
            1,
            (100 << 16) | (200 + i as u32),
            0,
        );
    }

    let (events, _ovf) = ring::drain_all();
    // 5 consecutive motions must be coalesced in-place into 1 slot with count=5
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_id, EventId::InputMotionCoalesce);
    assert_eq!(events[0].arg1, 5);
    assert_eq!(events[0].dur_us, 40);

    // Test input_enabled() gate with TERMWAY_TRACE_NO_INPUT flag
    trace::init(true, false, 64);
    assert!(trace::enabled());
    assert!(!trace::input_enabled());

    trace::instant(
        ThreadRole::Main,
        EventId::InputMouse,
        trace::now_us(),
        0,
        1,
        0,
        1,
    );
    let (no_input_events, _) = ring::drain_all();
    // When input is disabled, mouse events are not recorded
    assert_eq!(no_input_events.len(), 0);

    ring::reset_rings();
}

#[test]
fn test_window_state_counter_track() {
    let _lock = TRACE_TEST_MUTEX.lock().unwrap();
    use smithay::utils::{Point, Size};
    use termway::compositor::window::ManagedWindow;
    use termway::compositor::window_manager::WindowManager;

    trace::init(true, true, 64);
    assert!(trace::enabled());

    let mut wm = WindowManager::new();
    let id = wm.alloc_id();

    // 1. Window creation (Floating = 1)
    let win = ManagedWindow::new_dummy(
        id,
        "TestWindow".into(),
        Point::from((10, 10)),
        Size::from((400, 300)),
    );
    wm.add_window(win);

    // 2. Maximize window (Maximized = 2)
    let win = wm.get_mut(id).unwrap();
    win.maximize(1000, 800);

    // 3. Minimize window (Minimized = 0)
    wm.set_minimized(id, true);

    // 4. Restore window to floating (1)
    let win = wm.get_mut(id).unwrap();
    win.is_maximized = false;
    wm.set_minimized(id, false);

    // 5. Window close
    let _ = wm.remove_window(id);

    let (events, _) = ring::drain_all();

    // Verify counter transitions for win_{id}_state: 1 -> 2 -> 0 -> 1
    let state_transitions: Vec<u32> = events
        .iter()
        .filter(|e| e.event_id == EventId::WindowStateChange && e.phase == Phase::Counter && e.id == id as u32)
        .map(|e| e.arg1)
        .collect();
    assert_eq!(state_transitions, vec![1, 2, 0, 1]);

    // Verify lifecycle instant events: WindowMap, WindowConfigure, WindowClose
    let has_map = events.iter().any(|e| e.event_id == EventId::WindowMap && e.id == id as u32);
    let has_configure = events.iter().any(|e| e.event_id == EventId::WindowConfigure && e.id == id as u32);
    let has_close = events.iter().any(|e| e.event_id == EventId::WindowClose && e.id == id as u32);
    assert!(has_map, "Must record WindowMap");
    assert!(has_configure, "Must record WindowConfigure");
    assert!(has_close, "Must record WindowClose");

    // Verify JSON contains counter track win_{id}_state with ph:"C"
    let json = build_json_string(&events, 0);
    assert!(json.contains(&format!("\"name\":\"win_{}_state\"", id)));
    assert!(json.contains("\"ph\":\"C\""));
    assert!(json.contains("\"args\":{\"value\":1}"));
    assert!(json.contains("\"args\":{\"value\":2}"));
    assert!(json.contains("\"args\":{\"value\":0}"));
    assert!(json.contains("\"WindowMap\""));
    assert!(json.contains("\"WindowConfigure\""));
    assert!(json.contains("\"WindowClose\""));

    ring::reset_rings();
}

#[test]
fn test_snapshot_rotation_and_cli_parsing() {
    let _lock = TRACE_TEST_MUTEX.lock().unwrap();

    // 1. Buffer capacity parser
    assert_eq!(trace::parse_buffer_capacity("4k"), Some(4096));
    assert_eq!(trace::parse_buffer_capacity("64K"), Some(65536));
    assert_eq!(trace::parse_buffer_capacity("1m"), Some(1048576));
    assert_eq!(trace::parse_buffer_capacity("2M"), Some(2097152));
    assert_eq!(trace::parse_buffer_capacity("500"), Some(500));
    assert_eq!(trace::parse_buffer_capacity("invalid"), None);

    // 2. Snapshot path formatter
    assert_eq!(
        termway::trace::writer::format_snapshot_path("/tmp/test.json", 1),
        "/tmp/test.1.json"
    );
    assert_eq!(
        termway::trace::writer::format_snapshot_path("/tmp/test.json.gz", 2),
        "/tmp/test.2.json.gz"
    );
    assert_eq!(
        termway::trace::writer::format_snapshot_path("/tmp/test.gz", 3),
        "/tmp/test.3.gz"
    );
    assert_eq!(
        termway::trace::writer::format_snapshot_path("/tmp/test_custom", 4),
        "/tmp/test_custom.4"
    );

    // 3. Trace file path get/set
    trace::set_trace_file_path(Some("/tmp/my_custom_trace.json".into()));
    assert_eq!(trace::trace_file_path().as_deref(), Some("/tmp/my_custom_trace.json"));

    // 4. Sequential snapshot dumps
    termway::trace::writer::reset_snapshot_index();
    trace::init(true, true, 64);

    trace::slice(
        ThreadRole::Main,
        EventId::Render,
        trace::now_us(),
        1000,
        1,
        1,
        800,
        600,
    );

    let base_path = format!("/tmp/test_snap_rot_{}.json", std::process::id());
    let snap1 = termway::trace::writer::dump_snapshot(&base_path).expect("dump snap 1");
    assert_eq!(snap1, format!("{}.1.json", base_path.strip_suffix(".json").unwrap()));
    assert!(std::path::Path::new(&snap1).exists());
    let c1 = std::fs::read_to_string(&snap1).unwrap();
    assert!(c1.contains("\"Render\""));
    let _ = std::fs::remove_file(&snap1);

    // Emit more events and dump snapshot 2
    trace::slice(
        ThreadRole::AsyncRender,
        EventId::BackendEncode,
        trace::now_us(),
        500,
        2,
        2048,
        1,
        0,
    );
    let snap2 = termway::trace::writer::dump_snapshot(&base_path).expect("dump snap 2");
    assert_eq!(snap2, format!("{}.2.json", base_path.strip_suffix(".json").unwrap()));
    assert!(std::path::Path::new(&snap2).exists());
    let c2 = std::fs::read_to_string(&snap2).unwrap();
    assert!(c2.contains("\"BackendEncode\""));
    let _ = std::fs::remove_file(&snap2);

    ring::reset_rings();
}
