#![cfg(windows)]

mod support;

use fastpad::editor::scintilla_constants::SCI_SETSAVEPOINT;
use fastpad::platform::wide_null;
use fastpad::window::commands::CommandId;
use fastpad::window::titlebar::{Size, TitleBarLayout};
use std::error::Error;
use std::ffi::c_void;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use support::process::FastPadProcess;
use support::win32::{find_child_by_class, scintilla_text, send_text};
use windows_sys::Win32::Foundation::{E_INVALIDARG, HWND, LPARAM, SysFreeString, SysStringLen};
use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows_sys::Win32::System::Variant::{VARIANT, VT_I4};
use windows_sys::Win32::UI::Accessibility::{AccessibleObjectFromWindow, SELFLAG_TAKESELECTION};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BM_CLICK, EnumWindows, GetClientRect, GetDlgItem, GetWindowThreadProcessId, IDCANCEL, IDNO,
    IsWindow, OBJID_CLIENT, PostMessageW, SendMessageW, WM_CHAR, WM_CLOSE, WM_COMMAND,
    WM_LBUTTONUP,
};
use windows_sys::core::{BOOL, BSTR, GUID, HRESULT};

type TestResult<T> = Result<T, Box<dyn Error>>;
static NATIVE_TEST_LOCK: Mutex<()> = Mutex::new(());
const IID_IACCESSIBLE: GUID = GUID::from_u128(0x618736e0_3c3d_11cf_810c_00aa00389b71);

#[test]
fn native_tabs_preserve_text_and_clean_close_switches_documents() -> TestResult<()> {
    let _serial = NATIVE_TEST_LOCK.lock().unwrap();
    let mut process = FastPadProcess::spawn(["--new-window"])?;
    let hwnd = process.wait_for_main_window(Duration::from_secs(3))?;
    let editor = find_child_by_class(hwnd, "Scintilla")?;

    send_text(editor, "first")?;
    unsafe { SendMessageW(editor, SCI_SETSAVEPOINT, 0, 0) };
    click_new_tab(hwnd, 1)?;
    wait_for_editor_text(editor, "", Duration::from_secs(2))?;
    send_text(editor, "second")?;
    unsafe { SendMessageW(editor, SCI_SETSAVEPOINT, 0, 0) };

    click_tab(hwnd, 0, 2)?;
    wait_for_editor_text(editor, "first", Duration::from_secs(2))?;
    unsafe { SendMessageW(hwnd, WM_COMMAND, CommandId::CloseTab as usize, 0) };
    wait_for_editor_text(editor, "second", Duration::from_secs(2))?;

    process.close()
}

#[test]
fn accessibility_selection_switches_the_native_editor_document() -> TestResult<()> {
    let _serial = NATIVE_TEST_LOCK.lock().unwrap();
    let _com = ComApartment::initialize()?;
    let mut process = FastPadProcess::spawn(["--new-window"])?;
    let hwnd = process.wait_for_main_window(Duration::from_secs(3))?;
    let editor = find_child_by_class(hwnd, "Scintilla")?;

    send_text(editor, "first")?;
    unsafe { SendMessageW(editor, SCI_SETSAVEPOINT, 0, 0) };
    unsafe { SendMessageW(hwnd, WM_COMMAND, CommandId::New as usize, 0) };
    wait_for_editor_text(editor, "", Duration::from_secs(2))?;
    send_text(editor, "second")?;
    unsafe { SendMessageW(editor, SCI_SETSAVEPOINT, 0, 0) };

    let accessible = Accessible::from_window(hwnd)?;
    assert_eq!(accessible.select(1), windows_sys::Win32::Foundation::S_OK);
    wait_for_editor_text(editor, "first", Duration::from_secs(2))?;

    process.close()
}

#[test]
fn retained_accessibility_provider_tracks_current_tabs_and_rejects_removed_tab() -> TestResult<()> {
    let _serial = NATIVE_TEST_LOCK.lock().unwrap();
    let _com = ComApartment::initialize()?;
    let mut process = FastPadProcess::spawn(["--new-window"])?;
    let hwnd = process.wait_for_main_window(Duration::from_secs(3))?;
    let editor = find_child_by_class(hwnd, "Scintilla")?;
    send_text(editor, "first")?;
    unsafe { SendMessageW(editor, SCI_SETSAVEPOINT, 0, 0) };
    let accessible = Accessible::from_window(hwnd)?;
    assert_eq!(accessible.child_count()?, 6);

    unsafe { SendMessageW(hwnd, WM_COMMAND, CommandId::New as usize, 0) };
    send_text(editor, "second")?;
    assert_eq!(accessible.child_count()?, 7);
    assert_eq!(accessible.name(2)?, "Untitled *");

    unsafe { SendMessageW(editor, SCI_SETSAVEPOINT, 0, 0) };
    unsafe { SendMessageW(hwnd, WM_COMMAND, CommandId::CloseTab as usize, 0) };
    wait_for_editor_text(editor, "first", Duration::from_secs(2))?;
    assert_eq!(accessible.child_count()?, 6);
    assert_eq!(accessible.select(2), E_INVALIDARG);
    assert_eq!(scintilla_text(editor)?, "first");

    process.close()
}

