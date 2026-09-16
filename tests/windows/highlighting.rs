#[cfg(windows)]
mod support;

#[cfg(windows)]
use std::path::PathBuf;
#[cfg(windows)]
use std::time::Duration;

#[cfg(windows)]
use fastpad::editor::scintilla_constants::SCI_SETSAVEPOINT;
#[cfg(windows)]
use support::process::{FastPadProcess, process_has_module_loaded};
#[cfg(windows)]
use support::win32::{Deadline, find_child_by_class, scintilla_text};
#[cfg(windows)]
use windows_sys::Win32::Foundation::HWND;
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, SendMessageW, WM_CHAR};

#[cfg(windows)]
const LEXILLA_MODULE_NAME: &str = "Lexilla.dll";

/// A plain-text-only launch (no file argument, so the initial document stays `Language::PlainText`
/// and nothing ever detects a language for it) must never load `Lexilla.dll` into the process.
/// `Lexilla.dll` is deliberately left present on disk next to the real `FastPad.exe` for this test
/// (the deployed layout): the assertion is about deferred *loading*, not file absence.
#[cfg(windows)]
#[test]
fn empty_launch_never_loads_lexilla() {
    // Break caught: loading Lexilla eagerly during startup instead of deferring it to first
    // JSON/Markdown activation defeats Task 13's whole purpose for the common plain-text case.
    ensure_lexilla_present_next_to_fastpad_exe();
    let mut process = FastPadProcess::spawn(["--new-window"]).unwrap();
    process
        .wait_for_main_window(Duration::from_secs(2))
        .unwrap();

    // Give the deferred startup chain (which WM_FASTPAD_APPLY_LANGUAGE is part of) time to settle.
    std::thread::sleep(Duration::from_millis(300));
    assert!(!process_has_module_loaded(process.id(), LEXILLA_MODULE_NAME).unwrap());

    process.close().unwrap();
}

/// Opening a `.json` file drives real, automatic language detection (no explicit menu command):
/// `Lexilla.dll` becomes loaded, and the editor remains fully usable afterward.
#[cfg(windows)]
#[test]
fn opening_a_json_file_loads_lexilla_and_the_editor_stays_editable() {
    ensure_lexilla_present_next_to_fastpad_exe();
    let fixture = JsonFixture::new();
    let mut process = FastPadProcess::spawn([
        std::ffi::OsStr::new("--new-window"),
        fixture.path.as_os_str(),
    ])
    .unwrap();
    let hwnd = process
        .wait_for_main_window(Duration::from_secs(2))
        .unwrap();
    let editor = find_child_by_class(hwnd, "Scintilla").unwrap();

    // The launch file opens on its own once the window is ready; no keystroke unblocks it.
    wait_for_module_loaded(process.id(), LEXILLA_MODULE_NAME);

    // Still editable after the lexer switch: Lexilla is only ever handed an opaque pointer via
    // SCI_SETILEXER, never anything that could disable the control.
    type_char(editor, b'y');
    wait_until(
        || {
            scintilla_text(editor)
                .map(|text| text.contains("ok") && text.contains('y'))
                .unwrap_or(false)
        },
        &Deadline::after(Duration::from_secs(3)),
        "expected the opened JSON content and the later keystroke to both be present",
    );

    // The launch file is the only tab, so clearing the verification keystroke's dirty flag leaves
    // nothing for the shared close helper to prompt about.
    unsafe {
        SendMessageW(editor, SCI_SETSAVEPOINT, 0, 0);
    }
    process.close().unwrap();
}

/// A missing `Lexilla.dll` must not crash, hang, or half-apply a lexer: opening a `.json` file
/// leaves the document editable as plain text, exactly as if it had never detected a language.
#[cfg(windows)]
#[test]
fn missing_lexilla_leaves_the_document_editable_as_plain_text() {
    ensure_lexilla_absent_next_to_fastpad_exe();
    let fixture = JsonFixture::new();
    let mut process = FastPadProcess::spawn([
        std::ffi::OsStr::new("--new-window"),
        fixture.path.as_os_str(),
    ])
    .unwrap();
    let hwnd = process
        .wait_for_main_window(Duration::from_secs(2))
        .unwrap();
    let editor = find_child_by_class(hwnd, "Scintilla").unwrap();

    wait_until(
        || {
            scintilla_text(editor)
                .map(|text| text.contains("ok"))
                .unwrap_or(false)
        },
        &Deadline::after(Duration::from_secs(3)),
        "expected the JSON file to finish opening even without Lexilla.dll present",
    );

    // The failed activation is reported in the persistent in-window notification line (spec 239),
    // so it never blocks the deferred startup chain or the editor with a modal dialog.
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !process.has_dialog().unwrap(),
        "a failed language activation showed a modal dialog"
    );
    assert!(!process_has_module_loaded(process.id(), LEXILLA_MODULE_NAME).unwrap());

    // Plain text editing still works after the failed activation.
    type_char(editor, b'z');
    wait_until(
        || {
            scintilla_text(editor)
                .map(|text| text.contains('z'))
                .unwrap_or(false)
        },
        &Deadline::after(Duration::from_secs(3)),
        "expected the editor to remain editable after a failed Lexilla activation",
    );

    unsafe {
        SendMessageW(editor, SCI_SETSAVEPOINT, 0, 0);
    }
    process.close().unwrap();
}

#[cfg(windows)]
fn type_char(hwnd: HWND, byte: u8) {
    unsafe {
        PostMessageW(hwnd, WM_CHAR, byte as usize, 0);
    }
}

#[cfg(windows)]
fn wait_for_module_loaded(process_id: u32, module_file_name: &str) {
    wait_until(
        || process_has_module_loaded(process_id, module_file_name).unwrap_or(false),
        &Deadline::after(Duration::from_secs(3)),
        &format!("timed out waiting for {module_file_name} to load"),
    );
}

#[cfg(windows)]
fn wait_until(mut predicate: impl FnMut() -> bool, deadline: &Deadline, message: &str) {
    loop {
        if predicate() {
            return;
        }
        assert!(!deadline.expired(), "{message}");
        deadline.sleep_step();
    }
}

#[cfg(windows)]
fn lexilla_dll_path_next_to_fastpad_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fastpad")).with_file_name("Lexilla.dll")
}

#[cfg(windows)]
fn native_lexilla_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("native")
        .join("out")
        .join("x64")
        .join("Lexilla.dll")
}

/// Copies the real built `Lexilla.dll` next to the `FastPad.exe` this test spawns, matching the
/// portable-ZIP deployment layout `LanguageManager`'s `current_exe()`-relative resolution expects.
/// A no-op if it is already there.
#[cfg(windows)]
fn ensure_lexilla_present_next_to_fastpad_exe() {
    let target = lexilla_dll_path_next_to_fastpad_exe();
    if !target.exists() {
        std::fs::copy(native_lexilla_path(), &target).unwrap();
    }
}

/// Removes `Lexilla.dll` from next to the spawned `FastPad.exe`, simulating a broken/incomplete
/// install so `LanguageManager::apply`'s load-failure path can be exercised for real.
#[cfg(windows)]
fn ensure_lexilla_absent_next_to_fastpad_exe() {
    let _ = std::fs::remove_file(lexilla_dll_path_next_to_fastpad_exe());
}

#[cfg(windows)]
struct JsonFixture {
    directory: PathBuf,
    path: PathBuf,
}

#[cfg(windows)]
impl JsonFixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "fastpad-highlighting-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("sample.json");
        std::fs::write(&path, b"{\"ok\": true}").unwrap();
        Self { directory, path }
    }
}

#[cfg(windows)]
impl Drop for JsonFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
