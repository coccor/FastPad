use crate::app::{App, WindowIdentity};
use crate::editor::Editor;
use crate::error::StartupStage;
use crate::launch::LaunchOptions;
use crate::perf::{StartupMetrics, protocol::DiagnosticSession};
use crate::platform::{OwnedModule, last_error, wide_null};
use crate::window::{
    INPUT_MESSAGE_FIRST, INPUT_MESSAGE_LAST, MainWindowClass, WindowCreateContext,
    clear_input_priority, initialize_editor_with, input_priority_requested,
    input_queue_status_mask, maybe_post_deferred_start,
};
use crate::{FastPadError, Result};
#[cfg(test)]
use std::cell::RefCell;
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
#[cfg(test)]
use windows_sys::Win32::UI::WindowsAndMessaging::WM_QUIT;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, PM_REMOVE, PeekMessageW, SW_SHOW, ShowWindow,
    TranslateMessage, WM_PAINT,
};

pub fn run(options: LaunchOptions) -> Result<i32> {
    let mut startup = StartupMetrics::begin()?;
    let diagnostic = DiagnosticSession::attach(options.diagnostic)?;
    if let Some(diagnostic) = &diagnostic {
        startup.enable_diagnostic(std::rc::Rc::clone(diagnostic));
    }
    configure_dpi();
    let _scintilla = load_scintilla_module()
        .map_err(|error| FastPadError::startup(StartupStage::ScintillaLoad, error))?;
    let instance = current_module()?;
    let window_class = MainWindowClass::register(instance)
        .map_err(|error| FastPadError::startup(StartupStage::WindowClassRegistration, error))?;

    let app = App::new(options, startup);
    let identity = app.window_identity();
    let mut create_context = WindowCreateContext::new(Box::new(app));
    let hwnd = window_class.create(&mut create_context)?;
    if !identity.is_live_for(hwnd) {
        return Err(FastPadError::Invariant(
            "main window identity was not bound during creation",
        ));
    }
    let teardown = ParentWindowGuard::new(hwnd, identity.clone());

    let editor_hwnd = unsafe {
        initialize_editor_with(hwnd, &identity, |parent| {
            Editor::create(parent)
                .map_err(|error| FastPadError::startup(StartupStage::EditorCreate, error))
        })?
    };

    if let Some(diagnostic) = &diagnostic {
        if !identity.is_live_for(hwnd) {
            return Err(FastPadError::Invariant(
                "main window was destroyed before diagnostic hook installation",
            ));
        }
        diagnostic.install_input_hooks(hwnd, editor_hwnd)?;
        if !identity.is_live_for(hwnd) {
            return Err(FastPadError::Invariant(
                "main window was destroyed during diagnostic hook installation",
            ));
        }
    }

    unsafe {
        ShowWindow(hwnd, SW_SHOW);
    }
    if !identity.is_live_for(hwnd) {
        return Err(FastPadError::Invariant(
            "main window was destroyed before entering the message loop",
        ));
    }
    unsafe {
        SetFocus(editor_hwnd);
    }
    if !identity.is_live_for(hwnd) {
        return Err(FastPadError::Invariant(
            "main window was destroyed before entering the message loop",
        ));
    }

    finish_message_loop(hwnd, &identity, teardown, message_loop)
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

fn message_loop(hwnd: HWND, identity: &WindowIdentity) -> Result<i32> {
    let mut message = MSG::default();
    loop {
        drain_prioritized_input(hwnd, identity)?;
        let status = unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) };
        if status == -1 {
            return Err(last_error());
        }
        if status == 0 {
            return Ok(message.wParam as i32);
        }

        dispatch_message(hwnd, identity, &message);
    }
}

fn finish_message_loop<F>(
    hwnd: HWND,
    identity: &WindowIdentity,
    mut teardown: ParentWindowGuard,
    run_loop: F,
) -> Result<i32>
where
    F: FnOnce(HWND, &WindowIdentity) -> Result<i32>,
{
    let result = run_loop(hwnd, identity);
    if !identity.is_live_for(hwnd) {
        teardown.disarm();
    }
    drop(teardown);
    result
}

