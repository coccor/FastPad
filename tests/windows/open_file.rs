#![cfg(windows)]
mod support;

use fastpad::editor::scintilla_constants::{SCI_GETMODIFY, SCI_SETSAVEPOINT};
use fastpad::window::commands::CommandId;
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use support::process::FastPadProcess;
use support::win32::{find_child_by_class, scintilla_text, send_text};
use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BM_CLICK, EnumWindows, GetClassNameW, GetDlgItem, GetWindowThreadProcessId, IDCANCEL,
    PostMessageW, SendMessageW, WM_COMMAND,
};

#[test]
fn launch_json_waits_for_input_then_loads_clean_text() {
    // Break caught: launch request is ignored, or file bytes populate before the first input.
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
        PostMessageW(
            editor,
            windows_sys::Win32::UI::WindowsAndMessaging::WM_CHAR,
            b'x' as usize,
            0,
        );
    }
    wait_text(editor, "{\"ok\":true}");
    assert_eq!(unsafe { SendMessageW(editor, SCI_GETMODIFY, 0, 0) }, 0);
    // First accepted text has its own untitled tab; clean it before orderly teardown.
    unsafe {
        SendMessageW(hwnd, WM_COMMAND, CommandId::CloseTab as usize, 0);
        SendMessageW(editor, SCI_SETSAVEPOINT, 0, 0);
    }
    process.close().unwrap();
}

#[test]
fn native_open_command_cancellation_preserves_dirty_document() {
    // Break caught: Open does not show IFileOpenDialog, or cancellation loses active text.
    let mut process = FastPadProcess::spawn(["--new-window"]).unwrap();
    let hwnd = process
        .wait_for_main_window(Duration::from_secs(3))
        .unwrap();
    let editor = find_child_by_class(hwnd, "Scintilla").unwrap();
    send_text(editor, "unsaved").unwrap();
    unsafe {
        PostMessageW(hwnd, WM_COMMAND, CommandId::Open as usize, 0);
    }
    let dialog = wait_dialog(process.id());
    unsafe {
        SendMessageW(GetDlgItem(dialog, IDCANCEL), BM_CLICK, 0, 0);
    }
    wait_dialog_closed(dialog);
    wait_text(editor, "unsaved");
    unsafe {
        SendMessageW(editor, SCI_SETSAVEPOINT, 0, 0);
    }
    process.close().unwrap();
}

#[test]
fn selected_canonical_duplicate_reuses_the_native_document() {
    // Break caught: the shell path is not loaded, or a canonical alias creates a second document.
    let fixture = Fixture::new(b"{\"ok\":true}");
    let mut process = FastPadProcess::spawn(["--new-window"]).unwrap();
    let hwnd = process
        .wait_for_main_window(Duration::from_secs(3))
        .unwrap();
    let editor = find_child_by_class(hwnd, "Scintilla").unwrap();
    send_text(editor, "original").unwrap();
    unsafe {
        SendMessageW(editor, SCI_SETSAVEPOINT, 0, 0);
    }
    select_file(hwnd, process.id(), &fixture.path);
    eprintln!("selected first file");
    assert_process_live(process.id(), hwnd, editor);
    wait_text(editor, "{\"ok\":true}");
    let document = unsafe {
        SendMessageW(
            editor,
            fastpad::editor::scintilla_constants::SCI_GETDOCPOINTER,
            0,
            0,
        )
    };
    select_file(
        hwnd,
        process.id(),
        &fixture.path.parent().unwrap().join(".").join("config.json"),
    );
    eprintln!("selected duplicate file");
    assert_eq!(
        unsafe {
            SendMessageW(
                editor,
                fastpad::editor::scintilla_constants::SCI_GETDOCPOINTER,
                0,
                0,
            )
        },
        document
    );
    assert_eq!(unsafe { SendMessageW(editor, SCI_GETMODIFY, 0, 0) }, 0);
    unsafe {
        SendMessageW(hwnd, WM_COMMAND, CommandId::CloseTab as usize, 0);
    }
    wait_text(editor, "original");
    process.close().unwrap();
}

