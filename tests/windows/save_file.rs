#![cfg(windows)]
mod support;

// Build exact native sources with cfg(test); test seams remain absent from production.
include!("../../src/lib.rs");

use fastpad::editor::scintilla_constants::SCI_GETMODIFY;
use fastpad::window::commands::CommandId;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use support::win32::scintilla_text;
use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BM_CLICK, EnumWindows, GetClassNameW, GetDlgItem, GetWindowThreadProcessId, IDCANCEL, IsWindow,
    PostMessageW, SendMessageW, WM_CHAR, WM_COMMAND,
};

#[test]
fn plain_save_writes_atomically_and_clears_the_dirty_indicator() {
    // Break caught: Save can no-op, write a non-atomic partial file, or leave the dirty indicator
    // set after a successful write to an already-known path.
    let fixture = Fixture::new(b"before");
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    let editor = main.editor;
    window::open_path(main.hwnd, &fixture.path).unwrap();
    wait_text(editor, "before");
    type_text(editor, "X");
    let expected = scintilla_text(editor).unwrap();
    assert_ne!(expected, "before");

    unsafe {
        SendMessageW(main.hwnd, WM_COMMAND, CommandId::Save as usize, 0);
    }

    assert_eq!(std::fs::read(&fixture.path).unwrap(), expected.as_bytes());
    assert_eq!(unsafe { SendMessageW(editor, SCI_GETMODIFY, 0, 0) }, 0);
    main.with_app(|app| {
        assert!(!app.tabs.active().dirty);
        assert_eq!(
            app.tabs.active().path.as_deref(),
            Some(fixture.path.as_path())
        );
    });
}

#[test]
fn save_as_prompts_writes_the_new_path_and_updates_the_tab() {
    // Break caught: Save As on an untitled document does not prompt, does not write the file, or
    // leaves the tab pointed at the old (missing) path.
    //
    // Uses `window::save_path_as` directly rather than driving the real native dialog through to
    // a commit click: that specific interaction (choosing a filename and having the dialog
    // actually close) proved unreliable on this host for every filename tried, existing or not,
    // across several distinct synthetic-input techniques (see the long comment on
    // `save_as_onto_another_open_tabs_path_shows_an_error_and_does_not_overwrite`). Live dialog
    // *appearance* and *cancellation* remain covered by `save_as_cancellation_leaves_the_document_untouched`
    // and `save_command_with_no_path_behaves_like_save_as` below.
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    let editor = main.editor;
    type_text(editor, "hello");
    let scratch = ScratchDir::new("save-as-new");
    let target = scratch.path().join("brand-new.txt");

    window::save_path_as(main.hwnd, &target);

    assert_eq!(std::fs::read(&target).unwrap(), b"hello");
    assert_eq!(unsafe { SendMessageW(editor, SCI_GETMODIFY, 0, 0) }, 0);
    main.with_app(|app| {
        assert_eq!(app.tabs.active().path.as_deref(), Some(target.as_path()));
        assert!(!app.tabs.active().dirty);
    });
}

#[test]
fn save_command_with_no_path_behaves_like_save_as() {
    // Break caught: Save on an untitled (never-saved) document silently no-ops instead of
    // prompting for a location, per the decision that Save without a path behaves like Save As.
    // Proves delegation by driving the real dialog only as far as confirming it appears, then
    // cancelling it (reliable on this host, unlike committing a chosen filename - see the comment
    // on `save_as_prompts_writes_the_new_path_and_updates_the_tab` above).
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    let editor = main.editor;
    type_text(editor, "typed before any save");

    cancel_save_dialog(main.hwnd, CommandId::Save);

    assert_ne!(unsafe { SendMessageW(editor, SCI_GETMODIFY, 0, 0) }, 0);
    main.with_app(|app| {
        assert_eq!(app.tabs.active().path, None);
        assert!(app.tabs.active().dirty);
    });
}

#[test]
fn save_as_cancellation_leaves_the_document_untouched() {
    // Break caught: cancelling the Save As dialog still marks the document clean or clears its
    // (lack of) a path.
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    let editor = main.editor;
    type_text(editor, "unsaved");

    cancel_save_dialog(main.hwnd, CommandId::SaveAs);

    assert_ne!(unsafe { SendMessageW(editor, SCI_GETMODIFY, 0, 0) }, 0);
    main.with_app(|app| {
        assert_eq!(app.tabs.active().path, None);
        assert!(app.tabs.active().dirty);
    });
}

