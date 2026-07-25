use std::io::Read;
use std::process::{Command, Stdio};

#[test]
fn test_pty_execution() {
    unsafe {
        let mut master: libc::c_int = 0;
        let mut slave: libc::c_int = 0;
        let mut ws: libc::winsize = std::mem::zeroed();
        ws.ws_col = 80;
        ws.ws_row = 24;

        let ret = libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &ws,
        );
        assert_eq!(ret, 0, "openpty failed");

        let bin_path = env!("CARGO_BIN_EXE_bootart");

        let slave_file1 = std::fs::File::from_raw_fd(slave);
        let slave_file2 = slave_file1.try_clone().unwrap();

        let mut child = Command::new(bin_path)
            .arg("render-final")
            .stdout(Stdio::from(slave_file1))
            .stderr(Stdio::from(slave_file2))
            .spawn()
            .expect("failed to spawn bootart in PTY");

        let status = child.wait().expect("child wait failed");
        assert!(status.success(), "bootart exited with error in PTY");

        let mut master_file = std::fs::File::from_raw_fd(master);
        let mut output = Vec::new();
        // Set non-blocking read or read remaining
        let mut buf = [0u8; 4096];
        loop {
            let mut read_fd: libc::fd_set = std::mem::zeroed();
            libc::FD_ZERO(&mut read_fd);
            libc::FD_SET(master, &mut read_fd);

            let mut tv: libc::timeval = std::mem::zeroed();
            tv.tv_sec = 0;
            tv.tv_usec = 100_000;

            let ready = libc::select(
                master + 1,
                &mut read_fd,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut tv,
            );
            if ready <= 0 {
                break;
            }

            match master_file.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => output.extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
        }

        let output_str = String::from_utf8_lossy(&output);
        assert!(
            output_str.contains("\x1b[?25l"),
            "PTY output should hide cursor"
        );
        assert!(
            output_str.contains("\x1b[?25h"),
            "PTY output should show cursor"
        );
    }
}

use std::os::unix::io::FromRawFd;
