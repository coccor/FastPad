use crate::Result;
use crate::app::App;
use crate::perf::Milestone;
use crate::platform::{last_error, wide_null};
use crate::window::messages::{DeferredAction, classify_deferred_message, deferred_start_message};
use std::mem;
use windows_sys::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetClientRect,
    GetWindowLongPtrW, MoveWindow, PostMessageW, PostQuitMessage, RegisterClassW,
    SetWindowLongPtrW, UnregisterClassW, WM_CLOSE, WM_DESTROY, WM_NCCREATE, WM_NCDESTROY, WM_PAINT,
    WM_SETFOCUS, WM_SIZE, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};

pub struct MainWindowClass {
    class_name: Vec<u16>,
    instance: HMODULE,
}

impl MainWindowClass {
    pub fn register(instance: HMODULE) -> Result<Self> {
        let class_name = wide_null("FastPadMainWindow");
        let window_class = WNDCLASSW {
            lpfnWndProc: Some(main_window_proc),
            hInstance: instance,
            lpszClassName: class_name.as_ptr(),
            ..Default::default()
        };
        let atom = unsafe { RegisterClassW(&window_class) };
        if atom == 0 {
            return Err(last_error());
        }
        Ok(Self {
            class_name,
            instance,
        })
    }

    pub fn create(&self, app_ptr: *mut App) -> Result<HWND> {
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                self.class_name.as_ptr(),
                self.class_name.as_ptr(),
                WS_OVERLAPPEDWINDOW,
                100,
                100,
                1280,
                720,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                self.instance,
                app_ptr.cast(),
            )
        };
        if hwnd.is_null() {
            Err(last_error())
        } else {
            Ok(hwnd)
        }
    }
}

impl Drop for MainWindowClass {
    fn drop(&mut self) {
        unsafe {
            UnregisterClassW(self.class_name.as_ptr(), self.instance);
        }
    }
}

pub fn maybe_post_deferred_start(hwnd: HWND) {
    if let Some(app) = unsafe { app_mut(hwnd) } {
        if app.take_deferred_start_pending() {
            unsafe {
                PostMessageW(hwnd, deferred_start_message(), 0, 0);
            }
        }
    }
}

unsafe extern "system" fn main_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_NCCREATE => unsafe { on_nc_create(hwnd, lparam) },
        WM_SIZE => {
            if let Some(app) = unsafe { app_mut(hwnd) } {
                if let Some(editor) = app.editor.as_ref() {
                    let mut rect = Default::default();
                    unsafe {
                        GetClientRect(hwnd, &mut rect);
                        MoveWindow(
                            editor.hwnd(),
                            0,
                            0,
                            rect.right - rect.left,
                            rect.bottom - rect.top,
                            1,
                        );
                    }
                }
            }
            0
        }
        WM_SETFOCUS => {
            if let Some(app) = unsafe { app_mut(hwnd) } {
                if let Some(editor) = app.editor.as_ref() {
                    unsafe {
                        SetFocus(editor.hwnd());
                    }
                }
            }
            0
        }
        WM_CLOSE => {
            unsafe {
                DestroyWindow(hwnd);
            }
            0
        }
        WM_DESTROY => {
            unsafe {
                PostQuitMessage(0);
            }
            0
        }
        WM_PAINT => {
            let result = unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
            if let Some(app) = unsafe { app_mut(hwnd) } {
                app.mark_first_paint_complete();
            }
            result
        }
        WM_NCDESTROY => {
            let result = unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
            unsafe {
                drop(take_app(hwnd));
            }
            result
        }
        _ => {
            if let Some(action) = classify_deferred_message(message, input_pending()) {
                return handle_deferred(hwnd, action);
            }
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
    }
}

unsafe fn on_nc_create(hwnd: HWND, lparam: LPARAM) -> LRESULT {
    let create = unsafe { &*(lparam as *const CREATESTRUCTW) };
    let app_ptr = create.lpCreateParams as *mut App;
    if app_ptr.is_null() {
        return 0;
    }
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, app_ptr as isize);
        (*app_ptr).hwnd = hwnd;
    }
    1
}

fn handle_deferred(hwnd: HWND, action: DeferredAction) -> LRESULT {
    match action {
        DeferredAction::RepostSelf(message) => unsafe {
            PostMessageW(hwnd, message, 0, 0);
            0
        },
        DeferredAction::PostNext(message) => unsafe {
            PostMessageW(hwnd, message, 0, 0);
            0
        },
        DeferredAction::RecordFullyReady => {
            if let Some(app) = unsafe { app_mut(hwnd) } {
                let _ = app.startup.record_now(Milestone::FullyReady);
            }
            0
        }
    }
}

fn input_pending() -> bool {
    const STATUS_SHIFT: u32 = 16;
    unsafe {
        ((windows_sys::Win32::UI::WindowsAndMessaging::GetQueueStatus(
            windows_sys::Win32::UI::WindowsAndMessaging::QS_INPUT,
        ) >> STATUS_SHIFT)
            & windows_sys::Win32::UI::WindowsAndMessaging::QS_INPUT)
            != 0
    }
}

unsafe fn app_mut(hwnd: HWND) -> Option<&'static mut App> {
    let raw = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App };
    (!raw.is_null()).then_some(unsafe { &mut *raw })
}

unsafe fn take_app(hwnd: HWND) -> Option<Box<App>> {
    let raw = unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *mut App };
    unsafe { RawBoxOnce::new(raw).take() }
}

struct RawBoxOnce<T> {
    raw: *mut T,
}

impl<T> RawBoxOnce<T> {
    fn new(raw: *mut T) -> Self {
        Self { raw }
    }

    unsafe fn take(&mut self) -> Option<Box<T>> {
        let raw = mem::replace(&mut self.raw, std::ptr::null_mut());
        (!raw.is_null()).then(|| unsafe { Box::from_raw(raw) })
    }
}

#[cfg(test)]
mod tests {
    use super::RawBoxOnce;
    use crate::app::App;
    use crate::launch::LaunchOptions;
    use crate::perf::StartupMetrics;

    #[test]
    fn raw_box_once_releases_the_app_pointer_only_once() {
        // Break caught: recovering the same App allocation twice from window user data causes a
        // double free during WM_NCDESTROY or teardown retries.
        let app = Box::new(App::new(
            LaunchOptions::default(),
            StartupMetrics::with_frequency(1, 0),
        ));
        let raw = Box::into_raw(app);
        let mut slot = RawBoxOnce::new(raw);

        assert!(unsafe { slot.take() }.is_some());
        assert!(unsafe { slot.take() }.is_none());
    }
}
