use termway::trace::event::{EventId, Phase, ThreadRole, TraceEvent};
use termway::trace::ring::{self, ThreadRing};
use termway::trace::writer::{build_json_string, dump_to_file};
use termway::trace;

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
