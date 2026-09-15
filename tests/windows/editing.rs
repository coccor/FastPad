#![cfg(windows)]
mod support;

// Build exact native sources with cfg(test); test seams remain absent from production.
include!("../../src/lib.rs");

use fastpad::window::commands::CommandId;
use std::mem::size_of;
use std::time::{Duration, Instant};
use support::win32::scintilla_text;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
    VIRTUAL_KEY, VK_ESCAPE, VK_RETURN, VK_SHIFT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GUITHREADINFO, GetGUIThreadInfo, SendMessageW, WM_CHAR, WM_COMMAND, WM_KEYDOWN,
};

#[test]
fn undo_and_redo_route_through_the_shared_command_model() {
    // Break caught: Ctrl+Z/Ctrl+Y (routed as CommandId::Undo/Redo) not actually calling into
    // Scintilla's own undo stack.
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    let editor = main.editor;
    type_text(editor, "hello");
    assert_eq!(scintilla_text(editor).unwrap(), "hello");

    unsafe {
        SendMessageW(main.hwnd, WM_COMMAND, CommandId::Undo as usize, 0);
    }
    wait_text(editor, "");

    unsafe {
        SendMessageW(main.hwnd, WM_COMMAND, CommandId::Redo as usize, 0);
    }
    wait_text(editor, "hello");
}

#[test]
fn cut_and_paste_route_through_the_shared_command_model() {
    // Break caught: CommandId::Cut/Paste (menu-driven, no accelerator) not reaching Scintilla.
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    let editor = main.editor;
    type_text(editor, "clip me");
    select_all_in_editor(editor);

    unsafe {
        SendMessageW(main.hwnd, WM_COMMAND, CommandId::Cut as usize, 0);
    }
    wait_text(editor, "");

    unsafe {
        SendMessageW(main.hwnd, WM_COMMAND, CommandId::Paste as usize, 0);
    }
    wait_text(editor, "clip me");
}

#[test]
fn ctrl_f_opens_find_bar_prefills_from_selection_and_escape_returns_focus_to_editor() {
    // Break caught: the bar not appearing, not prefilling from a single-line selection, or Escape
    // leaving focus stranded in the (now hidden) query field instead of the editor.
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    let editor = main.editor;
    type_text(editor, "needle in a haystack");
    select_range(editor, 0, 6); // "needle"

    unsafe {
        SendMessageW(main.hwnd, WM_COMMAND, CommandId::Find as usize, 0);
    }

    let query_hwnd = main.with_app(|app| app.find_bar.as_ref().unwrap().query_hwnd());
    assert!(main.with_app(|app| app.find_bar.as_ref().unwrap().is_visible()));
    wait_text(query_hwnd, "needle");
    wait_focus(query_hwnd);

    send_key(query_hwnd, VK_ESCAPE, false);

    assert!(!main.with_app(|app| app.find_bar.as_ref().unwrap().is_visible()));
    wait_focus(editor);
}

#[test]
fn enter_in_find_field_cycles_forward_through_every_match() {
    // Break caught: navigation not honoring the wrap-once-then-stop contract end-to-end through
    // the real bar and a real Scintilla document. Each press starts a fresh search anchored at
    // the current selection, so repeated presses cycle through every match indefinitely (the
    // second press exercises an in-press wrap: nothing left after the caret, so it wraps to the
    // first match); the wrap-once contract bounds each individual press, not the whole cycle.
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    let editor = main.editor;
    type_text(editor, "one two one");
    select_range(editor, 8, 8); // caret right before the second "one"

    unsafe {
        SendMessageW(main.hwnd, WM_COMMAND, CommandId::Find as usize, 0);
    }
    let query_hwnd = main.with_app(|app| app.find_bar.as_ref().unwrap().query_hwnd());
    type_text(query_hwnd, "one");

    send_key(query_hwnd, VK_RETURN, false);
    assert_eq!(selection(editor), (8, 11));

    send_key(query_hwnd, VK_RETURN, false);
    assert_eq!(selection(editor), (0, 3));

    // Back around to the second match, proving this cycles rather than stopping after one pass.
    send_key(query_hwnd, VK_RETURN, false);
    assert_eq!(selection(editor), (8, 11));
}

#[test]
fn enter_in_find_field_with_no_match_leaves_the_selection_unchanged() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    let editor = main.editor;
    type_text(editor, "one two one");
    select_range(editor, 4, 7); // "two"

    unsafe {
        SendMessageW(main.hwnd, WM_COMMAND, CommandId::Find as usize, 0);
    }
    let query_hwnd = main.with_app(|app| app.find_bar.as_ref().unwrap().query_hwnd());
    type_text(query_hwnd, "missing");

    send_key(query_hwnd, VK_RETURN, false);

    assert_eq!(selection(editor), (4, 7));
}

#[test]
fn shift_enter_in_find_field_navigates_backward() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    let editor = main.editor;
    type_text(editor, "one two one");
    select_range(editor, 3, 3); // caret right after the first "one"

    unsafe {
        SendMessageW(main.hwnd, WM_COMMAND, CommandId::Find as usize, 0);
    }
    let query_hwnd = main.with_app(|app| app.find_bar.as_ref().unwrap().query_hwnd());
    type_text(query_hwnd, "one");

    send_key(query_hwnd, VK_RETURN, true);
    assert_eq!(selection(editor), (0, 3));

    send_key(query_hwnd, VK_RETURN, true);
    assert_eq!(selection(editor), (8, 11));
}

