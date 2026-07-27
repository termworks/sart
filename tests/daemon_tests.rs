use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(1);

struct TestTree {
    root: PathBuf,
    runtime: PathBuf,
    cmdline: PathBuf,
}

impl TestTree {
    fn new(cmdline: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "bootart-daemon-test-{}-{}",
            std::process::id(),
            NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let runtime = root.join("runtime");
        let cmdline_path = root.join("cmdline");
        fs::write(&cmdline_path, cmdline).unwrap();
        Self {
            root,
            runtime,
            cmdline: cmdline_path,
        }
    }
}

impl Drop for TestTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct DaemonChild(Option<Child>);

impl DaemonChild {
    fn spawn(tree: &TestTree) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_bootart"))
            .arg("daemon")
            .arg("--runtime-dir")
            .arg(&tree.runtime)
            .arg("--cmdline")
            .arg(&tree.cmdline)
            .arg("--test-buffer")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        Self(Some(child))
    }

    fn wait(mut self) -> std::process::ExitStatus {
        self.0.take().unwrap().wait().unwrap()
    }
}

impl Drop for DaemonChild {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let report_stderr = std::thread::panicking();
            // SAFETY: the child PID belongs to this test and SIGTERM is the
            // daemon's supported cleanup path.
            unsafe {
                libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
            }
            let _ = child.wait();
            if report_stderr {
                let mut stderr = String::new();
                if let Some(mut pipe) = child.stderr.take() {
                    let _ = pipe.read_to_string(&mut stderr);
                }
                if !stderr.is_empty() {
                    eprintln!("daemon stderr after test failure:\n{stderr}");
                }
            }
        }
    }
}

fn exited_daemon_failure(daemon: &mut DaemonChild) -> Option<String> {
    daemon.0.as_mut()?.try_wait().unwrap()?;
    let mut child = daemon.0.take().unwrap();
    // Child::wait returns the cached status after try_wait reaps the process;
    // keeping the explicit call also documents that this path cannot leak a
    // zombie if the test reports an early daemon failure.
    let status = child.wait().unwrap();
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        pipe.read_to_string(&mut stderr).unwrap();
    }
    Some(format!("daemon exited with {status}: {stderr}"))
}

