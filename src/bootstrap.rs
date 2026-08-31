use crate::app::App;
use crate::editor::Editor;
use crate::error::StartupStage;
use crate::launch::LaunchOptions;
use crate::perf::{Milestone, StartupMetrics};
use crate::platform::{OwnedModule, last_error, wide_null};
use crate::window::{MainWindowClass, maybe_post_deferred_start};
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
    DispatchMessageW, GetMessageW, MSG, SW_SHOW, ShowWindow, TranslateMessage, WM_PAINT,
};

pub fn run(options: LaunchOptions) -> Result<i32> {
    let startup = StartupMetrics::begin()?;
    configure_dpi();
    let _scintilla = load_scintilla_module()
        .map_err(|error| FastPadError::startup(StartupStage::ScintillaLoad, error))?;
    let instance = current_module()?;
    let window_class = MainWindowClass::register(instance)
        .map_err(|error| FastPadError::startup(StartupStage::WindowClassRegistration, error))?;

    let app = Box::new(App::new(options, startup));
    let app_ptr = Box::into_raw(app);
    let hwnd = match window_class.create(app_ptr.cast()) {
        Ok(hwnd) => hwnd,
        Err(error) => {
            unsafe {
                drop(Box::from_raw(app_ptr));
            }
            return Err(error);
        }
    };

    unsafe {
        let app = &mut *app_ptr;
        let _ = app.startup.record_now(Milestone::WindowCreated);
        let editor = Editor::create(hwnd)
            .map_err(|error| FastPadError::startup(StartupStage::EditorCreate, error))?;
        let _ = app.startup.record_now(Milestone::EditorCreated);
        app.editor = Some(editor);
    }

    unsafe {
        ShowWindow(hwnd, SW_SHOW);
        if let Some(editor) = (&*app_ptr).editor.as_ref() {
            SetFocus(editor.hwnd());
        }
    }

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

#[cfg(test)]
mod tests {
    use super::StartupStage;

    #[test]
    fn startup_stage_exit_codes_match_bootstrap_contract() {
        // Break caught: changing fatal startup exit codes breaks the shell's distinct user-facing
        // failures for Scintilla loading, class registration, and editor creation.
        assert_eq!(StartupStage::ScintillaLoad.exit_code(), 10);
        assert_eq!(StartupStage::WindowClassRegistration.exit_code(), 11);
        assert_eq!(StartupStage::EditorCreate.exit_code(), 12);
    }
}
