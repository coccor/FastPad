#![cfg(windows)]
mod support;

// Build exact native sources with cfg(test); test seams remain absent from production. This also
// means `show_json_issue`/`show_json_valid` compile to their `#[cfg(test)]` thread-local-recording
// variants here, so a real (blocking, modal) MessageBoxW is never shown by this target.
include!("../../src/lib.rs");

use fastpad::window::commands::CommandId;
use std::time::{Duration, Instant};
use support::win32::scintilla_text;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_CHAR, WM_COMMAND};

#[test]
fn format_json_reformats_with_two_spaces_and_is_undone_in_one_step() {
    // Break caught: Ctrl+Shift+F's Format JSON command not reaching the editor, not using
    // serde_json's two-space pretty printer, or splitting its rewrite into more than one undo
    // action (which would force repeated Ctrl+Z to fully restore the original bytes).
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    let editor = main.editor;
    type_text(editor, "{\"a\":[1,2]}");

    unsafe {
        SendMessageW(main.hwnd, WM_COMMAND, CommandId::FormatJson as usize, 0);
    }

    wait_text(editor, "{\n  \"a\": [\n    1,\n    2\n  ]\n}");
    assert!(window::take_json_issues().is_empty());

    unsafe {
        SendMessageW(main.hwnd, WM_COMMAND, CommandId::Undo as usize, 0);
    }
    wait_text(editor, "{\"a\":[1,2]}");
}

#[test]
fn format_json_on_invalid_json_leaves_the_document_unchanged_and_reports_an_issue() {
    // Break caught: Format JSON mutating the buffer (or starting an undo action) before
    // discovering the source does not parse, instead of leaving the bytes and undo stack exactly
    // as they were and surfacing the failure.
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    let editor = main.editor;
    type_text(editor, "{ bad");

    unsafe {
        SendMessageW(main.hwnd, WM_COMMAND, CommandId::FormatJson as usize, 0);
    }

    assert_eq!(scintilla_text(editor).unwrap(), "{ bad");
    let issues = window::take_json_issues();
    assert_eq!(issues.len(), 1);
    assert!(issues[0].contains("line 1"), "{}", issues[0]);
    // Whether Format JSON itself ever starts an undo action for invalid input (it must not) is
    // covered precisely at the unit level in `window::main_window::tests`, which starts from a
    // guaranteed-empty undo stack (`Editor::populate_clean`); this end-to-end test's own typed
    // text already carries its own (unrelated) typing undo entry, so asserting on `can_undo` here
    // would conflate the two.
}

#[test]
fn validate_json_reports_success_for_valid_json_and_never_mutates_the_document() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    let editor = main.editor;
    type_text(editor, "{\"a\":1}");

    unsafe {
        SendMessageW(main.hwnd, WM_COMMAND, CommandId::ValidateJson as usize, 0);
    }

    assert_eq!(window::take_json_valid_count(), 1);
    assert!(window::take_json_issues().is_empty());
    assert_eq!(scintilla_text(editor).unwrap(), "{\"a\":1}");
}

#[test]
fn validate_json_reports_the_line_and_column_for_invalid_json() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    let editor = main.editor;
    type_text(editor, "{\n  bad\n}");

    unsafe {
        SendMessageW(main.hwnd, WM_COMMAND, CommandId::ValidateJson as usize, 0);
    }

    assert_eq!(window::take_json_valid_count(), 0);
    let issues = window::take_json_issues();
    assert_eq!(issues.len(), 1);
    assert!(
        issues[0].contains("line 2") && issues[0].contains("column 3"),
        "{}",
        issues[0]
    );
    assert_eq!(scintilla_text(editor).unwrap(), "{\n  bad\n}");
}

struct TestMain {
    hwnd: HWND,
    editor: HWND,
    identity: app::WindowIdentity,
    _class: window::MainWindowClass,
}
impl TestMain {
    fn new() -> Self {
        let app = app::App::new(
            launch::LaunchOptions::default(),
            perf::StartupMetrics::begin().unwrap(),
        );
        let identity = app.window_identity();
        let instance = unsafe {
            windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null())
        };
        let class = window::MainWindowClass::register(instance).unwrap();
        let mut context = window::WindowCreateContext::new(Box::new(app));
        let hwnd = class.create(&mut context).unwrap();
        let editor = unsafe {
            window::initialize_editor_with(hwnd, &identity, editor::Editor::create).unwrap()
        };
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::ShowWindow(
                hwnd,
                windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOW,
            );
        }
        Self {
            hwnd,
            editor,
            identity,
            _class: class,
        }
    }
}
impl Drop for TestMain {
    fn drop(&mut self) {
        if self.identity.is_live_for(self.hwnd) {
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(self.hwnd);
            }
        }
    }
}

/// Types `text` into `hwnd` by sending WM_CHAR directly (synchronously) rather than posting it,
/// matching `editing.rs`'s own helper: these in-process tests share a thread with the target
/// window's own procedure and run no message loop of their own to dispatch a posted message.
fn type_text(hwnd: HWND, text: &str) {
    for unit in text.encode_utf16() {
        unsafe {
            SendMessageW(hwnd, WM_CHAR, unit as usize, 0);
        }
    }
}

fn wait_text(hwnd: HWND, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if scintilla_text(hwnd).unwrap() == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "expected control text {expected:?}, last saw {:?}",
            scintilla_text(hwnd).unwrap()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