#[test]
fn replace_all_replaces_every_match_and_is_undone_in_one_step() {
    // Break caught: Replace All grouping every individual replacement into its own undo action
    // instead of exactly one, forcing repeated Ctrl+Z presses to fully undo it.
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    let editor = main.editor;
    type_text(editor, "cat cat cat");

    unsafe {
        SendMessageW(main.hwnd, WM_COMMAND, CommandId::Replace as usize, 0);
    }
    let query_hwnd = main.with_app(|app| app.find_bar.as_ref().unwrap().query_hwnd());
    let replace_hwnd = main.with_app(|app| app.find_bar.as_ref().unwrap().replace_hwnd());
    type_text(query_hwnd, "cat");
    type_text(replace_hwnd, "dog");

    send_key(replace_hwnd, VK_RETURN, true);
    wait_text(editor, "dog dog dog");

    unsafe {
        SendMessageW(main.hwnd, WM_COMMAND, CommandId::Undo as usize, 0);
    }
    wait_text(editor, "cat cat cat");
}

#[test]
fn enter_in_replace_field_replaces_the_current_match_and_advances() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    let editor = main.editor;
    type_text(editor, "cat cat");
    select_range(editor, 0, 3); // select the first "cat" exactly

    unsafe {
        SendMessageW(main.hwnd, WM_COMMAND, CommandId::Replace as usize, 0);
    }
    let query_hwnd = main.with_app(|app| app.find_bar.as_ref().unwrap().query_hwnd());
    let replace_hwnd = main.with_app(|app| app.find_bar.as_ref().unwrap().replace_hwnd());
    type_text(query_hwnd, "cat");
    type_text(replace_hwnd, "dog");

    send_key(replace_hwnd, VK_RETURN, false);

    wait_text(editor, "dog cat");
    // Advanced to (and selected) the remaining match.
    assert_eq!(selection(editor), (4, 7));
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
    fn with_app<R>(&self, run: impl FnOnce(&app::App) -> R) -> R {
        assert!(self.identity.is_live_for(self.hwnd));
        let raw = unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(
                self.hwnd,
                windows_sys::Win32::UI::WindowsAndMessaging::GWLP_USERDATA,
            )
        } as *const app::App;
        assert!(!raw.is_null());
        run(unsafe { &*raw })
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
/// since these in-process tests share a thread with the target window's own procedure and run no
/// message loop of their own to dispatch a posted message. Works for both the Scintilla editor
/// and the find bar's plain Edit controls.
fn type_text(hwnd: HWND, text: &str) {
    for unit in text.encode_utf16() {
        unsafe {
            SendMessageW(hwnd, WM_CHAR, unit as usize, 0);
        }
    }
}

/// Sends a synchronous WM_KEYDOWN, optionally with Shift physically held for the duration of the
/// call via `SendInput` (the find bar's field subclass reads real-time keyboard state through
/// `GetAsyncKeyState`, which queries hardware state directly rather than a message-time table, so
/// this works even though these in-process tests run no message loop of their own).
fn send_key(hwnd: HWND, key: u16, shift: bool) {
    if shift {
        send_input(VK_SHIFT, 0);
    }
    unsafe {
        SendMessageW(hwnd, WM_KEYDOWN, key as usize, 0);
    }
    if shift {
        send_input(VK_SHIFT, KEYEVENTF_KEYUP);
    }
}

fn send_input(key: VIRTUAL_KEY, flags: KEYBD_EVENT_FLAGS) {
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe {
        SendInput(1, &input, size_of::<INPUT>() as i32);
    }
}

fn select_all_in_editor(editor: HWND) {
    let len = scintilla_text(editor).unwrap().len();
    select_range(editor, 0, len);
}

fn select_range(editor: HWND, start: usize, end: usize) {
    unsafe {
        SendMessageW(
            editor,
            editor::scintilla_constants::SCI_SETSEL,
            start,
            end as isize,
        );
    }
}

fn selection(editor: HWND) -> (usize, usize) {
    unsafe {
        let start = SendMessageW(
            editor,
            editor::scintilla_constants::SCI_GETSELECTIONSTART,
            0,
            0,
        );
        let end = SendMessageW(
            editor,
            editor::scintilla_constants::SCI_GETSELECTIONEND,
            0,
            0,
        );
        (start as usize, end as usize)
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

fn wait_focus(hwnd: HWND) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if current_focus() == Some(hwnd) {
            return;
        }
        assert!(Instant::now() < deadline, "expected focus on {hwnd:?}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn current_focus() -> Option<HWND> {
    let mut info = GUITHREADINFO {
        cbSize: size_of::<GUITHREADINFO>() as u32,
        ..unsafe { std::mem::zeroed() }
    };
    // Thread ID 0 asks for the calling thread's own focus info, which is all these
    // single-threaded, in-process tests need.
    let ok = unsafe { GetGUIThreadInfo(0, &mut info) };
    if ok == 0 {
        return None;
    }
    (!info.hwndFocus.is_null()).then_some(info.hwndFocus)
}
