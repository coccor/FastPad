#[cfg(windows)]
mod support;

#[cfg(windows)]
use std::time::Duration;

#[cfg(windows)]
use fastpad::editor::scintilla_constants::SCI_SETSAVEPOINT;
#[cfg(windows)]
use support::process::{FastPadProcess, wait_for_process_exit};
#[cfg(windows)]
use support::win32::{find_child_by_class, focused_window, scintilla_text, send_text};
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW;

#[cfg(windows)]
#[test]
fn launch_creates_a_focused_editable_scintilla() {
    // Break caught: bootstrap returns without creating a main window, so startup never exposes
    // a focused Scintilla editor that can accept real input.
    let mut process = FastPadProcess::spawn(["--diagnostic"]).unwrap();
    let hwnd = process
        .wait_for_main_window(Duration::from_secs(2))
        .unwrap();
    let editor = find_child_by_class(hwnd, "Scintilla").unwrap();
    assert_eq!(focused_window(hwnd).unwrap(), editor);
    send_text(editor, "x").unwrap();
    assert_eq!(scintilla_text(editor).unwrap(), "x");
    unsafe { SendMessageW(editor, SCI_SETSAVEPOINT, 0, 0) };
    process.close().unwrap();
}

#[cfg(windows)]
#[test]
fn dropping_fastpad_process_reaps_the_running_child() {
    // Break caught: a failed smoke assertion can orphan the spawned GUI process unless Drop
    // performs bounded cleanup.
    let process_id = {
        let mut process = FastPadProcess::spawn(["--diagnostic"]).unwrap();
        process
            .wait_for_main_window(Duration::from_secs(2))
            .unwrap();
        process.id()
    };

    wait_for_process_exit(process_id, Duration::from_secs(2)).unwrap();
}