#[test]
fn save_as_onto_another_open_tabs_path_shows_an_error_and_does_not_overwrite() {
    // Break caught: Save As can silently create two tabs that own the same canonical path, or
    // overwrite another open tab's file on disk before (or instead of) rejecting the rename.
    //
    // This drives the real production collision-rejection path (`complete_save` via
    // `window::save_path_as`) directly with an explicit path rather than through the native Save
    // dialog: this host's Explorer-style Save dialog does not reliably respond to synthetic
    // window messages (BM_CLICK, direct WM_LBUTTONDOWN/UP, VK_RETURN) that attempt to commit a
    // chosen filename, whether or not that target already exists (no secondary confirmation
    // window ever appears; the dialog simply never closes, for reasons that could not be
    // root-caused). Live dialog appearance and cancellation are covered by
    // `save_as_cancellation_leaves_the_document_untouched` and
    // `save_command_with_no_path_behaves_like_save_as`; the collision rejection itself is also
    // covered at the `Tabs::set_active_path` unit level.
    //
    // The resulting error notification is a real, blocking MessageBoxW in production; driving
    // that deterministically turned out to be equally unreliable on this host (its native
    // creation was confirmed via tracing, but it was not reliably discoverable via EnumWindows
    // within several seconds). `show_save_error` therefore records the message under
    // `#[cfg(test)]` instead of showing it, exposed here via `window::take_save_errors`;
    // production still shows the real dialog unchanged.
    let fixture = Fixture::new(b"owned by other tab");
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    let editor = main.editor;
    window::open_path(main.hwnd, &fixture.path).unwrap();
    unsafe {
        SendMessageW(main.hwnd, WM_COMMAND, CommandId::New as usize, 0);
    }
    type_text(editor, "mine");
    main.with_app(|app| assert_eq!(app.tabs.len(), 2));

    window::save_path_as(main.hwnd, &fixture.path);

    let reported = notices(&main);
    assert_eq!(reported.len(), 1);
    assert!(reported[0].contains("already open"), "{reported:?}");
    assert_eq!(std::fs::read(&fixture.path).unwrap(), b"owned by other tab");
    main.with_app(|app| {
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.tabs.active().path, None);
        assert!(app.tabs.active().dirty);
    });
}

#[test]
fn save_as_failure_reverts_the_tabs_path_to_its_original_value() {
    // Break caught: Save As renames the tab's path before writing. If the write then fails, the
    // tab must not keep claiming the new, never-written path as its own -- it should still claim
    // whatever path it was last actually saved at, and the original file must be untouched.
    let fixture = Fixture::new(b"already saved");
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    let editor = main.editor;
    window::open_path(main.hwnd, &fixture.path).unwrap();
    wait_text(editor, "already saved");
    let scratch = ScratchDir::new("save-as-rename-failure");
    // The parent directory does not exist, so save_atomic's temp-file creation fails
    // deterministically without needing any file-locking trickery.
    let target = scratch.path().join("missing-subdir").join("out.txt");

    window::save_path_as(main.hwnd, &target);

    let reported = notices(&main);
    assert_eq!(reported.len(), 1);
    assert!(reported[0].contains("could not save"), "{reported:?}");
    assert!(!target.exists());
    main.with_app(|app| {
        assert_eq!(
            app.tabs.active().path.as_deref(),
            Some(fixture.path.as_path())
        );
    });
    assert_eq!(std::fs::read(&fixture.path).unwrap(), b"already saved");
}

#[test]
fn tab_close_yes_saves_a_pathed_document_then_closes_it() {
    // Break caught: answering Yes to the close prompt closes the tab without saving it, losing
    // every unsaved edit.
    let fixture = Fixture::new(b"before");
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    window::open_path(main.hwnd, &fixture.path).unwrap();
    wait_text(main.editor, "before");
    type_text(main.editor, "X");
    let expected = scintilla_text(main.editor).unwrap();
    window::answer_next_close_prompt(|_| document::CloseDecision::Save);

    unsafe {
        SendMessageW(main.hwnd, WM_COMMAND, CommandId::CloseTab as usize, 0);
    }

    assert_eq!(std::fs::read(&fixture.path).unwrap(), expected.as_bytes());
    assert_eq!(scintilla_text(main.editor).unwrap(), "");
    main.with_app(|app| {
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tabs.active().path, None);
        assert!(!app.tabs.active().dirty);
    });
}

#[test]
fn tab_close_yes_on_an_untitled_document_saves_as_the_chosen_path_then_closes_it() {
    // Break caught: Yes on a never-saved document closes it without asking where to save.
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    type_text(main.editor, "draft");
    let scratch = ScratchDir::new("close-yes-save-as");
    let target = scratch.path().join("draft.txt");
    let chosen = target.clone();
    window::answer_next_close_prompt(|_| document::CloseDecision::Save);
    window::answer_next_save_dialog(move |_| Some(chosen));

    unsafe {
        SendMessageW(main.hwnd, WM_COMMAND, CommandId::CloseTab as usize, 0);
    }

    assert_eq!(std::fs::read(&target).unwrap(), b"draft");
    assert_eq!(scintilla_text(main.editor).unwrap(), "");
    main.with_app(|app| {
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tabs.active().path, None);
    });
}