fn wait_for_socket(tree: &TestTree, daemon: &mut DaemonChild) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if tree.runtime.join("control.sock").exists() {
            return;
        }
        if let Some(failure) = exited_daemon_failure(daemon) {
            panic!("{failure}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("daemon did not create its control socket");
}

fn wait_for_replaced_socket(tree: &TestTree, daemon: &mut DaemonChild, stale_inode: u64) {
    let socket = tree.runtime.join("control.sock");
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if fs::symlink_metadata(&socket)
            .map(|metadata| metadata.ino() != stale_inode)
            .unwrap_or(false)
        {
            return;
        }
        if let Some(failure) = exited_daemon_failure(daemon) {
            panic!("{failure}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("restarted daemon did not replace its stale socket");
}

fn control(tree: &TestTree, arguments: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_bootart"));
    command
        .args(arguments)
        .arg("--runtime-dir")
        .arg(&tree.runtime);
    command.output().unwrap()
}

fn control_success(tree: &TestTree, arguments: &[&str]) -> Output {
    let output = control(tree, arguments);
    assert!(
        output.status.success(),
        "control command {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn early_boot_enabled(cmdline: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bootart"))
        .arg("early-boot-enabled")
        .arg("--cmdline")
        .arg(cmdline)
        .output()
        .unwrap()
}

fn assert_early_boot_predicate(output: &Output, expected_code: i32) {
    assert_eq!(output.status.code(), Some(expected_code));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn hidden_early_boot_predicate_has_silent_fail_open_process_contract() {
    for (cmdline, expected_code) in [
        ("", 0),
        ("quiet bootart=0 root=/dev/vda", 1),
        ("quiet rd.bootart=0 root=/dev/vda", 1),
        ("quiet bootart=01 root=/dev/vda", 0),
    ] {
        let tree = TestTree::new(cmdline);
        let output = early_boot_enabled(&tree.cmdline);
        assert_early_boot_predicate(&output, expected_code);
        assert!(!tree.runtime.exists());
    }

    let tree = TestTree::new("");
    let missing_cmdline = tree.root.join("missing-cmdline");
    let output = early_boot_enabled(&missing_cmdline);
    assert_early_boot_predicate(&output, 1);
    assert!(!tree.runtime.exists());

    let unreadable_cmdline = tree.root.join("cmdline-directory");
    fs::create_dir(&unreadable_cmdline).unwrap();
    let output = early_boot_enabled(&unreadable_cmdline);
    assert_early_boot_predicate(&output, 1);
    assert!(!tree.runtime.exists());
}

#[test]
fn daemon_owns_state_rejects_duplicates_and_cleans_up() {
    let tree = TestTree::new("quiet");
    let mut daemon = DaemonChild::spawn(&tree);
    wait_for_socket(&tree, &mut daemon);

    assert_eq!(
        fs::metadata(&tree.runtime).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(tree.runtime.join("control.sock"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );

    let duplicate = Command::new(env!("CARGO_BIN_EXE_bootart"))
        .arg("daemon")
        .arg("--runtime-dir")
        .arg(&tree.runtime)
        .arg("--cmdline")
        .arg(&tree.cmdline)
        .arg("--test-buffer")
        .output()
        .unwrap();
    assert!(!duplicate.status.success());
    assert!(String::from_utf8_lossy(&duplicate.stderr).contains("daemon lock already exists"));

    control_success(&tree, &["ping"]);
    control_success(&tree, &["status", "Mounting filesystems"]);
    control_success(&tree, &["progress", "37"]);
    control_success(&tree, &["message", "Starting services"]);
    control_success(&tree, &["details", "show"]);
    control_success(&tree, &["update-root-fs", "/sysroot"]);

    let state = control_success(&tree, &["state", "--json"]);
    let json = String::from_utf8(state.stdout).unwrap();
    assert!(json.contains("\"lifecycle\":\"running\""));
    assert!(json.contains("\"view\":\"details\""));
    assert!(json.contains("\"root_stage\":\"real-root\""));
    assert!(json.contains("\"progress\":37"));
    assert!(!json.to_ascii_lowercase().contains("password"));

    control_success(&tree, &["quit"]);
    // The quit ACK is intentionally delayed until display restoration and
    // authenticated runtime release have completed.
    assert!(!tree.runtime.exists());
    assert!(daemon.wait().success());
    assert!(!tree.runtime.join("control.sock").exists());
    assert!(!tree.runtime.join("daemon.lock").exists());
    assert!(!tree.runtime.exists());
}

#[test]
fn disable_token_has_no_runtime_side_effects() {
    for token in ["bootart=0", "rd.bootart=0"] {
        let tree = TestTree::new(token);
        let status = DaemonChild::spawn(&tree).wait();
        assert!(status.success());
        assert!(!tree.runtime.exists());
    }
}

#[test]
fn sigterm_removes_socket_lock_and_owned_directory() {
    let tree = TestTree::new("quiet");
    let mut daemon = DaemonChild::spawn(&tree);
    wait_for_socket(&tree, &mut daemon);
    let child = daemon.0.as_mut().unwrap();
    // SAFETY: this PID is the child spawned immediately above.
    assert_eq!(
        unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) },
        0
    );
    let status = daemon.wait();
    assert!(status.success());
    assert!(!tree.runtime.exists());
}

#[test]
fn sigkill_stale_runtime_is_recovered_on_restart() {
    let tree = TestTree::new("quiet");
    let mut first = DaemonChild::spawn(&tree);
    wait_for_socket(&tree, &mut first);

    let mut child = first.0.take().unwrap();
    child.kill().unwrap();
    assert!(!child.wait().unwrap().success());
    assert!(tree.runtime.join("daemon.lock").exists());
    assert!(tree.runtime.join("control.sock").exists());
    let stale_inode = fs::symlink_metadata(tree.runtime.join("control.sock"))
        .unwrap()
        .ino();

    let mut restarted = DaemonChild::spawn(&tree);
    wait_for_replaced_socket(&tree, &mut restarted, stale_inode);
    control_success(&tree, &["ping"]);
    control_success(&tree, &["quit"]);
    assert!(restarted.wait().success());
    assert!(!tree.runtime.join("daemon.lock").exists());
    assert!(!tree.runtime.join("control.sock").exists());
}

#[test]
fn runtime_path_is_never_used_as_the_init_program() {
    let tree = TestTree::new("quiet");
    assert_ne!(tree.runtime, Path::new("/init"));
}

#[test]
fn partial_client_cannot_freeze_animation_or_control_dispatch() {
    let tree = TestTree::new("quiet");
    let mut daemon = DaemonChild::spawn(&tree);
    wait_for_socket(&tree, &mut daemon);

    let mut slow = UnixStream::connect(tree.runtime.join("control.sock")).unwrap();
    slow.write_all(b"B").unwrap();

    let started = Instant::now();
    control_success(&tree, &["progress", "51"]);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "a partial client blocked the command loop"
    );

    drop(slow);
    control_success(&tree, &["quit", "--retain-splash"]);
    assert!(!tree.runtime.exists());
    assert!(daemon.wait().success());
}
