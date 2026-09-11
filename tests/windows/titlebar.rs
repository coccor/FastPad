#![cfg(windows)]

mod support;

use fastpad::platform::wide_null;
use fastpad::window::commands::CommandId;
use fastpad::window::titlebar::{Size, TitleBarLayout};
use std::error::Error;
use std::ffi::c_void;
use std::time::Duration;
use support::process::FastPadProcess;
use windows_sys::Win32::Foundation::{
    E_INVALIDARG, RECT, S_FALSE, S_OK, SysFreeString, SysStringLen,
};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows_sys::Win32::UI::Accessibility::{
    AccessibleObjectFromWindow, ObjectFromLresult, ROLE_SYSTEM_PAGETAB, ROLE_SYSTEM_PAGETABLIST,
    ROLE_SYSTEM_PUSHBUTTON, SELFLAG_TAKESELECTION,
};
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow,
    SetThreadDpiAwarenessContext,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_F10, VK_MENU, VK_SPACE};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GetClientRect, GetMenu, GetMenuItemCount, GetWindowRect, HTLEFT, HTMAXBUTTON,
    MINMAXINFO, OBJID_CLIENT, PostMessageW, STATE_SYSTEM_SELECTED, SendMessageW, WM_CANCELMODE,
    WM_COMMAND, WM_GETMINMAXINFO, WM_GETOBJECT, WM_NCHITTEST, WM_SYSKEYDOWN, WM_SYSKEYUP,
};
use windows_sys::core::{BSTR, GUID, HRESULT};

type TestResult<T> = Result<T, Box<dyn Error>>;

const IID_IACCESSIBLE: GUID = GUID::from_u128(0x618736e0_3c3d_11cf_810c_00aa00389b71);

#[test]
fn get_object_returns_a_marshaled_title_provider() -> TestResult<()> {
    let _dpi = DpiContext::per_monitor_v2()?;
    let _com = ComApartment::initialize()?;
    let mut process = FastPadProcess::spawn(["--new-window"])?;
    let hwnd = process.wait_for_main_window(Duration::from_secs(3))?;

    let result = unsafe { SendMessageW(hwnd, WM_GETOBJECT, 0, OBJID_CLIENT as isize) };
    assert_ne!(result, 0, "WM_GETOBJECT did not return the title provider");
    let accessible = Accessible::from_lresult(result, 0)?;
    assert_eq!(accessible.child_count()?, 6);

    process.close()
}

#[test]
fn wm_command_exit_routes_through_app_execute() -> TestResult<()> {
    let _dpi = DpiContext::per_monitor_v2()?;
    let mut process = FastPadProcess::spawn(["--new-window"])?;
    let hwnd = process.wait_for_main_window(Duration::from_secs(3))?;
    let process_id = process.id();

    unsafe {
        SendMessageW(hwnd, WM_COMMAND, CommandId::Exit as usize, 0);
    }
    support::process::wait_for_process_exit(process_id, Duration::from_secs(2))?;
    drop(process);
    Ok(())
}

#[test]
fn alt_and_f10_from_the_focused_editor_activate_the_transient_menu() -> TestResult<()> {
    let _dpi = DpiContext::per_monitor_v2()?;
    let mut process = FastPadProcess::spawn(["--new-window"])?;
    let hwnd = process.wait_for_main_window(Duration::from_secs(3))?;
    let scintilla = wide_null("Scintilla");
    let editor = unsafe {
        FindWindowExW(
            hwnd,
            std::ptr::null_mut(),
            scintilla.as_ptr(),
            std::ptr::null(),
        )
    };
    assert!(!editor.is_null());

    for key in [VK_F10, VK_MENU] {
        assert_ne!(
            unsafe { PostMessageW(editor, WM_SYSKEYDOWN, key as usize, 0) },
            0
        );
        if key == VK_MENU {
            assert_ne!(
                unsafe { PostMessageW(editor, WM_SYSKEYUP, key as usize, 0) },
                0
            );
        }
        let menu = wait_for_menu(hwnd, true, Duration::from_secs(1))?;
        assert_eq!(unsafe { GetMenuItemCount(menu) }, 5);

        assert_ne!(unsafe { PostMessageW(hwnd, WM_CANCELMODE, 0, 0) }, 0);
        wait_for_menu(hwnd, false, Duration::from_secs(1))?;
    }
    process.close()
}