#[test]
fn save_point_notifications_control_dirty_close_review() -> TestResult<()> {
    let _serial = NATIVE_TEST_LOCK.lock().unwrap();
    let mut process = FastPadProcess::spawn(["--new-window"])?;
    let hwnd = process.wait_for_main_window(Duration::from_secs(3))?;
    let editor = find_child_by_class(hwnd, "Scintilla")?;
    send_text(editor, "unsaved")?;

    assert_ne!(
        unsafe { PostMessageW(hwnd, WM_COMMAND, CommandId::CloseTab as usize, 0) },
        0
    );
    let dialog = wait_for_dialog(process.id(), true, Duration::from_secs(2))?;
    answer_dialog(dialog, IDCANCEL)?;
    wait_for_dialog(process.id(), false, Duration::from_secs(2))?;
    assert_ne!(unsafe { IsWindow(hwnd) }, 0);
    assert_eq!(scintilla_text(editor)?, "unsaved");

    unsafe { SendMessageW(editor, SCI_SETSAVEPOINT, 0, 0) };
    unsafe { SendMessageW(hwnd, WM_COMMAND, CommandId::CloseTab as usize, 0) };
    wait_for_editor_text(editor, "", Duration::from_secs(2))?;
    wait_for_dialog(process.id(), false, Duration::from_millis(100))?;

    process.close()
}

#[test]
fn editing_an_already_dirty_document_invalidates_modal_close_decision() -> TestResult<()> {
    // Break caught: save-point transitions alone do not detect further edits during a prompt.
    let _serial = NATIVE_TEST_LOCK.lock().unwrap();
    let mut process = FastPadProcess::spawn(["--new-window"])?;
    let hwnd = process.wait_for_main_window(Duration::from_secs(3))?;
    let editor = find_child_by_class(hwnd, "Scintilla")?;
    send_text(editor, "unsaved")?;
    unsafe { PostMessageW(hwnd, WM_COMMAND, CommandId::CloseTab as usize, 0) };
    let dialog = wait_for_dialog(process.id(), true, Duration::from_secs(2))?;
    unsafe { SendMessageW(editor, WM_CHAR, b'!' as usize, 0) };
    wait_for_editor_text(editor, "unsaved!", Duration::from_secs(2))?;
    answer_dialog(dialog, IDNO)?;
    wait_for_dialog(process.id(), false, Duration::from_secs(2))?;
    assert_eq!(scintilla_text(editor)?, "unsaved!");
    unsafe { SendMessageW(editor, SCI_SETSAVEPOINT, 0, 0) };
    process.close()
}

#[test]
fn modal_close_does_not_close_a_reentrantly_created_active_tab() -> TestResult<()> {
    // Break caught: a prompt decision must not use the active index or count captured earlier.
    let _serial = NATIVE_TEST_LOCK.lock().unwrap();
    let _com = ComApartment::initialize()?;
    let mut process = FastPadProcess::spawn(["--new-window"])?;
    let hwnd = process.wait_for_main_window(Duration::from_secs(3))?;
    let editor = find_child_by_class(hwnd, "Scintilla")?;
    send_text(editor, "original")?;
    let accessible = Accessible::from_window(hwnd)?;
    unsafe { PostMessageW(hwnd, WM_COMMAND, CommandId::CloseTab as usize, 0) };
    let dialog = wait_for_dialog(process.id(), true, Duration::from_secs(2))?;
    unsafe { SendMessageW(hwnd, WM_COMMAND, CommandId::New as usize, 0) };
    answer_dialog(dialog, IDNO)?;
    wait_for_dialog(process.id(), false, Duration::from_secs(2))?;
    assert_eq!(accessible.child_count()?, 7);
    assert_eq!(accessible.select(1), windows_sys::Win32::Foundation::S_OK);
    assert_eq!(scintilla_text(editor)?, "original");
    unsafe { SendMessageW(editor, SCI_SETSAVEPOINT, 0, 0) };
    process.close()
}

