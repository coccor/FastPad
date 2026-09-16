use fastpad::platform::{OwnedModule, wide_null};
use std::error::Error;
use std::mem::size_of;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::{
    GetModuleHandleW, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
    LoadLibraryExW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EnumChildWindows,
    GUITHREADINFO, GetClassNameW, GetGUIThreadInfo, GetWindowThreadProcessId, MSG, PM_REMOVE,
    PeekMessageW, RegisterClassW, SMTO_ABORTIFHUNG, SW_HIDE, SendMessageTimeoutW, ShowWindow,
    TranslateMessage, UnregisterClassW, WM_CHAR, WM_GETTEXT, WM_GETTEXTLENGTH, WNDCLASSW,
    WS_OVERLAPPEDWINDOW,
};
use windows_sys::core::BOOL;

type TestResult<T> = Result<T, Box<dyn Error>>;

static WINDOW_CLASS_ID: AtomicUsize = AtomicUsize::new(1);

#[derive(Clone, Debug)]
pub struct Deadline {
    end: Instant,
}

impl Deadline {
    pub fn after(timeout: Duration) -> Self {
        Self {
            end: Instant::now() + timeout,
        }
    }

    pub fn expired(&self) -> bool {
        Instant::now() >= self.end
    }

    pub fn remaining_millis(&self) -> u32 {
        if self.expired() {
            0
        } else {
            self.end
                .saturating_duration_since(Instant::now())
                .as_millis()
                .clamp(1, u128::from(u32::MAX)) as u32
        }
    }

    pub fn sleep_step(&self) {
        if self.expired() {
            return;
        }

        std::thread::sleep(
            Duration::from_millis(10).min(self.end.saturating_duration_since(Instant::now())),
        );
    }
}

#[allow(dead_code)]
pub struct WindowHarness {
    hwnd: HWND,
    _scintilla: OwnedModule,
    class_name: Vec<u16>,
    instance: HMODULE,
}

#[allow(dead_code)]
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

#[allow(dead_code)]
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

pub fn find_child_by_class(parent: HWND, class_name: &str) -> TestResult<HWND> {
    let deadline = Deadline::after(Duration::from_secs(2));
    let wanted = class_name.to_string();
    loop {
        let mut search = ChildSearch {
            wanted: &wanted,
            found: None,
        };
        unsafe {
            EnumChildWindows(
                parent,
                Some(enum_child_by_class),
                &mut search as *mut ChildSearch as isize,
            );
        }
        if let Some(hwnd) = search.found {
            return Ok(hwnd);
        }
        if deadline.expired() {
            return Err(format!("timed out waiting for child class {class_name}").into());
        }
        deadline.sleep_step();
    }
}

pub fn focused_window(window: HWND) -> TestResult<HWND> {
    let deadline = Deadline::after(Duration::from_secs(2));
    loop {
        let thread_id = unsafe { GetWindowThreadProcessId(window, std::ptr::null_mut()) };
        let mut info = GUITHREADINFO {
            cbSize: size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        let ok = unsafe { GetGUIThreadInfo(thread_id, &mut info) };
        if ok != 0 && !info.hwndFocus.is_null() {
            return Ok(info.hwndFocus);
        }
        if deadline.expired() {
            return Err("timed out waiting for focused window".into());
        }
        deadline.sleep_step();
    }
}

pub fn send_text(hwnd: HWND, text: &str) -> TestResult<()> {
    let deadline = Deadline::after(Duration::from_secs(2));
    for unit in text.encode_utf16() {
        let delivered = unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                hwnd,
                WM_CHAR,
                unit as usize,
                0,
            )
        };
        if delivered == 0 {
            return Err(Box::new(fastpad::platform::last_error()));
        }
    }
    wait_for_text(hwnd, text, &deadline)
}

pub fn scintilla_text(hwnd: HWND) -> TestResult<String> {
    scintilla_text_with_deadline(hwnd, &Deadline::after(Duration::from_secs(2)))
}

fn scintilla_text_with_deadline(hwnd: HWND, deadline: &Deadline) -> TestResult<String> {
    let mut length = 0;
    let ok = unsafe {
        SendMessageTimeoutW(
            hwnd,
            WM_GETTEXTLENGTH,
            0,
            0,
            SMTO_ABORTIFHUNG,
            deadline.remaining_millis(),
            &mut length,
        )
    };
    if ok == 0 {
        return Err("timed out reading Scintilla text length".into());
    }

    let mut units = vec![0_u16; length + 1];
    let ok = unsafe {
        SendMessageTimeoutW(
            hwnd,
            WM_GETTEXT,
            units.len(),
            units.as_mut_ptr() as isize,
            SMTO_ABORTIFHUNG,
            deadline.remaining_millis(),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err("timed out reading Scintilla text".into());
    }
    let end = units
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(units.len());
    Ok(String::from_utf16(&units[..end])?)
}

fn wait_for_text(hwnd: HWND, expected: &str, deadline: &Deadline) -> TestResult<()> {
    loop {
        if scintilla_text_with_deadline(hwnd, deadline)? == expected {
            return Ok(());
        }
        if deadline.expired() {
            return Err(format!("timed out waiting for Scintilla text {expected:?}").into());
        }
        deadline.sleep_step();
    }
}

struct ChildSearch<'a> {
    wanted: &'a str,
    found: Option<HWND>,
}

unsafe extern "system" fn enum_child_by_class(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let search = unsafe { &mut *(lparam as *mut ChildSearch<'_>) };
    let mut class_name = [0_u16; 128];
    let length = unsafe { GetClassNameW(hwnd, class_name.as_mut_ptr(), class_name.len() as i32) };
    if length > 0 {
        let current = String::from_utf16_lossy(&class_name[..length as usize]);
        if current == search.wanted {
            search.found = Some(hwnd);
            return 0;
        }
    }
    1
}

#[allow(dead_code)]
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

#[allow(dead_code)]
fn native_scintilla_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("native")
        .join("out")
        .join("x64")
        .join("Scintilla.dll")
}

#[allow(dead_code)]
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::Deadline;
    use std::time::Duration;

    #[test]
    fn deadline_remaining_time_never_exceeds_the_original_budget() {
        // Break caught: nested helper calls that reset their timeout can exceed the documented
        // two-second public budget instead of sharing one deadline.
        let deadline = Deadline::after(Duration::from_millis(50));
        let first = deadline.remaining_millis();
        std::thread::sleep(Duration::from_millis(10));
        let second = deadline.remaining_millis();

        assert!(first <= 50);
        assert!(second <= first);
    }

    #[test]
    fn deadline_expires_to_zero_remaining_time() {
        // Break caught: nested SendMessageTimeout calls can keep granting fresh timeout windows
        // after the public helper budget is already exhausted.
        let deadline = Deadline::after(Duration::from_millis(1));
        std::thread::sleep(Duration::from_millis(20));

        assert_eq!(deadline.remaining_millis(), 0);
    }
}