fn select_file(owner: HWND, pid: u32, path: &std::path::Path) {
    use std::os::windows::ffi::OsStrExt;
    unsafe {
        PostMessageW(owner, WM_COMMAND, CommandId::Open as usize, 0);
    }
    let dialog = wait_dialog(pid);
    let filename = unsafe { GetDlgItem(dialog, 1148) };
    assert!(
        !filename.is_null(),
        "native dialog has no file-name control"
    );
    let edit = find_child_by_class(filename, "Edit").unwrap();
    eprintln!("located filename edit");
    eprintln!("initial filename: {:?}", scintilla_text(edit));
    let wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
    unsafe {
        assert_ne!(
            windows_sys::Win32::UI::WindowsAndMessaging::SendMessageTimeoutW(
                dialog,
                windows_sys::Win32::UI::WindowsAndMessaging::WM_NEXTDLGCTL,
                edit as usize,
                1,
                windows_sys::Win32::UI::WindowsAndMessaging::SMTO_ABORTIFHUNG,
                3000,
                std::ptr::null_mut()
            ),
            0
        );
        assert_ne!(
            windows_sys::Win32::UI::WindowsAndMessaging::SendMessageTimeoutW(
                edit,
                0x00b1,
                0,
                -1,
                windows_sys::Win32::UI::WindowsAndMessaging::SMTO_ABORTIFHUNG,
                3000,
                std::ptr::null_mut()
            ),
            0
        );
        for unit in wide {
            assert_ne!(
                PostMessageW(
                    edit,
                    windows_sys::Win32::UI::WindowsAndMessaging::WM_CHAR,
                    unit as usize,
                    0
                ),
                0
            );
        }
    }
    wait_text(edit, &path.to_string_lossy());
    eprintln!("open button enabled: {}", unsafe {
        windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(GetDlgItem(
            dialog,
            windows_sys::Win32::UI::WindowsAndMessaging::IDOK,
        ))
    });
    unsafe {
        eprintln!("set filename");
        PostMessageW(
            GetDlgItem(dialog, windows_sys::Win32::UI::WindowsAndMessaging::IDOK),
            BM_CLICK,
            0,
            0,
        );
    }
    wait_dialog_closed(dialog);
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

fn assert_process_live(pid: u32, main: HWND, editor: HWND) {
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    let handle = unsafe { fastpad::platform::OwnedHandle::from_raw_owned(raw) }.unwrap();
    let mut status = 0;
    assert_ne!(
        unsafe { GetExitCodeProcess(handle.as_raw(), &mut status) },
        0
    );
    assert_eq!(
        status, 259,
        "FastPad exited unexpectedly with code {status:#x}"
    );
    assert_ne!(
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsWindow(main) },
        0,
        "main window destroyed unexpectedly"
    );
    assert_ne!(
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsWindow(editor) },
        0,
        "editor window destroyed unexpectedly"
    );
}

fn wait_dialog_closed(dialog: HWND) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsWindow(dialog) } != 0 {
        if Instant::now() >= deadline {
            let mut pid = 0;
            unsafe {
                GetWindowThreadProcessId(dialog, &mut pid);
            }
            dump_windows(pid);
            panic!("outgoing Open dialog remained live");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn dump_windows(pid: u32) {
    unsafe extern "system" fn visit(hwnd: HWND, data: LPARAM) -> windows_sys::core::BOOL {
        let pid = data as u32;
        let mut owner = 0;
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut owner);
        }
        if owner == pid {
            let mut class = [0u16; 128];
            let len = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), 128) };
            eprintln!(
                "window class {:?}, text {:?}",
                String::from_utf16_lossy(&class[..len as usize]),
                scintilla_text(hwnd)
            );
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::EnumChildWindows(hwnd, Some(child), 0);
            }
        }
        1
    }
    unsafe extern "system" fn child(hwnd: HWND, _: LPARAM) -> windows_sys::core::BOOL {
        let mut class = [0u16; 128];
        let len = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), 128) };
        let name = String::from_utf16_lossy(&class[..len as usize]);
        if name == "Static" || name == "Edit" || name == "Button" {
            eprintln!("child {:?} {:?}", name, scintilla_text(hwnd));
        }
        1
    }
    unsafe {
        EnumWindows(Some(visit), pid as isize);
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
