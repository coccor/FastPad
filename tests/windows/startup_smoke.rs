#[cfg(windows)]
mod support;

#[cfg(windows)]
use std::time::Duration;

#[cfg(windows)]
use support::process::FastPadProcess;
#[cfg(windows)]
use support::win32::{find_child_by_class, focused_window, scintilla_text, send_text};

#[cfg(windows)]
#[test]
fn launch_creates_a_focused_editable_scintilla() {
    // Break caught: bootstrap returns without creating a main window, so startup never exposes
    // a focused Scintilla editor that can accept real input.
    let process = FastPadProcess::spawn(["--diagnostic"]).unwrap();
    let hwnd = process
        .wait_for_main_window(Duration::from_secs(2))
        .unwrap();
    let editor = find_child_by_class(hwnd, "Scintilla").unwrap();
    assert_eq!(focused_window(hwnd).unwrap(), editor);
    send_text(editor, "x").unwrap();
    assert_eq!(scintilla_text(editor).unwrap(), "x");
    process.close().unwrap();
}
