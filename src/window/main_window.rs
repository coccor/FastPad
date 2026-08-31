use crate::Result;
use crate::app::App;
use crate::perf::Milestone;
use crate::platform::{last_error, wide_null};
use crate::window::messages::{DeferredAction, classify_deferred_message, deferred_start_message};
use std::ffi::c_void;
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

pub struct WindowCreateContext<T> {
    value: Option<Box<T>>,
}

impl<T> WindowCreateContext<T> {
    pub fn new(value: Box<T>) -> Self {
        Self { value: Some(value) }
    }

    pub fn lp_param(&mut self) -> *mut c_void {
        self as *mut Self as *mut c_void
    }
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

    pub fn create(&self, lp_param: *mut c_void) -> Result<HWND> {
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
                lp_param.cast(),
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

pub(crate) fn maybe_post_deferred_start(hwnd: HWND) {
    let should_post: bool =
        app_mut(hwnd, |app| Ok(app.take_deferred_start_pending())).unwrap_or_default();
    if should_post {
        unsafe {
            PostMessageW(hwnd, deferred_start_message(), 0, 0);
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
            let _ = app_mut(hwnd, |app| {
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
                Ok(())
            });
            0
        }
        WM_SETFOCUS => {
            let _ = app_mut(hwnd, |app| {
                if let Some(editor) = app.editor.as_ref() {
                    unsafe {
                        SetFocus(editor.hwnd());
                    }
                }
                Ok(())
            });
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
            let _ = app_mut(hwnd, |app| {
                app.mark_first_paint_complete();
                Ok(())
            });
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
    let Some(mut app) = (unsafe { take_create_context_value::<App>(lparam) }) else {
        return 0;
    };
    app.hwnd = hwnd;
    store_window_value(hwnd, app);
    1
}

fn handle_deferred(hwnd: HWND, action: DeferredAction) -> LRESULT {
    match action {
        DeferredAction::RepostSelf(message) => {
            let _ = request_input_priority(hwnd);
            unsafe {
                PostMessageW(hwnd, message, 0, 0);
            }
            0
        }
        DeferredAction::PostNext(message) => unsafe {
            PostMessageW(hwnd, message, 0, 0);
            0
        },
        DeferredAction::RecordFullyReady => {
            let _ = app_mut(hwnd, |app| {
                let _ = app.startup.record_now(Milestone::FullyReady);
                Ok(())
            });
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

pub(crate) fn app_mut<R>(hwnd: HWND, map: impl FnOnce(&mut App) -> Result<R>) -> Result<R> {
    let raw = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App };
    if raw.is_null() {
        Err(crate::FastPadError::Invariant(
            "main window app state was not available",
        ))
    } else {
        map(unsafe { &mut *raw })
    }
}

pub(crate) fn request_input_priority(hwnd: HWND) -> Result<()> {
    if let Some(app) = raw_app_mut(hwnd) {
        app.request_input_priority();
    }
    Ok(())
}

pub(crate) fn input_priority_requested(hwnd: HWND) -> Result<bool> {
    Ok(raw_app(hwnd).is_some_and(App::prioritizes_input))
}

pub(crate) fn clear_input_priority(hwnd: HWND) -> Result<()> {
    if let Some(app) = raw_app_mut(hwnd) {
        app.clear_input_priority();
    }
    Ok(())
}

unsafe fn take_app(hwnd: HWND) -> Option<Box<App>> {
    unsafe { take_window_value(hwnd) }
}

fn raw_app(hwnd: HWND) -> Option<&'static App> {
    let raw = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const App };
    (!raw.is_null()).then(|| unsafe { &*raw })
}

fn raw_app_mut(hwnd: HWND) -> Option<&'static mut App> {
    let raw = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App };
    (!raw.is_null()).then(|| unsafe { &mut *raw })
}

unsafe fn take_create_context_value<T>(lparam: LPARAM) -> Option<Box<T>> {
    let create = unsafe { &mut *(lparam as *mut CREATESTRUCTW) };
    let context = create.lpCreateParams as *mut WindowCreateContext<T>;
    if context.is_null() {
        return None;
    }
    unsafe { (*context).value.take() }
}

fn store_window_value<T>(hwnd: HWND, value: Box<T>) {
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(value) as isize);
    }
}

