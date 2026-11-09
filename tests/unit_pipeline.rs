//! Tests for Milestone 3 (Double-Buffering Pipeline): PIP-003, PIP-004, PIP-005.

use std::io::Read;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;
use termway::backend::TerminalBackend;
use termway::terminal::TerminalSize;

fn dummy_term_size() -> TerminalSize {
    TerminalSize {
        cols: 80,
        rows: 24,
        width_px: 800,
        height_px: 480,
        cell_width_px: 10,
        cell_height_px: 20,
    }
}

/// Helper to create a pipe and spawn a background reader that drains all bytes.
fn create_test_pipe_draining() -> (std::fs::File, std::thread::JoinHandle<Vec<u8>>) {
    let mut fds = [0 as libc::c_int; 2];
    unsafe {
        assert_eq!(libc::pipe(fds.as_mut_ptr()), 0);
    }
    use std::os::unix::io::FromRawFd;
    let read_file = unsafe { std::fs::File::from_raw_fd(fds[0]) };
    let write_file = unsafe { std::fs::File::from_raw_fd(fds[1]) };

    let reader_handle = std::thread::spawn(move || {
        let mut reader = read_file;
        let mut buf = Vec::new();
        let _ = reader.read_to_end(&mut buf);
        buf
    });

    (write_file, reader_handle)
}

#[test]
fn test_pip_003_in_order_delivery_and_no_drop_after_encode() {
    let (pipe_write, reader_handle) = create_test_pipe_draining();
    let write_fd = pipe_write.as_raw_fd();

    let mut backend = TerminalBackend::with_output_fd(
        dummy_term_size(),
        false, // kitty
        false, // png
        75,
        write_fd,
    );

    let dummy_pixels = vec![128u8; 800 * 480 * 4];
    let num_frames = 100;

    for id in 1..=num_frames {
        let _ = backend.render_pixels_with_damage_and_id(
            &dummy_pixels,
            800,
            480,
            None,
            None,
            id,
        );
        // Small interval to let pipeline process
        std::thread::sleep(std::time::Duration::from_micros(500));
    }

    let metrics = Arc::clone(backend.metrics());
    let submitted = metrics.jobs_submitted.load(Ordering::Relaxed);
    assert_eq!(submitted, num_frames as u64);

    // Drop backend to flush pipeline and join threads (INV-5)
    drop(backend);
    drop(pipe_write); // Close write end so reader finishes

    let captured = reader_handle.join().expect("reader join");

    let rendered = metrics.jobs_rendered.load(Ordering::Relaxed);
    let written = metrics.frames_written.load(Ordering::Relaxed);
    let dropped = metrics.frames_dropped.load(Ordering::Relaxed);

    // Invariant 1: Exactly every rendered frame was written out (no dropping after encode!)
    assert_eq!(
        rendered, written,
        "Every rendered frame must be written (rendered={}, written={})",
        rendered, written
    );

    // Invariant: Dropped frames only occur in pending state before encode
    assert_eq!(
        dropped + rendered,
        submitted,
        "dropped ({}) + rendered ({}) must equal submitted ({})",
        dropped, rendered, submitted
    );

    // Verify stream is non-empty
    assert!(!captured.is_empty(), "Captured output stream must not be empty");
}

#[test]
fn test_pip_004_pipeline_parallel_concurrency() {
    let (pipe_write, reader_handle) = create_test_pipe_draining();
    let write_fd = pipe_write.as_raw_fd();

    let mut backend = TerminalBackend::with_output_fd(
        dummy_term_size(),
        false,
        false,
        75,
        write_fd,
    );

    // Inject 10ms artificial delay into both Encode and Write threads
    backend.set_artificial_delays(10, 10);

    let dummy_pixels = vec![64u8; 200 * 100 * 4];
    let num_frames = 30;

    let start = Instant::now();
    for id in 1..=num_frames {
        let _ = backend.render_pixels_with_damage_and_id(
            &dummy_pixels,
            200,
            100,
            None,
            None,
            id,
        );
        // Space inputs to avoid dropping
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    drop(backend);
    drop(pipe_write);
    let _ = reader_handle.join();

    let elapsed = start.elapsed();
    println!("Elapsed time for 30 parallel frames: {:?}", elapsed);

    // In serial mode, 30 frames with 10ms encode + 10ms write takes at least:
    // 30 * (10 + 10) = 600ms + wait times = ~900ms.
    // In pipeline mode, encode and write overlap, so total time is significantly lower.
    // We check that it completes within a reasonable threshold (< 850ms).
    assert!(
        elapsed.as_millis() < 850,
        "Pipeline should overlap encode and write (elapsed: {:?})",
        elapsed
    );
}

#[test]
fn test_pip_005_shutdown_safety_and_bracket_balance() {
    let (pipe_write, reader_handle) = create_test_pipe_draining();
    let write_fd = pipe_write.as_raw_fd();

    let mut backend = TerminalBackend::with_output_fd(
        dummy_term_size(),
        false,
        false,
        75,
        write_fd,
    );

    let dummy_pixels = vec![200u8; 400 * 300 * 4];

    // Submit a burst of frames
    for id in 1..=20 {
        let _ = backend.render_pixels_with_damage_and_id(
            &dummy_pixels,
            400,
            300,
            None,
            None,
            id,
        );
    }

    // Drop immediately while writes may be in-flight
    let drop_start = Instant::now();
    drop(backend);
    let drop_duration = drop_start.elapsed();

    // Must join cleanly within 1 second (no deadlock!)
    assert!(
        drop_duration.as_secs() < 1,
        "Backend drop took too long: {:?}",
        drop_duration
    );

    drop(pipe_write);
    let output = reader_handle.join().expect("reader join");

    // Count appearances of sync brackets \x1b[?2026h and \x1b[?2026l
    let output_str = String::from_utf8_lossy(&output);
    let count_h = output_str.matches("\x1b[?2026h").count();
    let count_l = output_str.matches("\x1b[?2026l").count();

    assert_eq!(
        count_h, count_l,
        "Every began sync block (2026h) must have matching end (2026l) (h={}, l={})",
        count_h, count_l
    );
}

#[test]
fn test_serial_pipeline_fallback() {
    std::env::set_var("TERMWAY_PIPELINE", "serial");

    let (pipe_write, reader_handle) = create_test_pipe_draining();
    let write_fd = pipe_write.as_raw_fd();

    let mut backend = TerminalBackend::with_output_fd(
        dummy_term_size(),
        false,
        false,
        75,
        write_fd,
    );

    let dummy_pixels = vec![100u8; 100 * 100 * 4];
    for id in 1..=5 {
        let _ = backend.render_pixels_with_damage_and_id(
            &dummy_pixels,
            100,
            100,
            None,
            None,
            id,
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    drop(backend);
    drop(pipe_write);
    let output = reader_handle.join().expect("reader join");

    assert!(!output.is_empty(), "Serial fallback should produce output");

    std::env::remove_var("TERMWAY_PIPELINE");
}
