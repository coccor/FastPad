#[cfg(windows)]
mod support;

#[cfg(windows)]
use fastpad::editor::Editor;
#[cfg(windows)]
use support::win32::WindowHarness;

#[cfg(windows)]
#[test]
fn editor_owns_text_and_switches_native_documents() {
    // Break caught: returning a borrowed/current document handle or copying text between
    // documents instead of relying on Scintilla-owned buffers when switching native documents.
    let harness = WindowHarness::new().unwrap();
    let editor = Editor::create(harness.hwnd()).unwrap();
    editor.set_text("first").unwrap();
    let first = editor.current_document().unwrap();
    let second = editor.create_document().unwrap();
    editor.use_document(&second).unwrap();
    editor.set_text("second").unwrap();
    editor.use_document(&first).unwrap();
    harness.pump_messages().unwrap();
    assert_eq!(editor.text().unwrap(), "first");
}