unsafe fn take_window_value<T>(hwnd: HWND) -> Option<Box<T>> {
    let raw = unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *mut T };
    (!raw.is_null()).then(|| unsafe { Box::from_raw(raw) })
}

#[cfg(test)]
mod tests {
    use super::{WindowCreateContext, store_window_value, take_window_value};
    use crate::platform::wide_null;
    use std::ffi::c_void;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use windows_sys::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW,
        UnregisterClassW, WM_APP, WM_NCCREATE, WM_NCDESTROY, WNDCLASSW, WS_OVERLAPPEDWINDOW,
    };

    #[test]
    fn create_context_drops_untransferred_value_on_pre_window_failure() {
        // Break caught: bootstrap manually reclaiming a create-time App allocation is unsafe once
        // ownership can also transfer through WM_NCCREATE.
        let drops = Arc::new(AtomicUsize::new(0));
        {
            let _context = WindowCreateContext::new(Box::new(DropProbe::new(Arc::clone(&drops))));
        }

        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn transferred_window_value_is_dropped_once_by_nc_destroy() {
        // Break caught: an App transferred into GWLP_USERDATA can be leaked or double-freed if
        // teardown does not take ownership exactly once on the WM_NCDESTROY path.
        let drops = Arc::new(AtomicUsize::new(0));
        {
            let mut context =
                WindowCreateContext::new(Box::new(DropProbe::new(Arc::clone(&drops))));
            let window = TransferWindow::new(context.lp_param());
            unsafe {
                DestroyWindow(window.hwnd);
            }
        }

        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn transferred_window_value_is_retrievable_only_once() {
        // Break caught: retrieving the owned App twice from window user data causes a double free
        // when bootstrap error handling and WM_NCDESTROY both believe they own teardown.
        let drops = Arc::new(AtomicUsize::new(0));
        let window = TransferWindow::empty();
        store_window_value(window.hwnd, Box::new(DropProbe::new(Arc::clone(&drops))));

        let owned = unsafe { take_window_value::<DropProbe>(window.hwnd) };
        assert!(owned.is_some());
        assert!(unsafe { take_window_value::<DropProbe>(window.hwnd) }.is_none());
        drop(owned);
        unsafe {
            DestroyWindow(window.hwnd);
        }

        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    struct DropProbe {
        drops: Arc<AtomicUsize>,
    }

    impl DropProbe {
        fn new(drops: Arc<AtomicUsize>) -> Self {
            Self { drops }
        }
    }

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct TransferWindow {
        hwnd: HWND,
        class_name: Vec<u16>,
        instance: HMODULE,
    }

    impl TransferWindow {
        fn new(lp_param: *mut c_void) -> Self {
            let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
            let class_name = wide_null("FastPadTransferWindow");
            let window_class = WNDCLASSW {
                lpfnWndProc: Some(transfer_window_proc),
                hInstance: instance,
                lpszClassName: class_name.as_ptr(),
                ..Default::default()
            };
            let atom = unsafe { RegisterClassW(&window_class) };
            assert_ne!(atom, 0);

            let hwnd = unsafe {
                CreateWindowExW(
                    0,
                    class_name.as_ptr(),
                    class_name.as_ptr(),
                    WS_OVERLAPPEDWINDOW,
                    0,
                    0,
                    100,
                    100,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    instance,
                    lp_param.cast(),
                )
            };
            assert!(!hwnd.is_null());

            Self {
                hwnd,
                class_name,
                instance,
            }
        }

        fn empty() -> Self {
            Self::new(std::ptr::null_mut())
        }
    }

    impl Drop for TransferWindow {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyWindow(self.hwnd);
                UnregisterClassW(self.class_name.as_ptr(), self.instance);
            }
        }
    }

    unsafe extern "system" fn transfer_window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            WM_NCCREATE => {
                let create = unsafe { &*(lparam as *const CREATESTRUCTW) };
                if !create.lpCreateParams.is_null() {
                    let context = create.lpCreateParams as *mut WindowCreateContext<DropProbe>;
                    if let Some(value) = unsafe { (*context).value.take() } {
                        store_window_value(hwnd, value);
                    }
                }
                1
            }
            WM_NCDESTROY => {
                unsafe {
                    drop(take_window_value::<DropProbe>(hwnd));
                }
                unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
            }
            WM_APP => 0,
            _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
        }
    }
}
