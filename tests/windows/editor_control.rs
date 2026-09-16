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

#[cfg(windows)]
#[test]
fn line_number_margin_is_sized_for_the_active_documents_line_count() {
    // Break caught: a real Scintilla measuring zero width for STYLE_LINENUMBER, or switching to a
    // long document keeping the short document's two-digit gutter.
    use fastpad::editor::scintilla_constants::SCI_GETMARGINWIDTHN;
    use windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW;

    let harness = WindowHarness::new().unwrap();
    let editor = Editor::create(harness.hwnd()).unwrap();
    let width = || unsafe { SendMessageW(editor.hwnd(), SCI_GETMARGINWIDTHN, 0, 0) };
    let short = editor.current_document().unwrap();
    let short_width = width();
    assert!(short_width > 0);

    let long = editor.create_document().unwrap();
    editor.use_document(&long).unwrap();
    editor.set_text(&"line\n".repeat(12_000)).unwrap();
    let long_width = width();
    assert!(long_width > short_width, "{long_width} <= {short_width}");

    editor.use_document(&short).unwrap();
    assert_eq!(width(), short_width);
    editor.use_document(&long).unwrap();
    assert_eq!(width(), long_width);
}
