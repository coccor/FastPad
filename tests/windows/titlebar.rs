#![cfg(windows)]

mod support;

use fastpad::window::titlebar::{Size, TitleBarLayout};
use std::error::Error;
use std::ffi::c_void;
use std::time::Duration;
use support::process::FastPadProcess;
use windows_sys::Win32::Foundation::{RECT, SysFreeString, SysStringLen};
use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows_sys::Win32::UI::Accessibility::{AccessibleObjectFromWindow, ObjectFromLresult};
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow,
    SetThreadDpiAwarenessContext,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetWindowRect, HTLEFT, HTMAXBUTTON, OBJID_CLIENT, SendMessageW, WM_GETOBJECT,
    WM_NCHITTEST,
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

struct ComApartment;

impl ComApartment {
    fn initialize() -> TestResult<Self> {
        let result = unsafe {
            CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32)
        };
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
    let packed =
        (screen_point.x as u16 as u32 | ((screen_point.y as u16 as u32) << 16)) as isize;
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
    let packed =
        (resize_point.x as u16 as u32 | ((resize_point.y as u16 as u32) << 16)) as isize;
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

    let accessible = Accessible::from_window(hwnd)?;
    assert_eq!(accessible.child_count()?, 6);
    let names = (1..=6)
        .map(|child| accessible.name(child))
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(
        names,
        vec![
            "Untitled",
            "New tab",
            "Overflow",
            "Minimize",
            "Maximize",
            "Close"
        ]
    );

    process.close()
}

struct DpiContext(DPI_AWARENESS_CONTEXT);

impl DpiContext {
    fn per_monitor_v2() -> TestResult<Self> {
        let previous = unsafe {
            SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
        };
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
    get_acc_name:
        unsafe extern "system" fn(*mut c_void, RawVariant, *mut BSTR) -> HRESULT,
}

#[repr(C)]
union VariantData {
    l_val: i32,
    _alignment: [u64; 2],
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
}

struct Accessible(*mut c_void);

impl Accessible {
    fn from_lresult(result: isize, wparam: usize) -> TestResult<Self> {
        let mut object = std::ptr::null_mut();
        let status = unsafe {
            ObjectFromLresult(result, &IID_IACCESSIBLE, wparam, &mut object)
        };
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
        let result = unsafe {
            (self.vtable().get_acc_name)(self.0, RawVariant::child(child), &mut value)
        };
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
}

impl Drop for Accessible {
    fn drop(&mut self) {
        unsafe {
            (self.vtable().release)(self.0);
        }
    }
}
