use crate::app::App;
use crate::editor::Editor;
use crate::error::StartupStage;
use crate::launch::LaunchOptions;
use crate::perf::{Milestone, StartupMetrics};
use crate::platform::{OwnedModule, last_error, wide_null};
use crate::window::{
    MainWindowClass, WindowCreateContext, app_mut, clear_input_priority, input_priority_requested,
    maybe_post_deferred_start,
};
use crate::{FastPadError, Result};
use std::path::PathBuf;
use windows_sys::Win32::Foundation::{HMODULE, HWND};
use windows_sys::Win32::System::LibraryLoader::{
    GetModuleHandleW, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
    LoadLibraryExW,
};
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, IsWindow, MSG, PM_REMOVE, PeekMessageW, SW_SHOW, ShowWindow,
    TranslateMessage, WM_KEYFIRST, WM_KEYLAST, WM_MOUSEFIRST, WM_MOUSELAST, WM_PAINT,
};

pub fn run(options: LaunchOptions) -> Result<i32> {
    let startup = StartupMetrics::begin()?;
    configure_dpi();
    let _scintilla = load_scintilla_module()
        .map_err(|error| FastPadError::startup(StartupStage::ScintillaLoad, error))?;
    let instance = current_module()?;
    let window_class = MainWindowClass::register(instance)
        .map_err(|error| FastPadError::startup(StartupStage::WindowClassRegistration, error))?;

    let mut create_context = WindowCreateContext::new(Box::new(App::new(options, startup)));
    let hwnd = window_class.create(create_context.lp_param())?;
    let mut teardown = ParentWindowGuard::new(hwnd);

    let editor_hwnd = app_mut(hwnd, |app| {
        let _ = app.startup.record_now(Milestone::WindowCreated);
        let editor = Editor::create(hwnd)
            .map_err(|error| FastPadError::startup(StartupStage::EditorCreate, error))?;
        let _ = app.startup.record_now(Milestone::EditorCreated);
        let editor_hwnd = editor.hwnd();
        app.editor = Some(editor);
        Ok(editor_hwnd)
    })?;

    unsafe {
        ShowWindow(hwnd, SW_SHOW);
        SetFocus(editor_hwnd);
    }

    teardown.disarm();
    message_loop(hwnd)
}

fn configure_dpi() {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

fn load_scintilla_module() -> Result<OwnedModule> {
    let path = native_scintilla_path();
    let text = path.to_str().ok_or(FastPadError::Invariant(
        "Scintilla path was not valid Unicode",
    ))?;
    let wide_path = wide_null(text);
    let raw = unsafe {
        LoadLibraryExW(
            wide_path.as_ptr(),
            std::ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    };
    unsafe { OwnedModule::from_raw_owned(raw) }
}

fn native_scintilla_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("native")
        .join("out")
        .join("x64")
        .join("Scintilla.dll")
}

fn current_module() -> Result<HMODULE> {
    let module = unsafe { GetModuleHandleW(std::ptr::null()) };
    if module.is_null() {
        Err(last_error())
    } else {
        Ok(module)
    }
}

fn message_loop(hwnd: HWND) -> Result<i32> {
    let mut message = MSG::default();
    loop {
        drain_prioritized_input(hwnd)?;
        let status = unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) };
        if status == -1 {
            return Err(last_error());
        }
        if status == 0 {
            return Ok(message.wParam as i32);
        }

        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }

        if message.hwnd == hwnd && message.message == WM_PAINT {
            maybe_post_deferred_start(hwnd);
        }
    }
}

fn drain_prioritized_input(hwnd: HWND) -> Result<()> {
    if !input_priority_requested(hwnd)? {
        return Ok(());
    }

    while dispatch_next_input_message()? {}
    clear_input_priority(hwnd)?;
    Ok(())
}

fn dispatch_next_input_message() -> Result<bool> {
    let mut message = MSG::default();
    let found_keyboard = unsafe {
        PeekMessageW(
            &mut message,
            std::ptr::null_mut(),
            WM_KEYFIRST,
            WM_KEYLAST,
            PM_REMOVE,
        )
    };
    let found = if found_keyboard != 0 {
        true
    } else {
        unsafe {
            PeekMessageW(
                &mut message,
                std::ptr::null_mut(),
                WM_MOUSEFIRST,
                WM_MOUSELAST,
                PM_REMOVE,
            ) != 0
        }
    };
    if !found {
        return Ok(false);
    }

    unsafe {
        TranslateMessage(&message);
        DispatchMessageW(&message);
    }
    Ok(true)
}

struct ParentWindowGuard {
    hwnd: HWND,
    armed: bool,
}