fn drain_prioritized_input(hwnd: HWND, identity: &WindowIdentity) -> Result<()> {
    if !identity.is_live_for(hwnd) || !unsafe { input_priority_requested(hwnd, identity) } {
        return Ok(());
    }

    while identity.is_live_for(hwnd) && dispatch_next_input_message(hwnd, identity)? {}
    if identity.is_live_for(hwnd) {
        unsafe {
            clear_input_priority(hwnd, identity);
        }
    }
    Ok(())
}

fn dispatch_next_input_message(hwnd: HWND, identity: &WindowIdentity) -> Result<bool> {
    let mut message = MSG::default();
    let queue_status = input_queue_status_mask();
    if unsafe {
        PeekMessageW(
            &mut message,
            std::ptr::null_mut(),
            INPUT_MESSAGE_FIRST,
            INPUT_MESSAGE_LAST,
            PM_REMOVE | (queue_status << 16),
        )
    } == 0
    {
        return Ok(false);
    }

    dispatch_message(hwnd, identity, &message);
    Ok(true)
}

fn dispatch_message(hwnd: HWND, identity: &WindowIdentity, message: &MSG) {
    #[cfg(test)]
    TEST_DISPATCHED_MESSAGES.with(|messages| messages.borrow_mut().push(message.message));

    unsafe {
        if crate::window::translate_accelerator(hwnd, identity, message) {
            return;
        }
        TranslateMessage(message);
        DispatchMessageW(message);
    }

    if !identity.is_live_for(hwnd) {
        return;
    }
    if message.hwnd == hwnd && message.message == WM_PAINT {
        unsafe {
            maybe_post_deferred_start(hwnd, identity);
        }
    }
}

#[cfg(test)]
thread_local! {
    static TEST_DISPATCHED_MESSAGES: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
}

#[cfg(test)]
fn pump_next_available_message(hwnd: HWND, identity: &WindowIdentity) -> Result<Option<u32>> {
    drain_prioritized_input(hwnd, identity)?;

    let mut message = MSG::default();
    if unsafe { PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) } == 0 {
        return Ok(None);
    }
    if message.message == WM_QUIT {
        return Ok(Some(WM_QUIT));
    }

    dispatch_message(hwnd, identity, &message);
    Ok(Some(message.message))
}

struct ParentWindowGuard {
    hwnd: HWND,
    identity: WindowIdentity,
    armed: bool,
}

