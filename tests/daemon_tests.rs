use std::fs;
use std::io::Write;
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
            // SAFETY: the child PID belongs to this test and SIGTERM is the
            // daemon's supported cleanup path.
            unsafe {
                libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
            }
            let _ = child.wait();
        }
    }
}

fn wait_for_socket(tree: &TestTree) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if tree.runtime.join("control.sock").exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("daemon did not create its control socket");
}

fn wait_for_replaced_socket(tree: &TestTree, stale_inode: u64) {
    let socket = tree.runtime.join("control.sock");
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if fs::symlink_metadata(&socket)
            .map(|metadata| metadata.ino() != stale_inode)
            .unwrap_or(false)
        {
            return;
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

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn daemon_owns_state_rejects_duplicates_and_cleans_up() {
    let tree = TestTree::new("quiet");
    let daemon = DaemonChild::spawn(&tree);
    wait_for_socket(&tree);

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

    assert_success(&control(&tree, &["ping"]));
    assert_success(&control(&tree, &["status", "Mounting filesystems"]));
    assert_success(&control(&tree, &["progress", "37"]));
    assert_success(&control(&tree, &["message", "Starting services"]));
    assert_success(&control(&tree, &["details", "show"]));
    assert_success(&control(&tree, &["update-root-fs", "/sysroot"]));

    let state = control(&tree, &["state", "--json"]);
    assert_success(&state);
    let json = String::from_utf8(state.stdout).unwrap();
    assert!(json.contains("\"lifecycle\":\"running\""));
    assert!(json.contains("\"view\":\"details\""));
    assert!(json.contains("\"root_stage\":\"real-root\""));
    assert!(json.contains("\"progress\":37"));
    assert!(!json.to_ascii_lowercase().contains("password"));

    assert_success(&control(&tree, &["quit"]));
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
    wait_for_socket(&tree);
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
    wait_for_socket(&tree);

    let mut child = first.0.take().unwrap();
    child.kill().unwrap();
    assert!(!child.wait().unwrap().success());
    assert!(tree.runtime.join("daemon.lock").exists());
    assert!(tree.runtime.join("control.sock").exists());
    let stale_inode = fs::symlink_metadata(tree.runtime.join("control.sock"))
        .unwrap()
        .ino();

    let restarted = DaemonChild::spawn(&tree);
    wait_for_replaced_socket(&tree, stale_inode);
    assert_success(&control(&tree, &["ping"]));
    assert_success(&control(&tree, &["quit"]));
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
    let daemon = DaemonChild::spawn(&tree);
    wait_for_socket(&tree);

    let mut slow = UnixStream::connect(tree.runtime.join("control.sock")).unwrap();
    slow.write_all(b"B").unwrap();

    let started = Instant::now();
    assert_success(&control(&tree, &["progress", "51"]));
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "a partial client blocked the command loop"
    );

    drop(slow);
    assert_success(&control(&tree, &["quit", "--retain-splash"]));
    assert!(!tree.runtime.exists());
    assert!(daemon.wait().success());
}
