//! Chrome Trace Event JSON serialization and file writer.

use std::fmt::Write as FmtWrite;
use std::io::{self, Write};
use super::event::{EventId, Phase, TraceEvent};

/// Formats args for a given trace event into a valid JSON object string fragment.
fn format_args(ev: &TraceEvent) -> Option<String> {
    match ev.event_id {
        EventId::Render => Some(format!(
            "\"frame_id\":{},\"windows\":{},\"w\":{},\"h\":{}",
            ev.id, ev.arg1, ev.arg2, ev.arg3
        )),
        EventId::DamageScan => Some(format!(
            "\"frame_id\":{},\"rects\":{},\"damaged_px\":{},\"clear\":{}",
            ev.id, ev.arg1, ev.arg2, ev.arg3
        )),
        EventId::EnqueueSubmit => Some(format!(
            "\"rects\":{},\"clear\":{}",
            ev.arg1, ev.arg3
        )),
        EventId::BackendDrop => Some(format!(
            "\"dropped_frame\":{},\"by_frame\":{},\"pending_age_us\":{}",
            ev.id, ev.arg1, ev.arg2
        )),
        EventId::BackendEncode => Some(format!(
            "\"frame_id\":{},\"bytes\":{},\"rects\":{},\"partial\":{}",
            ev.id, ev.arg1, ev.arg2, ev.arg3
        )),
        EventId::FrameAssemble => Some(format!(
            "\"frame_id\":{},\"frame_bytes\":{},\"hud\":{}",
            ev.id, ev.arg1, ev.arg3
        )),
        EventId::BackendWrite => Some(format!(
            "\"frame_id\":{},\"bytes\":{},\"tiocoutq\":{}",
            ev.id, ev.arg1, ev.arg2
        )),
        EventId::InputMouse => Some(format!(
            "\"button\":{},\"x\":{},\"y\":{}",
            ev.arg1, (ev.arg2 >> 16) as u16, (ev.arg2 & 0xFFFF) as u16
        )),
        EventId::InputMotionCoalesce => Some(format!(
            "\"count\":{},\"x\":{},\"y\":{}",
            ev.arg1, (ev.arg2 >> 16) as u16, (ev.arg2 & 0xFFFF) as u16
        )),
        EventId::WindowMap | EventId::WindowConfigure => Some(format!(
            "\"win_id\":{},\"w\":{},\"h\":{},\"maximized\":{}",
            ev.id, ev.arg1, ev.arg2, ev.arg3
        )),
        EventId::WindowRaise | EventId::WindowClose => Some(format!(
            "\"win_id\":{}",
            ev.id
        )),
        EventId::ClientCommit => Some(format!(
            "\"win_id\":{},\"surface_id\":{}",
            ev.id, ev.arg1
        )),
        EventId::ResizeApplied => Some(format!(
            "\"w\":{},\"h\":{}",
            ev.arg1, ev.arg2
        )),
        EventId::X11MapRequest | EventId::X11ConfigureNotify => Some(format!(
            "\"window_id\":{},\"w\":{},\"h\":{}",
            ev.id, ev.arg1, ev.arg2
        )),
        EventId::CounterRenderLatency
        | EventId::CounterEncodeLatency
        | EventId::CounterWriteLatency
        | EventId::CounterRingOverflow
        | EventId::CounterWindowState
        | EventId::WindowStateChange => Some(format!("\"value\":{}", ev.arg1)),
        _ if ev.phase == Phase::Counter => Some(format!("\"value\":{}", ev.arg1)),
        _ => {
            if ev.arg1 > 0 || ev.arg2 > 0 || ev.arg3 > 0 {
                Some(format!("\"arg1\":{},\"arg2\":{},\"arg3\":{}", ev.arg1, ev.arg2, ev.arg3))
            } else {
                None
            }
        }
    }
}

