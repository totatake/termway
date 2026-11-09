use std::io::{self, IoSlice};
use std::os::unix::io::RawFd;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct WritevStats {
    pub bytes: usize,
    pub syscalls: u32,
    pub eagain_waits: u32,
}

/// Writes all buffers to `fd` using `writev(2)`.
/// Handles partial writes, EINTR, and EAGAIN (with poll wait up to 1000ms).
pub fn writev_all(fd: RawFd, bufs: &mut [IoSlice<'_>]) -> io::Result<WritevStats> {
    let mut stats = WritevStats::default();

    // Find the first non-empty slice
    let mut slice_idx = 0;
    while slice_idx < bufs.len() && bufs[slice_idx].is_empty() {
        slice_idx += 1;
    }

    if slice_idx == bufs.len() {
        return Ok(stats);
    }

    let mut current_offset = 0;

    while slice_idx < bufs.len() {
        // Construct array of iovec pointing to remaining buffers
        // We can pass at most libc::UIO_MAXIOV or 1024
        let remaining_slices = bufs.len() - slice_idx;
        let count = remaining_slices.min(1024);

        let mut iov = Vec::with_capacity(count);
        // First slice might have an offset
        let first_slice = &bufs[slice_idx][current_offset..];
        iov.push(libc::iovec {
            iov_base: first_slice.as_ptr() as *mut libc::c_void,
            iov_len: first_slice.len(),
        });

        for s in &bufs[slice_idx + 1..slice_idx + count] {
            iov.push(libc::iovec {
                iov_base: s.as_ptr() as *mut libc::c_void,
                iov_len: s.len(),
            });
        }

        let res = unsafe { libc::writev(fd, iov.as_ptr(), iov.len() as libc::c_int) };
        stats.syscalls += 1;

        if res > 0 {
            let mut written = res as usize;
            stats.bytes += written;

            while slice_idx < bufs.len() && written > 0 {
                let available = bufs[slice_idx].len() - current_offset;
                if written >= available {
                    written -= available;
                    slice_idx += 1;
                    current_offset = 0;
                } else {
                    current_offset += written;
                    written = 0;
                }
            }
        } else if res == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "writev returned 0 bytes"));
        } else {
            let err = io::Error::last_os_error();
            match err.raw_os_error() {
                Some(libc::EINTR) => continue,
                Some(libc::EAGAIN) => {
                    let mut pfd = libc::pollfd {
                        fd,
                        events: libc::POLLOUT,
                        revents: 0,
                    };
                    let poll_res = unsafe { libc::poll(&mut pfd, 1, 1000) };
                    stats.eagain_waits += 1;
                    if poll_res < 0 {
                        let poll_err = io::Error::last_os_error();
                        if poll_err.raw_os_error() == Some(libc::EINTR) {
                            continue;
                        }
                        return Err(poll_err);
                    }
                    continue;
                }
                _ => return Err(err),
            }
        }
    }

    Ok(stats)
}
