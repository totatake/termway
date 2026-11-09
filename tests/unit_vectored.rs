use std::io::IoSlice;
use termway::backend::vectored::writev_all;

#[test]
fn test_writev_basic_and_exact_content() {
    let mut fds = [0i32; 2];
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    let read_fd = fds[0];
    let write_fd = fds[1];

    let p1 = b"Hello, ";
    let p2 = b"zero-copy ";
    let p3 = b"vectored I/O!\n";

    let mut slices = [
        IoSlice::new(p1),
        IoSlice::new(p2),
        IoSlice::new(p3),
    ];

    let stats = writev_all(write_fd, &mut slices).expect("writev_all should succeed");
    assert_eq!(stats.bytes, p1.len() + p2.len() + p3.len());
    assert!(stats.syscalls >= 1);

    // Read back and compare
    let mut buf = vec![0u8; stats.bytes];
    let n = unsafe { libc::read(read_fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
    assert_eq!(n as usize, stats.bytes);
    assert_eq!(&buf[..], b"Hello, zero-copy vectored I/O!\n");

    unsafe {
        libc::close(read_fd);
        libc::close(write_fd);
    }
}

#[test]
fn test_writev_empty_and_single_slice() {
    let mut fds = [0i32; 2];
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    let read_fd = fds[0];
    let write_fd = fds[1];

    // Empty slices only
    let empty1: &[u8] = &[];
    let empty2: &[u8] = &[];
    let mut slices = [IoSlice::new(empty1), IoSlice::new(empty2)];
    let stats = writev_all(write_fd, &mut slices).expect("writev_all on empty slices should succeed");
    assert_eq!(stats.bytes, 0);
    assert_eq!(stats.syscalls, 0);

    // Single slice
    let data = b"single slice test";
    let mut single = [IoSlice::new(data)];
    let stats = writev_all(write_fd, &mut single).expect("writev_all on single slice should succeed");
    assert_eq!(stats.bytes, data.len());

    let mut read_buf = vec![0u8; data.len()];
    let n = unsafe { libc::read(read_fd, read_buf.as_mut_ptr() as *mut libc::c_void, read_buf.len()) };
    assert_eq!(n as usize, data.len());
    assert_eq!(&read_buf[..], data);

    unsafe {
        libc::close(read_fd);
        libc::close(write_fd);
    }
}

#[test]
fn test_writev_large_payload_exceeding_pipe_capacity() {
    // Pipe buffer in Linux is typically 64KB. Writing 1MB will force multiple partial writes.
    let mut fds = [0i32; 2];
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    let read_fd = fds[0];
    let write_fd = fds[1];

    let total_size = 1024 * 1024; // 1MB
    let data1 = vec![0xAAu8; total_size / 2];
    let data2 = vec![0xBBu8; total_size / 2];

    let reader_handle = std::thread::spawn(move || {
        let mut received = Vec::with_capacity(total_size);
        let mut temp = [0u8; 16384];
        while received.len() < total_size {
            let n = unsafe { libc::read(read_fd, temp.as_mut_ptr() as *mut libc::c_void, temp.len()) };
            if n > 0 {
                received.extend_from_slice(&temp[..n as usize]);
            } else if n == 0 {
                break;
            }
        }
        unsafe { libc::close(read_fd); }
        received
    });

    let mut slices = [
        IoSlice::new(&data1),
        IoSlice::new(&data2),
    ];

    let stats = writev_all(write_fd, &mut slices).expect("writev_all 1MB should succeed");
    assert_eq!(stats.bytes, total_size);
    assert!(stats.syscalls >= 1);

    unsafe { libc::close(write_fd); }

    let received = reader_handle.join().unwrap();
    assert_eq!(received.len(), total_size);
    assert_eq!(&received[..total_size / 2], &data1[..]);
    assert_eq!(&received[total_size / 2..], &data2[..]);
}

#[test]
fn test_writev_nonblocking_eagain_recovery() {
    let mut fds = [0i32; 2];
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    let read_fd = fds[0];
    let write_fd = fds[1];

    // Set write_fd to non-blocking
    unsafe {
        let flags = libc::fcntl(write_fd, libc::F_GETFL);
        assert!(flags >= 0);
        assert_eq!(libc::fcntl(write_fd, libc::F_SETFL, flags | libc::O_NONBLOCK), 0);
    }

    let payload_size = 256 * 1024; // 256KB
    let payload = vec![0x42u8; payload_size];

    let reader_handle = std::thread::spawn(move || {
        // Sleep a bit before draining so write_fd hits EAGAIN
        std::thread::sleep(std::time::Duration::from_millis(50));
        let mut read_all = Vec::with_capacity(payload_size);
        let mut temp = [0u8; 8192];
        while read_all.len() < payload_size {
            let n = unsafe { libc::read(read_fd, temp.as_mut_ptr() as *mut libc::c_void, temp.len()) };
            if n > 0 {
                read_all.extend_from_slice(&temp[..n as usize]);
            }
        }
        unsafe { libc::close(read_fd); }
        read_all
    });

    let mut slices = [IoSlice::new(&payload)];
    let stats = writev_all(write_fd, &mut slices).expect("writev_all with EAGAIN should recover and succeed");
    assert_eq!(stats.bytes, payload_size);
    assert!(stats.eagain_waits > 0, "Expected at least 1 EAGAIN poll wait");

    unsafe { libc::close(write_fd); }

    let received = reader_handle.join().unwrap();
    assert_eq!(received.len(), payload_size);
    assert_eq!(&received[..], &payload[..]);
}