#[test]
fn tab_close_yes_on_an_untitled_document_keeps_it_open_when_save_as_is_cancelled() {
    // Break caught: cancelling the Save As dialog that a Yes answer opens still closes the tab.
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    type_text(main.editor, "draft");
    window::answer_next_close_prompt(|_| document::CloseDecision::Save);
    window::answer_next_save_dialog(|_| None);

    unsafe {
        SendMessageW(main.hwnd, WM_COMMAND, CommandId::CloseTab as usize, 0);
    }

    assert_eq!(scintilla_text(main.editor).unwrap(), "draft");
    main.with_app(|app| {
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tabs.active().path, None);
        assert!(app.tabs.active().dirty);
    });
}

#[test]
fn tab_close_yes_keeps_the_tab_open_when_the_write_fails() {
    // Break caught: a Yes answer whose save fails still closes the tab and discards its text.
    let fixture = Fixture::new(b"before");
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    window::open_path(main.hwnd, &fixture.path).unwrap();
    wait_text(main.editor, "before");
    type_text(main.editor, "X");
    let edited = scintilla_text(main.editor).unwrap();
    // Without its directory, save_atomic cannot create the temporary file.
    std::fs::remove_dir_all(&fixture.directory).unwrap();
    window::answer_next_close_prompt(|_| document::CloseDecision::Save);

    unsafe {
        SendMessageW(main.hwnd, WM_COMMAND, CommandId::CloseTab as usize, 0);
    }

    assert_eq!(scintilla_text(main.editor).unwrap(), edited);
    let reported = notices(&main);
    assert!(
        reported
            .iter()
            .any(|notice| notice.contains("could not save")),
        "{reported:?}"
    );
    main.with_app(|app| {
        assert_eq!(
            app.tabs.active().path.as_deref(),
            Some(fixture.path.as_path())
        );
        assert!(app.tabs.active().dirty);
    });
}

#[test]
fn tab_close_no_discards_and_cancel_keeps_the_document() {
    // Break caught: No leaving the tab open, or Cancel closing it.
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    type_text(main.editor, "kept");
    window::answer_next_close_prompt(|_| document::CloseDecision::Cancel);

    unsafe {
        SendMessageW(main.hwnd, WM_COMMAND, CommandId::CloseTab as usize, 0);
    }

    assert_eq!(scintilla_text(main.editor).unwrap(), "kept");
    main.with_app(|app| assert!(app.tabs.active().dirty));

    window::answer_next_close_prompt(|_| document::CloseDecision::Discard);
    unsafe {
        SendMessageW(main.hwnd, WM_COMMAND, CommandId::CloseTab as usize, 0);
    }

    assert_eq!(scintilla_text(main.editor).unwrap(), "");
    main.with_app(|app| {
        assert_eq!(app.tabs.len(), 1);
        assert!(!app.tabs.active().dirty);
    });
}

#[test]
fn window_close_yes_saves_every_dirty_document_before_the_window_closes() {
    // Break caught: window close treating Yes as "keep going" and destroying unsaved documents.
    let fixture = Fixture::new(b"before");
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    window::open_path(main.hwnd, &fixture.path).unwrap();
    wait_text(main.editor, "before");
    type_text(main.editor, "X");
    let first = scintilla_text(main.editor).unwrap();
    unsafe {
        SendMessageW(main.hwnd, WM_COMMAND, CommandId::New as usize, 0);
    }
    type_text(main.editor, "draft");
    let scratch = ScratchDir::new("window-close-yes");
    let target = scratch.path().join("draft.txt");
    let chosen = target.clone();
    window::answer_next_close_prompt(|_| document::CloseDecision::Save);
    window::answer_next_close_prompt(|_| document::CloseDecision::Save);
    window::answer_next_save_dialog(move |_| Some(chosen));

    unsafe {
        SendMessageW(
            main.hwnd,
            windows_sys::Win32::UI::WindowsAndMessaging::WM_CLOSE,
            0,
            0,
        );
    }

    assert_eq!(unsafe { IsWindow(main.hwnd) }, 0);
    assert_eq!(std::fs::read(&fixture.path).unwrap(), first.as_bytes());
    assert_eq!(std::fs::read(&target).unwrap(), b"draft");
}

