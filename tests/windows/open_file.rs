#![cfg(windows)]
mod support;

// Build exact native sources with cfg(test); test seams remain absent from production.
include!("../../src/lib.rs");

use fastpad::editor::scintilla_constants::{SCI_GETMODIFY, SCI_SETSAVEPOINT};
use fastpad::window::commands::CommandId;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use support::process::FastPadProcess;
use support::win32::{find_child_by_class, scintilla_text, send_text};
use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BM_CLICK, EnumWindows, GetClassNameW, GetDlgItem, GetWindowThreadProcessId, IDCANCEL, IDOK,
    PostMessageW, SendMessageW, WM_CHAR, WM_COMMAND,
};

#[test]
fn launch_json_waits_for_input_then_loads_clean_text() {
    // Break caught: ignored launch paths, or population before the first editor input.
    let fixture = Fixture::new(b"\xEF\xBB\xBF{\"ok\":true}");
    let mut process = FastPadProcess::spawn([
        OsString::from("--new-window"),
        fixture.path.clone().into_os_string(),
    ])
    .unwrap();
    let hwnd = process
        .wait_for_main_window(Duration::from_secs(3))
        .unwrap();
    let editor = find_child_by_class(hwnd, "Scintilla").unwrap();
    assert_eq!(scintilla_text(editor).unwrap(), "");
    unsafe {
        assert_ne!(PostMessageW(editor, WM_CHAR, b'x' as usize, 0), 0);
    }
    wait_text(editor, "{\"ok\":true}");
    assert_eq!(unsafe { SendMessageW(editor, SCI_GETMODIFY, 0, 0) }, 0);
    unsafe {
        SendMessageW(hwnd, WM_COMMAND, CommandId::CloseTab as usize, 0);
        SendMessageW(editor, SCI_SETSAVEPOINT, 0, 0);
    }
    process.close().unwrap();
}

#[test]
fn native_open_command_cancellation_preserves_dirty_document() {
    // Break caught: Open is not wired, or cancellation modifies the prior document.
    let mut process = FastPadProcess::spawn(["--new-window"]).unwrap();
    let hwnd = process
        .wait_for_main_window(Duration::from_secs(3))
        .unwrap();
    let editor = find_child_by_class(hwnd, "Scintilla").unwrap();
    send_text(editor, "unsaved").unwrap();
    unsafe {
        assert_ne!(
            PostMessageW(hwnd, WM_COMMAND, CommandId::Open as usize, 0),
            0
        );
    }
    let dialog = wait_dialog(process.id());
    unsafe {
        assert_ne!(
            PostMessageW(GetDlgItem(dialog, IDCANCEL), BM_CLICK, 0, 0),
            0
        );
    }
    wait_dialog_closed(dialog);
    wait_text(editor, "unsaved");
    assert_ne!(unsafe { SendMessageW(editor, SCI_GETMODIFY, 0, 0) }, 0);
    unsafe {
        SendMessageW(editor, SCI_SETSAVEPOINT, 0, 0);
    }
    process.close().unwrap();
}