#[test]
fn window_close_reenumerates_documents_dirtied_during_a_prompt() -> TestResult<()> {
    // Break caught: a once-only dirty snapshot silently discards a previously clean document.
    let _serial = NATIVE_TEST_LOCK.lock().unwrap();
    let mut process = FastPadProcess::spawn(["--new-window"])?;
    let hwnd = process.wait_for_main_window(Duration::from_secs(3))?;
    let editor = find_child_by_class(hwnd, "Scintilla")?;
    send_text(editor, "first dirty")?;
    unsafe { SendMessageW(hwnd, WM_COMMAND, CommandId::New as usize, 0) };
    unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
    let dialog = wait_for_dialog(process.id(), true, Duration::from_secs(2))?;
    unsafe { SendMessageW(editor, WM_CHAR, b'x' as usize, 0) };
    answer_dialog(dialog, IDNO)?;
    let next = wait_for_replacement_dialog(process.id(), dialog, Duration::from_secs(2))?;
    answer_dialog(next, IDCANCEL)?;
    wait_for_dialog(process.id(), false, Duration::from_secs(2))?;
    assert_ne!(unsafe { IsWindow(hwnd) }, 0);
    assert_eq!(scintilla_text(editor)?, "x");
    unsafe { SendMessageW(editor, SCI_SETSAVEPOINT, 0, 0) };
    click_tab(hwnd, 0, 2)?;
    unsafe { SendMessageW(editor, SCI_SETSAVEPOINT, 0, 0) };
    process.close()
}

#[test]
fn window_close_reviews_dirty_tabs_in_order_and_cancel_aborts_shutdown() -> TestResult<()> {
    let _serial = NATIVE_TEST_LOCK.lock().unwrap();
    let mut process = FastPadProcess::spawn(["--new-window"])
        .map_err(|error| format!("spawn FastPad: {error}"))?;
    let hwnd = process
        .wait_for_main_window(Duration::from_secs(3))
        .map_err(|error| format!("find main window: {error}"))?;
    let editor = find_child_by_class(hwnd, "Scintilla")
        .map_err(|error| format!("find Scintilla: {error}"))?;
    send_text(editor, "first dirty").map_err(|error| format!("type first tab: {error}"))?;
    unsafe { SendMessageW(hwnd, WM_COMMAND, CommandId::New as usize, 0) };
    wait_for_editor_text(editor, "", Duration::from_secs(2))
        .map_err(|error| format!("wait for new document: {error}"))?;
    send_text(editor, "second dirty").map_err(|error| format!("type second tab: {error}"))?;

    assert_ne!(unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) }, 0);
    answer_next_dialog(process.id(), IDNO)
        .map_err(|error| format!("discard first review: {error}"))?;
    answer_next_dialog(process.id(), IDCANCEL)
        .map_err(|error| format!("cancel second review: {error}"))?;
    wait_for_dialog(process.id(), false, Duration::from_secs(2))?;
    assert_ne!(
        unsafe { IsWindow(hwnd) },
        0,
        "Cancel must abort the whole window close"
    );

    assert_ne!(unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) }, 0);
    answer_next_dialog(process.id(), IDNO)
        .map_err(|error| format!("discard first retry review: {error}"))?;
    answer_next_dialog(process.id(), IDNO)
        .map_err(|error| format!("discard second retry review: {error}"))?;
    process
        .close()
        .map_err(|error| format!("reap closed FastPad: {error}").into())
}

fn click_tab(hwnd: HWND, index: usize, tab_count: usize) -> TestResult<()> {
    let layout = title_layout(hwnd, tab_count)?;
    let point = layout.tab(index).center();
    let packed = (point.x as u16 as u32 | ((point.y as u16 as u32) << 16)) as isize;
    unsafe { SendMessageW(hwnd, WM_LBUTTONUP, 0, packed) };
    Ok(())
}

fn click_new_tab(hwnd: HWND, tab_count: usize) -> TestResult<()> {
    let layout = title_layout(hwnd, tab_count)?;
    let point = layout.new_tab.center();
    let packed = (point.x as u16 as u32 | ((point.y as u16 as u32) << 16)) as isize;
    unsafe { SendMessageW(hwnd, WM_LBUTTONUP, 0, packed) };
    Ok(())
}

fn title_layout(hwnd: HWND, tab_count: usize) -> TestResult<TitleBarLayout> {
    let mut client = windows_sys::Win32::Foundation::RECT::default();
    if unsafe { GetClientRect(hwnd, &mut client) } == 0 {
        return Err(Box::new(fastpad::platform::last_error()));
    }
    Ok(TitleBarLayout::calculate(
        Size::new(client.right - client.left, client.bottom - client.top),
        unsafe { GetDpiForWindow(hwnd) },
        tab_count,
    ))
}