#[test]
fn window_close_yes_with_save_as_cancelled_aborts_the_close() {
    // Break caught: a cancelled Save As during window close still destroys the unsaved document.
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    type_text(main.editor, "draft");
    window::answer_next_close_prompt(|_| document::CloseDecision::Save);
    window::answer_next_save_dialog(|_| None);

    unsafe {
        SendMessageW(
            main.hwnd,
            windows_sys::Win32::UI::WindowsAndMessaging::WM_CLOSE,
            0,
            0,
        );
    }

    assert_ne!(unsafe { IsWindow(main.hwnd) }, 0);
    assert_eq!(scintilla_text(main.editor).unwrap(), "draft");
    main.with_app(|app| assert!(app.tabs.active().dirty));
}

#[test]
fn window_close_cancel_aborts_and_no_discards_without_writing() {
    // Break caught: Cancel destroying the window, or No writing a file instead of discarding.
    let fixture = Fixture::new(b"before");
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    window::open_path(main.hwnd, &fixture.path).unwrap();
    wait_text(main.editor, "before");
    type_text(main.editor, "X");
    window::answer_next_close_prompt(|_| document::CloseDecision::Cancel);

    unsafe {
        SendMessageW(
            main.hwnd,
            windows_sys::Win32::UI::WindowsAndMessaging::WM_CLOSE,
            0,
            0,
        );
    }

    assert_ne!(unsafe { IsWindow(main.hwnd) }, 0);
    main.with_app(|app| assert!(app.tabs.active().dirty));

    window::answer_next_close_prompt(|_| document::CloseDecision::Discard);
    unsafe {
        SendMessageW(
            main.hwnd,
            windows_sys::Win32::UI::WindowsAndMessaging::WM_CLOSE,
            0,
            0,
        );
    }

    assert_eq!(unsafe { IsWindow(main.hwnd) }, 0);
    assert_eq!(std::fs::read(&fixture.path).unwrap(), b"before");
}

/// The non-modal notification messages currently queued on the window (spec 239).
fn notices(main: &TestMain) -> Vec<String> {
    main.with_app(|app| {
        app.notifications
            .pending()
            .iter()
            .map(|notice| notice.message.clone())
            .collect()
    })
}

/// Drives a Save/Save As command and immediately cancels the native dialog.
fn cancel_save_dialog(owner: HWND, command: CommandId) {
    let pid = std::process::id();
    let driver = std::thread::spawn(move || {
        let dialog = wait_dialog(pid);
        unsafe {
            assert_ne!(
                PostMessageW(GetDlgItem(dialog, IDCANCEL), BM_CLICK, 0, 0),
                0
            );
        }
        wait_dialog_closed(dialog);
    });
    unsafe {
        SendMessageW(owner, WM_COMMAND, command as usize, 0);
    }
    driver.join().unwrap();
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

fn wait_text(editor: HWND, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if scintilla_text(editor).unwrap() == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "expected editor text {expected:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Types `text` into `editor` by sending WM_CHAR directly (synchronously) rather than posting it,
/// since these in-process tests share a thread with `editor`'s own window procedure and run no
/// message loop of their own to dispatch a posted message.
fn type_text(editor: HWND, text: &str) {
    for unit in text.encode_utf16() {
        unsafe {
            SendMessageW(editor, WM_CHAR, unit as usize, 0);
        }
    }
}

fn wait_dialog_closed(dialog: HWND) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while unsafe { IsWindow(dialog) } != 0 {
        assert!(Instant::now() < deadline, "outgoing dialog remained live");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_dialog(pid: u32) -> HWND {
    struct Search {
        pid: u32,
        found: HWND,
    }
    unsafe extern "system" fn visit(hwnd: HWND, data: LPARAM) -> windows_sys::core::BOOL {
        let search = unsafe { &mut *(data as *mut Search) };
        let mut pid = 0;
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut pid);
        }
        let mut class = [0u16; 32];
        let len = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), 32) };
        if pid == search.pid
            && String::from_utf16_lossy(&class[..len as usize]) == "#32770"
            && unsafe { !GetDlgItem(hwnd, IDCANCEL).is_null() }
        {
            search.found = hwnd;
            return 0;
        }
        1
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let mut search = Search {
            pid,
            found: std::ptr::null_mut(),
        };
        unsafe {
            EnumWindows(Some(visit), &mut search as *mut Search as isize);
        }
        if !search.found.is_null() {
            return search.found;
        }
        assert!(
            Instant::now() < deadline,
            "Save command did not show a native dialog"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct Fixture {
    directory: PathBuf,
    path: PathBuf,
}
impl Fixture {
    fn new(bytes: &[u8]) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "fastpad-save-integration-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("document.txt");
        std::fs::write(&path, bytes).unwrap();
        Self { directory, path }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

struct ScratchDir(PathBuf);
impl ScratchDir {
    fn new(label: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "fastpad-save-integration-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&directory).unwrap();
        Self(directory)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