impl ParentWindowGuard {
    fn new(hwnd: HWND) -> Self {
        Self { hwnd, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ParentWindowGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }

        unsafe {
            if IsWindow(self.hwnd) != 0 {
                let _ = windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(self.hwnd);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{StartupStage, dispatch_next_input_message};
    use crate::platform::wide_null;
    use std::sync::{Arc, Mutex};
    use windows_sys::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, MSG, PM_REMOVE, PeekMessageW, PostMessageW,
        RegisterClassW, UnregisterClassW, WM_KEYDOWN, WNDCLASSW, WS_OVERLAPPEDWINDOW,
    };

    #[test]
    fn startup_stage_exit_codes_match_bootstrap_contract() {
        // Break caught: changing fatal startup exit codes breaks the shell's distinct user-facing
        // failures for Scintilla loading, class registration, and editor creation.
        assert_eq!(StartupStage::ScintillaLoad.exit_code(), 10);
        assert_eq!(StartupStage::WindowClassRegistration.exit_code(), 11);
        assert_eq!(StartupStage::EditorCreate.exit_code(), 12);
    }

    #[test]
    fn prioritized_input_dispatches_keyboard_before_reposted_deferred_work() {
        // Break caught: a reposted deferred startup message can livelock ahead of pending input
        // unless the message loop explicitly drains QS_INPUT before reading the queue again.
        let state = Arc::new(Mutex::new(Vec::new()));
        let window = TestWindow::new(Arc::clone(&state));
        pump_thread_messages();
        state.lock().unwrap().clear();

        unsafe {
            PostMessageW(window.hwnd, crate::window::WM_FASTPAD_LOAD_SETTINGS, 0, 0);
            PostMessageW(window.hwnd, WM_KEYDOWN, usize::from(b'X'), 0);
        }

        assert!(dispatch_next_input_message().unwrap());
        let mut queued = MSG::default();
        let status = unsafe {
            PeekMessageW(
                &mut queued,
                std::ptr::null_mut(),
                crate::window::WM_FASTPAD_LOAD_SETTINGS,
                crate::window::WM_FASTPAD_LOAD_SETTINGS,
                PM_REMOVE,
            )
        };
        assert_ne!(status, 0);
        assert_eq!(queued.message, crate::window::WM_FASTPAD_LOAD_SETTINGS);
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::TranslateMessage(&queued);
            windows_sys::Win32::UI::WindowsAndMessaging::DispatchMessageW(&queued);
        }

        assert_eq!(
            *state.lock().unwrap(),
            vec![WM_KEYDOWN, crate::window::WM_FASTPAD_LOAD_SETTINGS]
        );
    }

    fn pump_thread_messages() {
        unsafe {
            let mut message = MSG::default();
            while PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                windows_sys::Win32::UI::WindowsAndMessaging::TranslateMessage(&message);
                windows_sys::Win32::UI::WindowsAndMessaging::DispatchMessageW(&message);
            }
        }
    }

    struct TestWindow {
        hwnd: HWND,
        class_name: Vec<u16>,
        instance: HMODULE,
        _state: Arc<Mutex<Vec<u32>>>,
    }

    impl TestWindow {
        fn new(state: Arc<Mutex<Vec<u32>>>) -> Self {
            let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
            let class_name = wide_null("FastPadBootstrapQueueTest");
            let window_class = WNDCLASSW {
                lpfnWndProc: Some(test_window_proc),
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
                    Arc::into_raw(Arc::clone(&state)) as *const _,
                )
            };
            assert!(!hwnd.is_null());

            Self {
                hwnd,
                class_name,
                instance,
                _state: state,
            }
        }
    }

    impl Drop for TestWindow {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyWindow(self.hwnd);
                UnregisterClassW(self.class_name.as_ptr(), self.instance);
            }
        }
    }

    unsafe extern "system" fn test_window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            windows_sys::Win32::UI::WindowsAndMessaging::WM_NCCREATE => {
                let create = unsafe {
                    &*(lparam as *const windows_sys::Win32::UI::WindowsAndMessaging::CREATESTRUCTW)
                };
                unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW(
                        hwnd,
                        windows_sys::Win32::UI::WindowsAndMessaging::GWLP_USERDATA,
                        create.lpCreateParams as isize,
                    );
                }
                1
            }
            windows_sys::Win32::UI::WindowsAndMessaging::WM_NCDESTROY => {
                let raw = unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW(
                        hwnd,
                        windows_sys::Win32::UI::WindowsAndMessaging::GWLP_USERDATA,
                        0,
                    ) as *const Mutex<Vec<u32>>
                };
                if !raw.is_null() {
                    unsafe {
                        drop(Arc::from_raw(raw));
                    }
                }
                unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
            }
            _ => {
                let raw = unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(
                        hwnd,
                        windows_sys::Win32::UI::WindowsAndMessaging::GWLP_USERDATA,
                    ) as *const Mutex<Vec<u32>>
                };
                if !raw.is_null() {
                    unsafe {
                        (*raw).lock().unwrap().push(message);
                    }
                }
                0
            }
        }
    }
}
