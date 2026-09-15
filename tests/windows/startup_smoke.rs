#[cfg(windows)]
mod support;

#[cfg(windows)]
use std::time::Duration;

#[cfg(windows)]
use fastpad::editor::scintilla_constants::{SCI_GETTABWIDTH, SCI_SETSAVEPOINT};
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
fn corrupt_settings_keep_input_live_apply_valid_keys_and_never_block() {
    // Break caught: a corrupt fastpad.ini delaying first input, discarding its valid keys, or
    // reporting problems with a modal dialog during the deferred startup chain.
    let local_app_data =
        std::env::temp_dir().join(format!("fastpad-smoke-settings-{}", std::process::id()));
    let settings_dir = local_app_data.join("FastPad");
    std::fs::create_dir_all(&settings_dir).unwrap();
    std::fs::write(
        settings_dir.join("fastpad.ini"),
        "tab_width=8\nfont_size=huge\nbogus=1\n",
    )
    .unwrap();

    let mut process =
        FastPadProcess::spawn_with_local_app_data(["--diagnostic"], &local_app_data).unwrap();
    let hwnd = process
        .wait_for_main_window(Duration::from_secs(2))
        .unwrap();
    let editor = find_child_by_class(hwnd, "Scintilla").unwrap();
    send_text(editor, "x").unwrap();
    assert_eq!(scintilla_text(editor).unwrap(), "x");

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while unsafe { SendMessageW(editor, SCI_GETTABWIDTH, 0, 0) } != 8 {
        assert!(
            std::time::Instant::now() < deadline,
            "the valid tab_width key was never applied"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!process.has_dialog().unwrap());

    unsafe { SendMessageW(editor, SCI_SETSAVEPOINT, 0, 0) };
    process.close().unwrap();
    let _ = std::fs::remove_dir_all(&local_app_data);
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
