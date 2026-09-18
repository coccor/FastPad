#![cfg(windows)]
// Requires that no other FastPad runs in this session: only the primary instance keeps a session.

mod support;

use fastpad::window::commands::CommandId;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use support::process::{FastPadProcess, wait_and_dismiss_dialog, wait_for_process_exit};
use support::win32::{Deadline, find_child_by_class, scintilla_text, send_text};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE, WM_COMMAND};

static SESSION_TEST_LOCK: Mutex<()> = Mutex::new(());
const WAIT: Duration = Duration::from_secs(5);

#[test]
fn a_closed_session_reopens_its_tabs_and_unsaved_text_on_the_next_launch() {
    // Break caught: a session close that still prompts, a manifest never written, a restore that
    // loses the unsaved tab or the active tab, or crash recovery adding a duplicate tab.
    let _serial = SESSION_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let data = Scratch::new("reopen");
    let file = data.file("notes.txt", "saved text");

    let mut first = FastPadProcess::spawn_with_local_app_data([&file], &data.root).unwrap();
    let hwnd = first
        .wait_for_main_window(WAIT)
        .expect("no main window: is another FastPad running in this session?");
    let editor = find_child_by_class(hwnd, "Scintilla").unwrap();
    wait_for_text(editor, "saved text");
    command(hwnd, CommandId::New);
    wait_for_text(editor, "");
    send_text(editor, "unsaved words").unwrap();
    wait_for_text(editor, "unsaved words");
    unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
    wait_for_process_exit(first.id(), WAIT)
        .expect("closing with session restore on must not prompt");
    first.close().unwrap();
    assert!(data.session().exists());

    let mut second =
        FastPadProcess::spawn_with_local_app_data(std::iter::empty::<&str>(), &data.root).unwrap();
    let hwnd = second.wait_for_main_window(WAIT).unwrap();
    let editor = find_child_by_class(hwnd, "Scintilla").unwrap();
    wait_for_text(editor, "unsaved words");
    command(hwnd, CommandId::SelectTab1);
    wait_for_text(editor, "saved text");
    // Give the recovery unit time to run; a duplicate would show up as a third tab.
    std::thread::sleep(Duration::from_millis(500));
    command(hwnd, CommandId::SelectTab3);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        scintilla_text(editor).unwrap(),
        "saved text",
        "a third tab exists"
    );
    assert!(
        !data.session().exists(),
        "the restore consumes the manifest"
    );

    unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
    wait_for_process_exit(second.id(), WAIT).unwrap();
    second.close().unwrap();
}

#[test]
fn with_session_restore_off_closing_asks_and_nothing_is_kept() {
    // Break caught: the setting being ignored, so FastPad silently keeps text the user expects
    // to be asked about.
    let _serial = SESSION_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let data = Scratch::new("off");
    std::fs::write(
        data.root.join("FastPad").join("fastpad.ini"),
        "restore_session=false\n",
    )
    .unwrap();

    let mut process =
        FastPadProcess::spawn_with_local_app_data(std::iter::empty::<&str>(), &data.root).unwrap();
    let hwnd = process
        .wait_for_main_window(WAIT)
        .expect("no main window: is another FastPad running in this session?");
    let editor = find_child_by_class(hwnd, "Scintilla").unwrap();
    send_text(editor, "throwaway").unwrap();
    wait_for_text(editor, "throwaway");
    unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
    wait_and_dismiss_dialog(process.id(), WAIT)
        .expect("closing with session restore off must prompt");
    wait_for_process_exit(process.id(), WAIT).unwrap();
    process.close().unwrap();
    assert!(!data.session().exists());
}

struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("fastpad-session-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("FastPad")).unwrap();
        Self { root }
    }

    fn file(&self, name: &str, text: &str) -> PathBuf {
        let path = self.root.join(name);
        std::fs::write(&path, text).unwrap();
        path
    }

    fn session(&self) -> PathBuf {
        self.root.join("FastPad").join("session.ini")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn command(hwnd: HWND, command: CommandId) {
    unsafe {
        PostMessageW(hwnd, WM_COMMAND, command as usize, 0);
    }
}

fn wait_for_text(editor: HWND, expected: &str) {
    let deadline = Deadline::after(WAIT);
    while !scintilla_text(editor).is_ok_and(|text| text == expected) {
        assert!(
            !deadline.expired(),
            "timed out waiting for editor text {expected:?}"
        );
        deadline.sleep_step();
    }
}