fn wait_for_editor_text(hwnd: HWND, expected: &str, timeout: Duration) -> TestResult<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if scintilla_text(hwnd)? == expected {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!("timed out waiting for editor text {expected:?}").into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn answer_next_dialog(process_id: u32, button: i32) -> TestResult<()> {
    let dialog = wait_for_dialog(process_id, true, Duration::from_secs(2))?;
    answer_dialog(dialog, button)?;
    std::thread::sleep(Duration::from_millis(30));
    Ok(())
}

fn answer_dialog(dialog: HWND, button: i32) -> TestResult<()> {
    let control = unsafe { GetDlgItem(dialog, button) };
    if control.is_null() {
        return Err(format!("close-review dialog did not expose button {button}").into());
    }
    unsafe { SendMessageW(control, BM_CLICK, 0, 0) };
    Ok(())
}

fn wait_for_dialog(process_id: u32, present: bool, timeout: Duration) -> TestResult<HWND> {
    let deadline = Instant::now() + timeout;
    loop {
        let dialog = find_dialog(process_id);
        let ready = dialog.is_some_and(|dialog| unsafe { !GetDlgItem(dialog, IDCANCEL).is_null() });
        if (present && ready) || (!present && dialog.is_none()) {
            return Ok(dialog.unwrap_or(std::ptr::null_mut()));
        }
        if Instant::now() >= deadline {
            return Err(format!("close-review dialog present={present} was not observed").into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_replacement_dialog(
    process_id: u32,
    previous: HWND,
    timeout: Duration,
) -> TestResult<HWND> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(dialog) = find_dialog(process_id).filter(|dialog| {
            *dialog != previous && unsafe { !GetDlgItem(*dialog, IDCANCEL).is_null() }
        }) {
            return Ok(dialog);
        }
        if Instant::now() >= deadline {
            return Err("a fresh close-review dialog was not observed".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn find_dialog(process_id: u32) -> Option<HWND> {
    struct Search {
        process_id: u32,
        found: Option<HWND>,
    }
    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let search = unsafe { &mut *(lparam as *mut Search) };
        let mut owner_process = 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut owner_process) };
        if owner_process != search.process_id {
            return 1;
        }
        let mut class = [0_u16; 32];
        let length = unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::GetClassNameW(
                hwnd,
                class.as_mut_ptr(),
                class.len() as i32,
            )
        };
        if length > 0
            && &class[..length as usize] == wide_null("#32770").strip_suffix(&[0]).unwrap()
        {
            search.found = Some(hwnd);
            return 0;
        }
        1
    }
    let mut search = Search {
        process_id,
        found: None,
    };
    unsafe { EnumWindows(Some(visit), &mut search as *mut Search as isize) };
    search.found
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
    get_acc_child: usize,
    get_acc_name: unsafe extern "system" fn(*mut c_void, VARIANT, *mut BSTR) -> HRESULT,
    get_acc_value: usize,
    get_acc_description: usize,
    get_acc_role: usize,
    get_acc_state: usize,
    get_acc_help: usize,
    get_acc_help_topic: usize,
    get_acc_keyboard_shortcut: usize,
    get_acc_focus: usize,
    get_acc_selection: usize,
    get_acc_default_action: usize,
    acc_select: unsafe extern "system" fn(*mut c_void, i32, VARIANT) -> HRESULT,
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

    fn select(&self, child: i32) -> HRESULT {
        unsafe {
            (self.vtable().acc_select)(self.0, SELFLAG_TAKESELECTION as i32, child_variant(child))
        }
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

    fn name(&self, child: i32) -> TestResult<String> {
        let mut name: BSTR = std::ptr::null();
        let status =
            unsafe { (self.vtable().get_acc_name)(self.0, child_variant(child), &mut name) };
        if status < 0 || name.is_null() {
            return Err(format!("get_accName({child}) failed: {status:#x}").into());
        }
        let len = unsafe { SysStringLen(name) } as usize;
        let text = String::from_utf16(unsafe { std::slice::from_raw_parts(name, len) })?;
        unsafe { SysFreeString(name) };
        Ok(text)
    }
}

fn child_variant(child: i32) -> VARIANT {
    let mut value = VARIANT::default();
    value.Anonymous.Anonymous.vt = VT_I4;
    value.Anonymous.Anonymous.Anonymous.lVal = child;
    value
}

impl Drop for Accessible {
    fn drop(&mut self) {
        unsafe { (self.vtable().release)(self.0) };
    }
}