#[test]
fn selected_canonical_duplicate_reuses_the_native_document() {
    // Break caught: shell results do not load, or canonical aliases establish competing owners.
    let fixture = Fixture::new(b"\xEF\xBB\xBF{\"ok\":true}");
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    let editor = main.editor;
    for unit in "original".encode_utf16() {
        unsafe {
            SendMessageW(editor, WM_CHAR, unit as usize, 0);
        }
    }
    unsafe {
        SendMessageW(editor, SCI_SETSAVEPOINT, 0, 0);
    }
    select_file(main.hwnd, &fixture.path);
    use platform::dialogs::DialogEvent::*;
    assert_eq!(
        platform::dialogs::take_dialog_events(),
        [
            ComInitialized,
            ShowReturned,
            ResultRetrieved,
            DisplayNameRetrieved,
            PathFreed,
            InterfaceReleased,
            InterfaceReleased,
            ComUninitialized
        ]
    );
    wait_text(editor, "{\"ok\":true}");
    let document =
        unsafe { SendMessageW(editor, editor::scintilla_constants::SCI_GETDOCPOINTER, 0, 0) };
    main.with_app(|app| {
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(
            app.tabs.active().path.as_deref(),
            Some(fixture.path.as_path())
        );
        assert_eq!(
            app.tabs.active().encoding,
            file::encoding::Encoding::Utf8Bom
        );
        assert!(!app.tabs.active().dirty);
        assert!(
            app.startup
                .micros(perf::Milestone::FirstInputAccepted)
                .unwrap()
                < app.startup.micros(perf::Milestone::FileLoaded).unwrap()
        );
    });
    let alternate = fixture.path.parent().unwrap().join(".").join("config.json");
    select_file(main.hwnd, &alternate);
    assert_eq!(
        platform::dialogs::take_dialog_events(),
        [
            ComInitialized,
            ShowReturned,
            ResultRetrieved,
            DisplayNameRetrieved,
            PathFreed,
            InterfaceReleased,
            InterfaceReleased,
            ComUninitialized
        ]
    );
    assert_eq!(
        unsafe { SendMessageW(editor, editor::scintilla_constants::SCI_GETDOCPOINTER, 0, 0) },
        document
    );
    main.with_app(|app| assert_eq!(app.tabs.len(), 2));
    assert_eq!(unsafe { SendMessageW(editor, SCI_GETMODIFY, 0, 0) }, 0);
    unsafe {
        SendMessageW(main.hwnd, WM_COMMAND, CommandId::CloseTab as usize, 0);
    }
    wait_text(editor, "original");
    unsafe {
        SendMessageW(
            main.hwnd,
            windows_sys::Win32::UI::WindowsAndMessaging::WM_CLOSE,
            0,
            0,
        );
    }
}

fn select_file(owner: HWND, path: &Path) {
    platform::dialogs::set_next_open_dialog_filename(path.to_path_buf());
    let expected = path.to_string_lossy().into_owned();
    let basename = path.file_name().unwrap().to_string_lossy().into_owned();
    let hidden_extension = path.with_extension("").to_string_lossy().into_owned();
    let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
    let driver = std::thread::spawn(move || {
        let dialog = wait_dialog(std::process::id());
        let _cancel = DriverCancel(dialog);
        // Wait for the preconfigured filename to be visibly installed before activating Open.
        let filename = unsafe { GetDlgItem(dialog, 1148) };
        let edit = find_child_by_class(filename, "Edit").unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let current = scintilla_text(edit).unwrap();
            if current == expected
                || current == basename
                || current == hidden_extension
                || current == stem
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "preconfigured filename was {current:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        unsafe {
            assert_ne!(PostMessageW(GetDlgItem(dialog, IDOK), BM_CLICK, 0, 0), 0);
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsWindow(dialog) } != 0 {
            if Instant::now() >= deadline {
                unsafe {
                    PostMessageW(GetDlgItem(dialog, IDCANCEL), BM_CLICK, 0, 0);
                }
                return Err("preconfigured selection did not close the native dialog");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    });
    // Same-thread WM_COMMAND enters real Show/GetResult/GetDisplayName and all RAII drops.
    unsafe {
        SendMessageW(owner, WM_COMMAND, CommandId::Open as usize, 0);
    }
    driver.join().unwrap().unwrap();
}

struct DriverCancel(HWND);
impl Drop for DriverCancel {
    fn drop(&mut self) {
        if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsWindow(self.0) } != 0 {
            unsafe {
                PostMessageW(GetDlgItem(self.0, IDCANCEL), BM_CLICK, 0, 0);
            }
        }
    }
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
fn wait_dialog_closed(dialog: HWND) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsWindow(dialog) } != 0 {
        assert!(
            Instant::now() < deadline,
            "outgoing Open dialog remained live"
        );
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
            "Open command did not show a native dialog"
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
            "fastpad-open-integration-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("config.json");
        std::fs::write(&path, bytes).unwrap();
        Self { directory, path }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
