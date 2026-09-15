#![cfg(windows)]
mod support;

use fastpad::document::RecoveryId;
use fastpad::editor::scintilla_constants::SCI_SETSAVEPOINT;
use fastpad::file::encoding::Encoding;
use fastpad::recovery::{Snapshot, write_snapshot};
use fastpad::window::commands::CommandId;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use support::process::{FastPadProcess, wait_and_dismiss_dialog};
use support::win32::{find_child_by_class, scintilla_text, send_text};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, SendMessageW, WM_COMMAND};

const RECOVERED_TEXT: &str = "text from a crashed session";

#[test]
fn valid_snapshots_reopen_after_launch_and_malformed_ones_are_quarantined_without_a_dialog() {
    // Break caught: discovery never running, opening recovered text into the wrong tab, deleting
    // (instead of quarantining) malformed data, or blocking startup with a modal dialog.
    let data = LocalAppData::new("restore");
    let source = data.place_snapshot();
    let malformed = data.recovery().join("ffffffffffffffffffffffffffffffff.fps");
    std::fs::write(&malformed, b"FPS1\x01torn").unwrap();

    let (process, _hwnd, editor) = launch_until_recovered(&data);

    wait_until("malformed snapshot quarantine", || {
        !malformed.exists() && quarantined(&malformed).exists()
    });
    assert_eq!(
        std::fs::read(quarantined(&malformed)).unwrap(),
        b"FPS1\x01torn"
    );
    assert!(source.exists(), "an unsaved recovered tab keeps its source");
    assert!(!process.has_dialog().unwrap());

    unsafe { SendMessageW(editor, SCI_SETSAVEPOINT, 0, 0) };
    process.close().unwrap();
}

#[test]
fn discarding_a_recovered_tab_removes_its_source_snapshot() {
    // Break caught: a discarded recovered document resurrecting on every later launch.
    let data = LocalAppData::new("discard");
    let source = data.place_snapshot();
    let (process, hwnd, editor) = launch_until_recovered(&data);

    unsafe {
        PostMessageW(hwnd, WM_COMMAND, CommandId::CloseTab as usize, 0);
    }
    wait_and_dismiss_dialog(process.id(), Duration::from_secs(3)).unwrap();

    wait_until("source snapshot removal", || !source.exists());
    wait_until("the remaining untitled tab", || {
        scintilla_text(editor).is_ok_and(|text| text.is_empty())
    });
    process.close().unwrap();
}

#[test]
fn no_snapshot_exists_at_launch_and_idle_edits_are_snapshotted_until_clean_exit() {
    // Break caught: a recovery timer running during bootstrap, never writing edited documents,
    // or leaving snapshots behind after a clean exit.
    let data = LocalAppData::new("timer");
    std::fs::write(
        data.root().join("FastPad").join("fastpad.ini"),
        "recovery_interval_seconds=1\n",
    )
    .unwrap();
    let mut process =
        FastPadProcess::spawn_with_local_app_data([] as [&str; 0], data.root()).unwrap();
    let hwnd = process
        .wait_for_main_window(Duration::from_secs(2))
        .unwrap();
    let editor = find_child_by_class(hwnd, "Scintilla").unwrap();

    let quiet_until = Instant::now() + Duration::from_millis(1_500);
    while Instant::now() < quiet_until {
        assert!(
            snapshot_files(&data.recovery()).is_empty(),
            "snapshot written at launch"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    send_text(editor, "hello").unwrap();
    wait_until_for(
        "an idle snapshot of the edit",
        Duration::from_secs(15),
        || {
            snapshot_files(&data.recovery()).iter().any(|file| {
                std::fs::read(file)
                    .is_ok_and(|bytes| bytes.windows(5).any(|window| window == b"hello"))
            })
        },
    );

    unsafe { SendMessageW(editor, SCI_SETSAVEPOINT, 0, 0) };
    process.close().unwrap();
    assert!(snapshot_files(&data.recovery()).is_empty());
}

fn launch_until_recovered(data: &LocalAppData) -> (FastPadProcess, HWND, HWND) {
    let mut process =
        FastPadProcess::spawn_with_local_app_data([] as [&str; 0], data.root()).unwrap();
    let hwnd = process
        .wait_for_main_window(Duration::from_secs(2))
        .unwrap();
    let editor = find_child_by_class(hwnd, "Scintilla").unwrap();
    wait_until("the recovered tab's text", || {
        scintilla_text(editor).is_ok_and(|text| text == RECOVERED_TEXT)
    });
    (process, hwnd, editor)
}

fn wait_until(what: &str, condition: impl FnMut() -> bool) {
    wait_until_for(what, Duration::from_secs(5), condition);
}

fn wait_until_for(what: &str, timeout: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn quarantined(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".invalid");
    PathBuf::from(name)
}

fn snapshot_files(directory: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(directory)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|extension| extension == "fps"))
                .collect()
        })
        .unwrap_or_default()
}

struct LocalAppData(PathBuf);

impl LocalAppData {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "fastpad-recovery-it-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("FastPad").join("Recovery")).unwrap();
        Self(root)
    }

    fn root(&self) -> &Path {
        &self.0
    }

    fn recovery(&self) -> PathBuf {
        self.0.join("FastPad").join("Recovery")
    }

    fn place_snapshot(&self) -> PathBuf {
        let snapshot = Snapshot::new(
            RecoveryId::from_u128(0x0123_4567_89ab_cdef_0000_0001_0000_0001),
            Some(PathBuf::from(r"C:\docs\notes.txt")),
            Encoding::Utf8,
            RECOVERED_TEXT,
        );
        write_snapshot(&self.recovery(), &snapshot).unwrap()
    }
}

impl Drop for LocalAppData {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
