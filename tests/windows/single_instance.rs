#![cfg(windows)]
// Requires that no other FastPad runs in this session: the default launch path claims its mutex.

mod support;

use fastpad::ipc::client::send_frame;
use fastpad::ipc::{InstanceNames, IpcRequest, encode_frame};
use std::error::Error;
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use support::process::{FastPadProcess, wait_for_process_exit};
use support::win32::{find_child_by_class, scintilla_text, send_text};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows_sys::Win32::System::Pipes::WaitNamedPipeW;
use windows_sys::Win32::UI::Accessibility::AccessibleObjectFromWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    IsIconic, OBJID_CLIENT, PostMessageW, SW_MINIMIZE, ShowWindow, WM_CHAR,
};
use windows_sys::core::{GUID, HRESULT};

type TestResult<T> = Result<T, Box<dyn Error>>;
static INSTANCE_TEST_LOCK: Mutex<()> = Mutex::new(());
const IID_IACCESSIBLE: GUID = GUID::from_u128(0x618736e0_3c3d_11cf_810c_00aa00389b71);
const WAIT: Duration = Duration::from_secs(5);

#[test]
fn second_launch_forwards_its_file_to_the_primary_and_exits_zero() -> TestResult<()> {
    // Break caught: a secondary that opens its own window, exits before writing, or makes the
    // primary add a duplicate tab for a path it already shows.
    let _serial = INSTANCE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let _com = ComApartment::initialize()?;
    let scratch = Scratch::new("forward")?;
    let file = scratch.file("forwarded.txt", "forwarded text")?;
    let primary = Primary::start(&scratch)?;
    let tabs = tab_count(primary.hwnd)?;

    for _ in 0..2 {
        let mut secondary = FastPadProcess::spawn_with_local_app_data([&file], &scratch.root)?;
        wait_for_process_exit(secondary.id(), WAIT)?;
        assert!(secondary.wait_for_main_window(Duration::ZERO).is_err());
        assert_eq!(
            secondary.exit_code()?,
            Some(0),
            "a secondary that forwarded its file must exit zero"
        );
        secondary.close()?;
        wait_until("the forwarded tab", || {
            tab_count(primary.hwnd).is_ok_and(|count| count == tabs + 1)
                && scintilla_text(primary.editor).is_ok_and(|text| text == "forwarded text")
        })?;
    }
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(tab_count(primary.hwnd)?, tabs + 1);
    Ok(())
}

#[test]
fn raw_activate_frame_restores_the_primary_without_changing_tabs() -> TestResult<()> {
    // Break caught: Activate decoded but ignored, or treated as New.
    let _serial = INSTANCE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let _com = ComApartment::initialize()?;
    let scratch = Scratch::new("activate")?;
    let primary = Primary::start(&scratch)?;
    let tabs = tab_count(primary.hwnd)?;
    unsafe { ShowWindow(primary.hwnd, SW_MINIMIZE) };
    wait_until("the primary to minimize", || unsafe {
        IsIconic(primary.hwnd) != 0
    })?;

    send_frame(&names()?, &encode_frame(&IpcRequest::Activate)?, WAIT)?;

    wait_until("the primary to restore", || unsafe {
        IsIconic(primary.hwnd) == 0
    })?;
    assert_eq!(tab_count(primary.hwnd)?, tabs);
    Ok(())
}

#[test]
fn secondary_racing_server_start_is_never_lost_and_never_hangs() -> TestResult<()> {
    // Break caught: a secondary that exits zero while the primary has no listener, silently
    // dropping the request, or waits without a bound for a pipe that never appears.
    let _serial = INSTANCE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let _com = ComApartment::initialize()?;
    let scratch = Scratch::new("race")?;
    let file = scratch.file("raced.txt", "raced text")?;
    let mut process = FastPadProcess::spawn_with_local_app_data([] as [&str; 0], &scratch.root)?;
    let hwnd = process.wait_for_main_window(WAIT)?;
    let editor = find_child_by_class(hwnd, "Scintilla")?;
    let tabs = tab_count(hwnd)?;

    unsafe { PostMessageW(editor, WM_CHAR, usize::from(b'x'), 0) };
    let started = Instant::now();
    let mut secondary = FastPadProcess::spawn_with_local_app_data([&file], &scratch.root)?;
    match secondary.wait_for_main_window(WAIT) {
        Ok(_) => {
            // Fell back to an independent editor: the primary must not also have received it.
            std::thread::sleep(Duration::from_millis(300));
            assert_eq!(tab_count(hwnd)?, tabs);
        }
        Err(_) => {
            wait_for_process_exit(secondary.id(), WAIT)?;
            secondary.close()?;
            wait_until("the raced tab in the primary", || {
                tab_count(hwnd).is_ok_and(|count| count == tabs + 1)
                    && scintilla_text(editor).is_ok_and(|text| text == "raced text")
            })?;
        }
    }
    assert!(started.elapsed() < Duration::from_secs(10));
    drop(process);
    Ok(())
}