#[test]
fn alt_space_does_not_attach_the_transient_menu() -> TestResult<()> {
    let _dpi = DpiContext::per_monitor_v2()?;
    let mut process = FastPadProcess::spawn(["--new-window"])?;
    let hwnd = process.wait_for_main_window(Duration::from_secs(3))?;
    let scintilla = wide_null("Scintilla");
    let editor = unsafe {
        FindWindowExW(
            hwnd,
            std::ptr::null_mut(),
            scintilla.as_ptr(),
            std::ptr::null(),
        )
    };
    assert!(!editor.is_null());

    assert_ne!(
        unsafe { PostMessageW(editor, WM_SYSKEYDOWN, VK_MENU as usize, 0) },
        0
    );
    assert_ne!(
        unsafe { PostMessageW(editor, WM_SYSKEYDOWN, VK_SPACE as usize, 0) },
        0
    );
    std::thread::sleep(Duration::from_millis(100));
    assert!(unsafe { GetMenu(hwnd) }.is_null());

    assert_ne!(unsafe { PostMessageW(hwnd, WM_CANCELMODE, 0, 0) }, 0);
    process.close()
}

fn wait_for_menu(
    hwnd: windows_sys::Win32::Foundation::HWND,
    attached: bool,
    timeout: Duration,
) -> TestResult<windows_sys::Win32::UI::WindowsAndMessaging::HMENU> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let menu = unsafe { GetMenu(hwnd) };
        if !menu.is_null() == attached {
            return Ok(menu);
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!("transient menu attached={attached} was not observed").into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct ComApartment;

impl ComApartment {
    fn initialize() -> TestResult<Self> {
        let result = unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };
        if result < 0 {
            Err(format!("CoInitializeEx failed: {result:#x}").into())
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

#[test]
fn custom_titlebar_preserves_snap_hit_target_and_accessible_children() -> TestResult<()> {
    let _dpi = DpiContext::per_monitor_v2()?;
    let _com = ComApartment::initialize()?;
    let mut process = FastPadProcess::spawn(["--new-window"])?;
    let hwnd = process.wait_for_main_window(Duration::from_secs(3))?;

    let mut client = RECT::default();
    assert_ne!(unsafe { GetClientRect(hwnd, &mut client) }, 0);
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let layout = TitleBarLayout::calculate(
        Size::new(client.right - client.left, client.bottom - client.top),
        dpi,
        1,
    );
    let point = layout.maximize.center();
    let mut screen_point = windows_sys::Win32::Foundation::POINT {
        x: point.x,
        y: point.y,
    };
    assert_ne!(
        unsafe { windows_sys::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut screen_point) },
        0
    );
    let packed = (screen_point.x as u16 as u32 | ((screen_point.y as u16 as u32) << 16)) as isize;
    assert_eq!(
        unsafe { SendMessageW(hwnd, WM_NCHITTEST, 0, packed) },
        HTMAXBUTTON as isize
    );

    let mut window = RECT::default();
    assert_ne!(unsafe { GetWindowRect(hwnd, &mut window) }, 0);
    let mut client_origin = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
    assert_ne!(
        unsafe { windows_sys::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut client_origin) },
        0
    );
    let resize_point = windows_sys::Win32::Foundation::POINT {
        x: window.left + 1,
        y: client_origin.y + layout.height / 2,
    };
    let packed = (resize_point.x as u16 as u32 | ((resize_point.y as u16 as u32) << 16)) as isize;
    assert_eq!(
        unsafe { SendMessageW(hwnd, WM_NCHITTEST, 0, packed) },
        HTLEFT as isize,
        "custom title strip swallowed the left resize border: window=({}, {}, {}, {}), client_origin=({}, {}), client=({}, {}, {}, {})",
        window.left,
        window.top,
        window.right,
        window.bottom,
        client_origin.x,
        client_origin.y,
        client.left,
        client.top,
        client.right,
        client.bottom
    );

    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    assert!(!monitor.is_null());
    let mut monitor_info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    assert_ne!(unsafe { GetMonitorInfoW(monitor, &mut monitor_info) }, 0);
    let mut minmax = MINMAXINFO::default();
    unsafe {
        SendMessageW(
            hwnd,
            WM_GETMINMAXINFO,
            0,
            (&mut minmax as *mut MINMAXINFO) as isize,
        );
    }
    assert_eq!(
        (minmax.ptMaxPosition.x, minmax.ptMaxPosition.y),
        (
            monitor_info.rcWork.left - monitor_info.rcMonitor.left,
            monitor_info.rcWork.top - monitor_info.rcMonitor.top,
        )
    );
    assert_eq!(
        (minmax.ptMaxSize.x, minmax.ptMaxSize.y),
        (
            monitor_info.rcWork.right - monitor_info.rcWork.left,
            monitor_info.rcWork.bottom - monitor_info.rcWork.top,
        )
    );

    let accessible = Accessible::from_window(hwnd)?;
    assert_eq!(std::mem::size_of::<RawVariant>(), 16);
    assert_eq!(accessible.child_count()?, 6);
    assert_eq!(accessible.role(0)?, ROLE_SYSTEM_PAGETABLIST as i32);
    assert_eq!(accessible.role(1)?, ROLE_SYSTEM_PAGETAB as i32);
    for child in 2..=6 {
        assert_eq!(accessible.role(child)?, ROLE_SYSTEM_PUSHBUTTON as i32);
    }
    let names = (1..=6)
        .map(|child| accessible.name(child))
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(
        names,
        vec![
            "Untitled", "New tab", "Overflow", "Minimize", "Maximize", "Close"
        ]
    );
    assert_ne!(accessible.state(1)? as u32 & STATE_SYSTEM_SELECTED, 0);
    assert_eq!(accessible.focus()?, None);
    assert_eq!(accessible.selection()?, Some(1));
    assert_eq!(accessible.default_action(1)?, "Close");
    assert_eq!(accessible.select(SELFLAG_TAKESELECTION as i32, 1), S_OK);
    assert_eq!(
        accessible.select(SELFLAG_TAKESELECTION as i32, 2),
        E_INVALIDARG
    );

    process.close()
}

struct DpiContext(DPI_AWARENESS_CONTEXT);

impl DpiContext {
    fn per_monitor_v2() -> TestResult<Self> {
        let previous =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        if previous.is_null() {
            Err("SetThreadDpiAwarenessContext failed".into())
        } else {
            Ok(Self(previous))
        }
    }
}

impl Drop for DpiContext {
    fn drop(&mut self) {
        unsafe {
            SetThreadDpiAwarenessContext(self.0);
        }
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
    get_acc_name: unsafe extern "system" fn(*mut c_void, RawVariant, *mut BSTR) -> HRESULT,
    get_acc_value: usize,
    get_acc_description: usize,
    get_acc_role: unsafe extern "system" fn(*mut c_void, RawVariant, *mut RawVariant) -> HRESULT,
    get_acc_state: unsafe extern "system" fn(*mut c_void, RawVariant, *mut RawVariant) -> HRESULT,
    get_acc_help: usize,
    get_acc_help_topic: usize,
    get_acc_keyboard_shortcut: usize,
    get_acc_focus: unsafe extern "system" fn(*mut c_void, *mut RawVariant) -> HRESULT,
    get_acc_selection: unsafe extern "system" fn(*mut c_void, *mut RawVariant) -> HRESULT,
    get_acc_default_action:
        unsafe extern "system" fn(*mut c_void, RawVariant, *mut BSTR) -> HRESULT,
    acc_select: unsafe extern "system" fn(*mut c_void, i32, RawVariant) -> HRESULT,
}

#[repr(C)]
union VariantData {
    l_val: i32,
    _alignment: u64,
}

#[repr(C)]
struct RawVariant {
    vt: u16,
    reserved: [u16; 3],
    data: VariantData,
}

impl RawVariant {
    fn child(id: i32) -> Self {
        Self {
            vt: 3,
            reserved: [0; 3],
            data: VariantData { l_val: id },
        }
    }

    fn integer(&self) -> Option<i32> {
        (self.vt == 3).then_some(unsafe { self.data.l_val })
    }

    fn empty() -> Self {
        Self {
            vt: 0,
            reserved: [0; 3],
            data: VariantData { _alignment: 0 },
        }
    }
}

struct Accessible(*mut c_void);

impl Accessible {
    fn from_lresult(result: isize, wparam: usize) -> TestResult<Self> {
        let mut object = std::ptr::null_mut();
        let status = unsafe { ObjectFromLresult(result, &IID_IACCESSIBLE, wparam, &mut object) };
        if status < 0 || object.is_null() {
            return Err(format!("ObjectFromLresult failed: {status:#x}").into());
        }
        Ok(Self(object))
    }

    fn from_window(hwnd: windows_sys::Win32::Foundation::HWND) -> TestResult<Self> {
        let mut object = std::ptr::null_mut();
        let result = unsafe {
            AccessibleObjectFromWindow(hwnd, OBJID_CLIENT as u32, &IID_IACCESSIBLE, &mut object)
        };
        if result < 0 || object.is_null() {
            return Err(format!("AccessibleObjectFromWindow failed: {result:#x}").into());
        }
        Ok(Self(object))
    }

    fn vtable(&self) -> &AccessibleVtable {
        unsafe { &**(self.0 as *const *const AccessibleVtable) }
    }

    fn child_count(&self) -> TestResult<i32> {
        let mut count = 0;
        let result = unsafe { (self.vtable().get_acc_child_count)(self.0, &mut count) };
        if result < 0 {
            Err(format!("get_accChildCount failed: {result:#x}").into())
        } else {
            Ok(count)
        }
    }

    fn name(&self, child: i32) -> TestResult<String> {
        let mut value: BSTR = std::ptr::null();
        let result =
            unsafe { (self.vtable().get_acc_name)(self.0, RawVariant::child(child), &mut value) };
        if result < 0 || value.is_null() {
            return Err(format!("get_accName({child}) failed: {result:#x}").into());
        }
        let length = unsafe { SysStringLen(value) } as usize;
        let name = String::from_utf16(unsafe { std::slice::from_raw_parts(value, length) })?;
        unsafe {
            SysFreeString(value);
        }
        Ok(name)
    }

    fn role(&self, child: i32) -> TestResult<i32> {
        let mut value = RawVariant::child(0);
        let result =
            unsafe { (self.vtable().get_acc_role)(self.0, RawVariant::child(child), &mut value) };
        if result < 0 {
            return Err(format!("get_accRole({child}) failed: {result:#x}").into());
        }
        value
            .integer()
            .ok_or_else(|| format!("get_accRole({child}) returned a non-integer").into())
    }

    fn state(&self, child: i32) -> TestResult<i32> {
        let mut value = RawVariant::empty();
        let result =
            unsafe { (self.vtable().get_acc_state)(self.0, RawVariant::child(child), &mut value) };
        if result < 0 {
            return Err(format!("get_accState({child}) failed: {result:#x}").into());
        }
        value
            .integer()
            .ok_or_else(|| format!("get_accState({child}) returned a non-integer").into())
    }

    fn focus(&self) -> TestResult<Option<i32>> {
        let mut value = RawVariant::child(99);
        let result = unsafe { (self.vtable().get_acc_focus)(self.0, &mut value) };
        if result != S_FALSE {
            return Err(format!("get_accFocus returned {result:#x}, expected S_FALSE").into());
        }
        Ok(value.integer())
    }

    fn selection(&self) -> TestResult<Option<i32>> {
        let mut value = RawVariant::empty();
        let result = unsafe { (self.vtable().get_acc_selection)(self.0, &mut value) };
        if result < 0 {
            return Err(format!("get_accSelection failed: {result:#x}").into());
        }
        Ok(value.integer())
    }

    fn default_action(&self, child: i32) -> TestResult<String> {
        let mut value: BSTR = std::ptr::null();
        let result = unsafe {
            (self.vtable().get_acc_default_action)(self.0, RawVariant::child(child), &mut value)
        };
        if result < 0 || value.is_null() {
            return Err(format!("get_accDefaultAction({child}) failed: {result:#x}").into());
        }
        let length = unsafe { SysStringLen(value) } as usize;
        let action = String::from_utf16(unsafe { std::slice::from_raw_parts(value, length) })?;
        unsafe { SysFreeString(value) };
        Ok(action)
    }

    fn select(&self, flags: i32, child: i32) -> HRESULT {
        unsafe { (self.vtable().acc_select)(self.0, flags, RawVariant::child(child)) }
    }
}

impl Drop for Accessible {
    fn drop(&mut self) {
        unsafe {
            (self.vtable().release)(self.0);
        }
    }
}
