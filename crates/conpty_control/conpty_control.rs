#[cfg(all(test, windows))]
mod tests {
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};
    use std::{
        io::Read as _,
        sync::mpsc,
        time::{Duration, Instant},
    };

    const TEST_TIMEOUT: Duration = Duration::from_secs(20);

    #[test]
    fn conpty_direct_portable_pty_control() {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty failed");

        let command = CommandBuilder::new("cmd.exe");
        let mut child = pair
            .slave
            .spawn_command(command)
            .expect("spawn cmd.exe failed");
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().expect("clone reader failed");
        let (bytes_tx, bytes_rx) = mpsc::channel();
        let reader_thread = std::thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        eprintln!("[no-gpui-control] read: {count}");
                        if bytes_tx.send(buffer[..count].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        eprintln!("[no-gpui-control] read error: {error}");
                        break;
                    }
                }
            }
        });

        let mut total = Vec::new();
        let deadline = Instant::now() + TEST_TIMEOUT;
        while total.len() < 40 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match bytes_rx.recv_timeout(remaining) {
                Ok(batch) => total.extend_from_slice(&batch),
                Err(error) => panic!(
                    "no output beyond {} bytes from a no-gpui portable-pty cmd.exe session: \
                     {error}; output so far: {:?}",
                    total.len(),
                    String::from_utf8_lossy(&total)
                ),
            }
        }

        eprintln!(
            "[no-gpui-control] session produced output: {:?}",
            String::from_utf8_lossy(&total)
        );
        child.kill().expect("failed to kill cmd.exe");
        let status = child.wait().expect("failed to wait for cmd.exe");
        eprintln!("[no-gpui-control] child exited: {status:?}");
        drop(pair.master);
        reader_thread.join().expect("reader thread panicked");
    }
}