/// Builds a complete Chrome Trace Event JSON array from a slice of events.
pub fn build_json_string(events: &[TraceEvent], total_overflow: u64) -> String {
    let mut out = String::with_capacity(events.len() * 128 + 512);
    out.push_str("[\n");

    // Process & thread metadata
    let _ = writeln!(
        out,
        r#"  {{"name":"process_name","ph":"M","pid":1,"tid":0,"args":{{"name":"termway"}}}},"#
    );
    let _ = writeln!(
        out,
        r#"  {{"name":"thread_name","ph":"M","pid":1,"tid":0,"args":{{"name":"Main"}}}},"#
    );
    let _ = writeln!(
        out,
        r#"  {{"name":"thread_name","ph":"M","pid":1,"tid":1,"args":{{"name":"AsyncRender"}}}},"#
    );

    // Initial overflow counter if any
    if total_overflow > 0 {
        let _ = writeln!(
            out,
            r#"  {{"name":"ring_overflow","cat":"counter","ph":"C","pid":1,"tid":0,"ts":0,"args":{{"value":{}}}}},"#,
            total_overflow
        );
    }

    for (i, ev) in events.iter().enumerate() {
        let name = ev.event_id.name();
        let cat = ev.event_id.category();
        let _ph = ev.phase.as_char();
        let pid = 1;
        let tid = ev.thread_id.tid();
        let ts = ev.ts_us;
        let comma = if i + 1 < events.len() { "," } else { "" };

        let args_part = if let Some(args_str) = format_args(ev) {
            format!(",\"args\":{{{}}}", args_str)
        } else {
            String::new()
        };

        match ev.phase {
            Phase::Complete => {
                let _ = writeln!(
                    out,
                    r#"  {{"name":"{}","cat":"{}","ph":"X","pid":{},"tid":{},"ts":{},"dur":{}{}}}{}"#,
                    name, cat, pid, tid, ts, ev.dur_us, args_part, comma
                );
            }
            Phase::Begin => {
                let _ = writeln!(
                    out,
                    r#"  {{"name":"{}","cat":"{}","ph":"B","pid":{},"tid":{},"ts":{}{}}}{}"#,
                    name, cat, pid, tid, ts, args_part, comma
                );
            }
            Phase::End => {
                let _ = writeln!(
                    out,
                    r#"  {{"name":"{}","cat":"{}","ph":"E","pid":{},"tid":{},"ts":{}}}{}"#,
                    name, cat, pid, tid, ts, comma
                );
            }
            Phase::Instant => {
                let _ = writeln!(
                    out,
                    r#"  {{"name":"{}","cat":"{}","ph":"i","s":"t","pid":{},"tid":{},"ts":{}{}}}{}"#,
                    name, cat, pid, tid, ts, args_part, comma
                );
            }
            Phase::FlowStart => {
                let _ = writeln!(
                    out,
                    r#"  {{"name":"{}","cat":"{}","ph":"s","id":{},"pid":{},"tid":{},"ts":{}{}}}{}"#,
                    name, cat, ev.id, pid, tid, ts, args_part, comma
                );
            }
            Phase::FlowEnd => {
                let _ = writeln!(
                    out,
                    r#"  {{"name":"{}","cat":"{}","ph":"f","bp":"e","id":{},"pid":{},"tid":{},"ts":{}{}}}{}"#,
                    name, cat, ev.id, pid, tid, ts, args_part, comma
                );
            }
            Phase::Counter => {
                let name_buf;
                let counter_name = if ev.event_id == EventId::WindowStateChange
                    || ev.event_id == EventId::CounterWindowState
                {
                    name_buf = format!("win_{}_state", ev.id);
                    &name_buf
                } else {
                    name
                };
                let _ = writeln!(
                    out,
                    r#"  {{"name":"{}","cat":"{}","ph":"C","pid":{},"tid":{},"ts":{}{}}}{}"#,
                    counter_name, cat, pid, tid, ts, args_part, comma
                );
            }
            Phase::Metadata => {
                let _ = writeln!(
                    out,
                    r#"  {{"name":"{}","ph":"M","pid":{},"tid":{},"args":{{{}}}}}{}"#,
                    name, pid, tid, args_part, comma
                );
            }
        }
    }

    out.push_str("]\n");
    out
}

/// Dumps all drained trace events into the specified file path (supporting .gz via flate2 or miniz_oxide).
pub fn dump_to_file(path: &str) -> io::Result<()> {
    let (events, total_overflow) = super::ring::drain_all();
    let json = build_json_string(&events, total_overflow);

    if path.ends_with(".gz") {
        #[cfg(feature = "zlib-ng")]
        {
            use flate2::write::GzEncoder;
            use flate2::Compression;
            let file = std::fs::File::create(path)?;
            let mut encoder = GzEncoder::new(file, Compression::default());
            encoder.write_all(json.as_bytes())?;
            encoder.finish()?;
        }
        #[cfg(not(feature = "zlib-ng"))]
        {
            // Simple fallback: write uncompressed or basic deflate if gzip wrapper is omitted
            let file = std::fs::File::create(path)?;
            let mut writer = std::io::BufWriter::new(file);
            writer.write_all(json.as_bytes())?;
        }
    } else {
        std::fs::write(path, json)?;
    }

    Ok(())
}