#[test]
fn malformed_frames_change_nothing_and_the_server_keeps_accepting() -> TestResult<()> {
    // Break caught: an oversized or unknown frame adding a tab or wedging the only pipe instance.
    let _serial = INSTANCE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let _com = ComApartment::initialize()?;
    let scratch = Scratch::new("malformed")?;
    let primary = Primary::start(&scratch)?;
    let tabs = tab_count(primary.hwnd)?;
    let names = names()?;

    let _ = send_frame(&names, &vec![0x41; 70_000], WAIT);
    send_frame(&names, b"FPI1\x09\0\0\0\0", WAIT)?;
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(tab_count(primary.hwnd)?, tabs);

    send_frame(&names, &encode_frame(&IpcRequest::New)?, WAIT)?;
    wait_until("the New tab", || {
        tab_count(primary.hwnd).is_ok_and(|count| count == tabs + 1)
    })?;
    Ok(())
}

#[test]
fn new_window_flag_starts_a_distinct_process_while_a_primary_runs() -> TestResult<()> {
    // Break caught: --new-window still checking the mutex and forwarding into the primary.
    let _serial = INSTANCE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let _com = ComApartment::initialize()?;
    let scratch = Scratch::new("new-window")?;
    let primary = Primary::start(&scratch)?;
    let tabs = tab_count(primary.hwnd)?;

    let mut other = FastPadProcess::spawn_with_local_app_data(["--new-window"], &scratch.root)?;
    let other_hwnd = other.wait_for_main_window(WAIT)?;
    assert_ne!(other.id(), primary.process.id());
    assert_ne!(other_hwnd, primary.hwnd);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(tab_count(primary.hwnd)?, tabs);
    other.close()
}

/// A default-path FastPad past first input whose pipe server is listening.
struct Primary {
    process: FastPadProcess,
    hwnd: HWND,
    editor: HWND,
}

impl Primary {
    fn start(scratch: &Scratch) -> TestResult<Self> {
        let mut process =
            FastPadProcess::spawn_with_local_app_data([] as [&str; 0], &scratch.root)?;
        let hwnd = process.wait_for_main_window(WAIT)?;
        let editor = find_child_by_class(hwnd, "Scintilla")?;
        send_text(editor, "x")?;
        let pipe = names()?.pipe;
        wait_until("the primary pipe listener", || unsafe {
            WaitNamedPipeW(pipe.as_ptr(), 50) != 0
        })?;
        Ok(Self {
            process,
            hwnd,
            editor,
        })
    }
}

fn names() -> TestResult<InstanceNames> {
    Ok(InstanceNames::for_current_session()?)
}

fn wait_until(what: &str, mut condition: impl FnMut() -> bool) -> TestResult<()> {
    let deadline = Instant::now() + WAIT;
    while !condition() {
        if Instant::now() >= deadline {
            return Err(format!("timed out waiting for {what}").into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

/// Client-area accessible children are the fixed chrome items plus one per tab.
fn tab_count(hwnd: HWND) -> TestResult<i32> {
    Accessible::from_window(hwnd)?.child_count()
}

struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> TestResult<Self> {
        let root = std::env::temp_dir().join(format!(
            "fastpad-single-instance-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("FastPad"))?;
        Ok(Self { root })
    }

    fn file(&self, name: &str, text: &str) -> TestResult<PathBuf> {
        let path = self.root.join(name);
        std::fs::write(&path, text)?;
        Ok(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

struct ComApartment;

impl ComApartment {
    fn initialize() -> TestResult<Self> {
        let status = unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };
        if status < 0 {
            Err(format!("CoInitializeEx failed: {status:#x}").into())
        } else {
            Ok(Self)
        }
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

#[repr(C)]
struct AccessibleVtable {
    query_interface: usize,
    add_ref: usize,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    get_type_info_count: usize,
    get_type_info: usize,
    get_ids_of_names: usize,
    invoke: usize,
    get_acc_parent: usize,
    get_acc_child_count: unsafe extern "system" fn(*mut c_void, *mut i32) -> HRESULT,
}

struct Accessible(*mut c_void);

impl Accessible {
    fn from_window(hwnd: HWND) -> TestResult<Self> {
        let mut object = std::ptr::null_mut();
        let status = unsafe {
            AccessibleObjectFromWindow(hwnd, OBJID_CLIENT as u32, &IID_IACCESSIBLE, &mut object)
        };
        if status < 0 || object.is_null() {
            Err(format!("AccessibleObjectFromWindow failed: {status:#x}").into())
        } else {
            Ok(Self(object))
        }
    }

    fn vtable(&self) -> &AccessibleVtable {
        unsafe { &**(self.0 as *const *const AccessibleVtable) }
    }

    fn child_count(&self) -> TestResult<i32> {
        let mut count = 0;
        let status = unsafe { (self.vtable().get_acc_child_count)(self.0, &mut count) };
        if status < 0 {
            Err(format!("get_accChildCount failed: {status:#x}").into())
        } else {
            Ok(count)
        }
    }
}

impl Drop for Accessible {
    fn drop(&mut self) {
        unsafe { (self.vtable().release)(self.0) };
    }
}
