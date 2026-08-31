use fastpad::platform::{OwnedModule, wide_null};
use std::error::Error;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use windows_sys::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::{
    GetModuleHandleW, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
    LoadLibraryExW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, MSG, PM_REMOVE, PeekMessageW,
    RegisterClassW, SW_HIDE, ShowWindow, TranslateMessage, UnregisterClassW, WNDCLASSW,
    WS_OVERLAPPEDWINDOW,
};

type TestResult<T> = Result<T, Box<dyn Error>>;

static WINDOW_CLASS_ID: AtomicUsize = AtomicUsize::new(1);

pub struct WindowHarness {
    hwnd: HWND,
    _scintilla: OwnedModule,
    class_name: Vec<u16>,
    instance: HMODULE,
}

impl WindowHarness {
    pub fn new() -> TestResult<Self> {
        let scintilla = load_scintilla()?;
        let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
        if instance.is_null() {
            return Err(Box::new(fastpad::platform::last_error()));
        }

        let class_name = wide_null(&format!(
            "FastPadEditorHarness{}",
            WINDOW_CLASS_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let window_class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class_name.as_ptr(),
            ..Default::default()
        };

        let atom = unsafe { RegisterClassW(&window_class) };
        if atom == 0 {
            return Err(Box::new(fastpad::platform::last_error()));
        }

        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class_name.as_ptr(),
                class_name.as_ptr(),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                640,
                480,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            )
        };
        if hwnd.is_null() {
            unsafe {
                UnregisterClassW(class_name.as_ptr(), instance);
            }
            return Err(Box::new(fastpad::platform::last_error()));
        }

        unsafe {
            ShowWindow(hwnd, SW_HIDE);
        }

        let harness = Self {
            hwnd,
            _scintilla: scintilla,
            class_name,
            instance,
        };
        harness.pump_messages()?;
        Ok(harness)
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    pub fn pump_messages(&self) -> TestResult<()> {
        unsafe {
            let mut message = MSG::default();
            while PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        Ok(())
    }
}

impl Drop for WindowHarness {
    fn drop(&mut self) {
        unsafe {
            if !self.hwnd.is_null() {
                DestroyWindow(self.hwnd);
            }
            UnregisterClassW(self.class_name.as_ptr(), self.instance);
        }
    }
}

fn load_scintilla() -> TestResult<OwnedModule> {
    let path = native_scintilla_path();
    let text = path
        .to_str()
        .ok_or_else(|| "native Scintilla path was not valid Unicode".to_string())?;
    let wide_path = wide_null(text);
    let module = unsafe {
        LoadLibraryExW(
            wide_path.as_ptr(),
            std::ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    };
    Ok(unsafe { OwnedModule::from_raw_owned(module) }?)
}

fn native_scintilla_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("native")
        .join("out")
        .join("x64")
        .join("Scintilla.dll")
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}