impl ParentWindowGuard {
    fn new(hwnd: HWND, identity: WindowIdentity) -> Self {
        Self {
            hwnd,
            identity,
            armed: true,
        }
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
            if self.identity.is_live_for(self.hwnd) {
                let _ = windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(self.hwnd);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ParentWindowGuard, StartupStage, finish_message_loop, load_scintilla_module,
        pump_next_available_message,
    };
    use crate::app::{App, WindowIdentity};
    use crate::editor::Editor;
    use crate::error::FastPadError;
    use crate::launch::LaunchOptions;
    use crate::perf::StartupMetrics;
    use crate::window::{
        MainWindowClass, WindowCreateContext, initialize_editor_with, with_test_input_queue_status,
    };
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DestroyWindow, IsWindow, MSG, PM_REMOVE, PeekMessageW, PostMessageW, QS_POSTMESSAGE,
        WM_INPUT, WM_KEYDOWN, WM_LBUTTONDOWN,
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
    fn prioritized_input_drains_mixed_input_in_queue_order_before_reposted_work() {
        // Break caught: splitting keyboard and mouse drains can reorder queued input and let a
        // reposted deferred startup unit continue before the oldest pending input is dispatched.
        let main = ProductionWindow::new(make_app());
        pump_thread_messages();
        super::TEST_DISPATCHED_MESSAGES.with(|messages| messages.borrow_mut().clear());

        with_test_input_queue_status(QS_POSTMESSAGE, || {
            unsafe {
                PostMessageW(main.hwnd, crate::window::WM_FASTPAD_LOAD_SETTINGS, 0, 0);
                assert_ne!(PostMessageW(main.hwnd, WM_INPUT, 0, 0), 0);
                assert_ne!(PostMessageW(main.hwnd, WM_LBUTTONDOWN, 0, 0), 0);
                assert_ne!(PostMessageW(main.hwnd, WM_KEYDOWN, usize::from(b'X'), 0), 0);
            }

            pump_until_message(
                main.hwnd,
                &main.identity,
                crate::window::WM_FASTPAD_LOAD_SETTINGS,
            );
            assert!(unsafe { crate::window::input_priority_requested(main.hwnd, &main.identity) });
            pump_until_message(
                main.hwnd,
                &main.identity,
                crate::window::WM_FASTPAD_LOAD_SETTINGS,
            );
        });
        let mut queued = MSG::default();
        let load_settings_status = unsafe {
            PeekMessageW(
                &mut queued,
                std::ptr::null_mut(),
                crate::window::WM_FASTPAD_LOAD_SETTINGS,
                crate::window::WM_FASTPAD_LOAD_SETTINGS,
                PM_REMOVE,
            )
        };
        assert_eq!(load_settings_status, 0);
        let open_request_status = unsafe {
            PeekMessageW(
                &mut queued,
                std::ptr::null_mut(),
                crate::window::WM_FASTPAD_OPEN_REQUEST,
                crate::window::WM_FASTPAD_OPEN_REQUEST,
                PM_REMOVE,
            )
        };
        assert_ne!(open_request_status, 0);
        assert_eq!(queued.message, crate::window::WM_FASTPAD_OPEN_REQUEST);

        let input_messages = super::TEST_DISPATCHED_MESSAGES
            .with(|messages| messages.borrow().clone())
            .iter()
            .copied()
            .filter(|message| matches!(message, &WM_INPUT | &WM_LBUTTONDOWN | &WM_KEYDOWN))
            .collect::<Vec<_>>();
        assert_eq!(input_messages, vec![WM_INPUT, WM_LBUTTONDOWN, WM_KEYDOWN]);
        assert!(!unsafe { crate::window::input_priority_requested(main.hwnd, &main.identity) });
    }

    #[test]
    fn message_loop_failure_destroys_parent_before_return() {
        // Break caught: disarming the parent teardown guard before entering the fallible message
        // loop can leak a live HWND and unload Scintilla without running WM_NCDESTROY cleanup.
        let window = ProductionWindow::new(make_app());
        let teardown = ParentWindowGuard::new(window.hwnd, window.identity.clone());

        let error = finish_message_loop(
            window.hwnd,
            &window.identity,
            teardown,
            |_hwnd, _identity| Err(FastPadError::Invariant("expected message loop failure")),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            FastPadError::Invariant("expected message loop failure")
        ));
        assert_eq!(unsafe { IsWindow(window.hwnd) }, 0);
    }

    #[test]
    fn editor_initialization_rechecks_window_after_reentrant_creation() {
        // Break caught: retaining or reusing App state across reentrant Editor::create can install
        // an editor into window state that WM_NCDESTROY has already dropped.
        let _scintilla = load_scintilla_module().unwrap();
        let window = ProductionWindow::new(make_app());

        let error = unsafe {
            initialize_editor_with(window.hwnd, &window.identity, |parent| {
                let editor = Editor::create(parent)?;
                DestroyWindow(parent);
                Ok(editor)
            })
        }
        .unwrap_err();

        assert!(matches!(
            error,
            FastPadError::Invariant("main window was destroyed during editor initialization")
        ));
        assert_eq!(unsafe { IsWindow(window.hwnd) }, 0);
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

    fn pump_until_message(hwnd: HWND, identity: &WindowIdentity, expected: u32) {
        for _ in 0..32 {
            match pump_next_available_message(hwnd, identity).unwrap() {
                Some(message) if message == expected => return,
                Some(_) => {}
                None => panic!("message queue emptied before expected message {expected}"),
            }
        }
        panic!("expected message {expected} was not dispatched within 32 queue steps");
    }

    fn make_app() -> Box<App> {
        Box::new(App::new(
            LaunchOptions::default(),
            StartupMetrics::with_frequency(1, 0),
        ))
    }

    struct ProductionWindow {
        hwnd: HWND,
        identity: WindowIdentity,
        _class: MainWindowClass,
    }

    impl ProductionWindow {
        fn new(app: Box<App>) -> Self {
            let identity = app.window_identity();
            let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
            let class = MainWindowClass::register(instance).unwrap();
            let mut context = WindowCreateContext::new(app);
            let hwnd = class.create(&mut context).unwrap();
            Self {
                hwnd,
                identity,
                _class: class,
            }
        }
    }

    impl Drop for ProductionWindow {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }
}
