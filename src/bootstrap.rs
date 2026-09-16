use crate::app::{App, WindowIdentity};
use crate::editor::Editor;
use crate::error::StartupStage;
use crate::ipc::client::InstanceClaim;
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
use std::path::{Path, PathBuf};
use windows_sys::Win32::Foundation::{HMODULE, HWND, WAIT_FAILED, WAIT_OBJECT_0};
use windows_sys::Win32::System::LibraryLoader::{
    GetModuleHandleW, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
    LoadLibraryExW,
};
use windows_sys::Win32::System::Threading::INFINITE;
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx,
    PM_REMOVE, PeekMessageW, QS_ALLINPUT, SW_SHOW, ShowWindow, TranslateMessage, WM_PAINT, WM_QUIT,
};

pub fn run(options: LaunchOptions) -> Result<i32> {
    let mut startup = StartupMetrics::begin()?;
    // Before DPI and Scintilla so a forwarding secondary loads nothing it will not use.
    let instance_mutex = if options.new_window {
        None
    } else {
        match crate::ipc::client::claim_or_forward(&options.request) {
            InstanceClaim::Primary(mutex) => Some(mutex),
            InstanceClaim::Forwarded => return Ok(0),
            InstanceClaim::Independent => None,
        }
    };
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

    let mut app = App::new(options, startup);
    app.instance_mutex = instance_mutex;
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
    let path = native_scintilla_path().ok_or(FastPadError::Invariant(
        "could not locate the directory of the running executable",
    ))?;
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

const SCINTILLA_DLL: &str = "Scintilla.dll";

fn native_scintilla_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok();
    let dev_dir = dev_scintilla_dir();
    select_scintilla_path(
        exe.as_deref().and_then(Path::parent),
        dev_dir.as_deref(),
        Path::is_file,
    )
}

// Unpackaged dev, test, and benchmark runs have no DLL beside target\<profile>\fastpad.exe.
#[cfg(not(feature = "release-package"))]
fn dev_scintilla_dir() -> Option<PathBuf> {
    Some(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("native")
            .join("out")
            .join("x64"),
    )
}

#[cfg(feature = "release-package")]
fn dev_scintilla_dir() -> Option<PathBuf> {
    None
}

fn select_scintilla_path(
    exe_dir: Option<&Path>,
    dev_dir: Option<&Path>,
    exists: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let staged = exe_dir.map(|dir| dir.join(SCINTILLA_DLL));
    if let Some(path) = &staged
        && exists(path)
    {
        return staged;
    }
    dev_dir.map(|dir| dir.join(SCINTILLA_DLL)).or(staged)
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
        if !next_message(hwnd, identity, &mut message)? {
            continue;
        }
        if message.message == WM_QUIT {
            return Ok(message.wParam as i32);
        }

        dispatch_message(hwnd, identity, &message);
    }
}

/// Fills `message` and returns true, or returns false after a wake that only serviced the pipe.
fn next_message(hwnd: HWND, identity: &WindowIdentity, message: &mut MSG) -> Result<bool> {
    // The event handle is copied fresh for each wait and nothing is dispatched while it is used.
    let Some(event) = crate::window::ipc_wait_handle(hwnd, identity) else {
        if unsafe { GetMessageW(message, std::ptr::null_mut(), 0, 0) } == -1 {
            return Err(last_error());
        }
        return Ok(true);
    };
    let wait = unsafe {
        MsgWaitForMultipleObjectsEx(1, &event, INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE)
    };
    if wait == WAIT_FAILED {
        return Err(last_error());
    }
    if wait == WAIT_OBJECT_0 {
        crate::window::service_ipc(hwnd, identity);
        return Ok(false);
    }
    Ok(unsafe { PeekMessageW(message, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0)
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
    use std::path::PathBuf;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWLP_USERDATA, GetWindowLongPtrW, SendMessageW, WM_CHAR,
    };

    #[test]
    fn launch_open_loads_into_the_initial_tab_without_waiting_for_input() {
        // Break caught: the launch file waits for a typed character before loading, which also
        // stalls every later deferred unit, and opening it beside the untouched initial document
        // leaves a stray "Untitled *" tab behind.
        let _scintilla = load_scintilla_module().unwrap();
        let fixture = OpenFixture::new(b"\xEF\xBB\xBF{\"ok\":true}");
        let mut app = make_app();
        app.launch.request =
            crate::launch::LaunchRequest::Open(fixture.path.clone().into_os_string());
        let main = ProductionWindow::new(app);
        let editor =
            unsafe { initialize_editor_with(main.hwnd, &main.identity, Editor::create).unwrap() };

        unsafe {
            PostMessageW(main.hwnd, crate::window::WM_FASTPAD_OPEN_REQUEST, 0, 0);
        }
        pump_thread_messages();

        with_app(main.hwnd, |app| {
            assert_eq!(
                app.editor.as_ref().unwrap().text().unwrap(),
                "{\"ok\":true}"
            );
            assert_eq!(
                app.tabs.active().path.as_deref(),
                Some(fixture.path.as_path())
            );
            assert_eq!(
                app.tabs.active().encoding,
                crate::file::encoding::Encoding::Utf8Bom
            );
            assert!(!app.tabs.active().dirty);
            assert_eq!(
                app.tabs.len(),
                1,
                "the launch file must replace the initial empty tab"
            );
            assert!(
                app.startup
                    .micros(crate::perf::Milestone::FileLoaded)
                    .is_some()
            );
        });
        assert_eq!(
            unsafe {
                SendMessageW(
                    editor,
                    crate::editor::scintilla_constants::SCI_CANUNDO,
                    0,
                    0,
                )
            },
            0
        );
        unsafe {
            SendMessageW(editor, WM_CHAR, b'!' as usize, 0);
        }
        assert_ne!(
            unsafe {
                SendMessageW(
                    editor,
                    crate::editor::scintilla_constants::SCI_CANUNDO,
                    0,
                    0,
                )
            },
            0
        );
    }

    #[test]
    fn launch_open_reposts_itself_while_input_is_pending() {
        // Break caught: the launch open runs ahead of queued input instead of yielding to it the
        // way every other deferred startup unit does.
        let _scintilla = load_scintilla_module().unwrap();
        let fixture = OpenFixture::new(b"not yet");
        let mut app = make_app();
        app.launch.request =
            crate::launch::LaunchRequest::Open(fixture.path.clone().into_os_string());
        let main = ProductionWindow::new(app);
        unsafe {
            initialize_editor_with(main.hwnd, &main.identity, Editor::create).unwrap();
        }

        with_test_input_queue_status(QS_POSTMESSAGE, || {
            unsafe {
                assert_ne!(PostMessageW(main.hwnd, WM_KEYDOWN, usize::from(b'X'), 0), 0);
                SendMessageW(main.hwnd, crate::window::WM_FASTPAD_OPEN_REQUEST, 0, 0);
            }

            with_app(main.hwnd, |app| {
                assert_eq!(app.editor.as_ref().unwrap().text().unwrap(), "");
                assert!(
                    app.startup
                        .micros(crate::perf::Milestone::FileLoaded)
                        .is_none()
                );
            });
            let mut queued = MSG::default();
            assert_ne!(
                unsafe {
                    PeekMessageW(
                        &mut queued,
                        main.hwnd,
                        crate::window::WM_FASTPAD_OPEN_REQUEST,
                        crate::window::WM_FASTPAD_OPEN_REQUEST,
                        PM_REMOVE,
                    )
                },
                0,
                "the launch open must repost itself while input is pending"
            );
        });
    }

    #[test]
    fn app_construction_and_path_loading_do_not_initialize_com() {
        // Break caught: application-owned COM initialization moves into App or file loading.
        use windows_sys::Win32::System::Com::{APTTYPE, APTTYPEQUALIFIER, CoGetApartmentType};
        fn apartment() -> (i32, APTTYPE, APTTYPEQUALIFIER) {
            let mut kind = 0;
            let mut qualifier = 0;
            let status = unsafe { CoGetApartmentType(&mut kind, &mut qualifier) };
            (status, kind, qualifier)
        }
        let before = apartment();
        let fixture = OpenFixture::new(b"same");
        let plain = make_app();
        crate::file::loader::load(&fixture.path).unwrap();
        assert_eq!(apartment(), before);
        drop(plain);
        let _scintilla = load_scintilla_module().unwrap();
        let main = ProductionWindow::new(make_app());
        unsafe {
            initialize_editor_with(main.hwnd, &main.identity, Editor::create).unwrap();
        }
        // Native UI can establish an OS-owned implicit apartment; that is not our explicit COM init.
        let native_baseline = apartment();
        App::open_path(main.hwnd, &fixture.path).unwrap();
        assert_eq!(apartment(), native_baseline);
    }

    #[test]
    fn open_reuses_empty_tab_and_duplicate_without_reloading_dirty_text() {
        // Break caught: duplicate opens establish competing native document ownership.
        let _scintilla = load_scintilla_module().unwrap();
        let fixture = OpenFixture::new(b"same");
        let main = ProductionWindow::new(make_app());
        let editor =
            unsafe { initialize_editor_with(main.hwnd, &main.identity, Editor::create).unwrap() };
        App::open_path(main.hwnd, &fixture.path).unwrap();
        with_app(main.hwnd, |app| assert_eq!(app.tabs.len(), 1));
        unsafe {
            SendMessageW(editor, WM_CHAR, b'!' as usize, 0);
        }
        let before = with_app(main.hwnd, |app| {
            app.editor.as_ref().unwrap().text().unwrap()
        });
        App::open_path(
            main.hwnd,
            &fixture.path.parent().unwrap().join(".").join("config.json"),
        )
        .unwrap();
        with_app(main.hwnd, |app| {
            assert_eq!(app.tabs.len(), 1);
            assert_eq!(app.editor.as_ref().unwrap().text().unwrap(), before);
            assert!(app.tabs.active().dirty);
        });
    }

    #[test]
    fn successful_launch_open_posts_language_continuation_only_once() {
        // Break caught: both the loader and its deferred request handler advance the chain.
        let _scintilla = load_scintilla_module().unwrap();
        let fixture = OpenFixture::new(b"same");
        let mut app = make_app();
        app.launch.request =
            crate::launch::LaunchRequest::Open(fixture.path.clone().into_os_string());
        let main = ProductionWindow::new(app);
        let editor =
            unsafe { initialize_editor_with(main.hwnd, &main.identity, Editor::create).unwrap() };
        unsafe {
            SendMessageW(main.hwnd, crate::window::WM_FASTPAD_OPEN_REQUEST, 0, 0);
            SendMessageW(main.hwnd, crate::window::WM_FASTPAD_OPEN_REQUEST, 0, 0);
        }
        let mut count = 0;
        let mut message = MSG::default();
        while unsafe {
            PeekMessageW(
                &mut message,
                main.hwnd,
                crate::window::WM_FASTPAD_APPLY_LANGUAGE,
                crate::window::WM_FASTPAD_APPLY_LANGUAGE,
                PM_REMOVE,
            )
        } != 0
        {
            count += 1;
        }
        assert_eq!(count, 1);
    }

    #[test]
    fn open_errors_leave_active_document_and_native_text_unchanged() {
        // Break caught: creating/switching a tab before successful decode loses active state.
        let _scintilla = load_scintilla_module().unwrap();
        let fixture = OpenFixture::new(&[0x80]);
        let main = ProductionWindow::new(make_app());
        let editor =
            unsafe { initialize_editor_with(main.hwnd, &main.identity, Editor::create).unwrap() };
        unsafe {
            SendMessageW(editor, WM_CHAR, b'x' as usize, 0);
        }
        let before = with_app(main.hwnd, |app| {
            (
                app.tabs.active().id,
                app.tabs.active().generation,
                app.tabs.active().dirty,
            )
        });
        assert!(App::open_path(main.hwnd, &fixture.path).is_err());
        assert!(App::open_path(main.hwnd, &fixture.path.with_extension("missing")).is_err());
        std::fs::write(&fixture.path, b"nul\0text").unwrap();
        assert!(App::open_path(main.hwnd, &fixture.path).is_err());
        with_app(main.hwnd, |app| {
            assert_eq!(app.tabs.len(), 1);
            assert_eq!(
                (
                    app.tabs.active().id,
                    app.tabs.active().generation,
                    app.tabs.active().dirty
                ),
                before
            );
            assert_eq!(app.editor.as_ref().unwrap().text().unwrap(), "x");
            assert!(app.tabs.active().path.is_none());
        });
    }

    fn with_app<R>(hwnd: HWND, run: impl FnOnce(&App) -> R) -> R {
        let raw = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const App;
        assert!(!raw.is_null());
        run(unsafe { &*raw })
    }

    struct OpenFixture {
        directory: PathBuf,
        path: PathBuf,
    }
    impl OpenFixture {
        fn new(bytes: &[u8]) -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let directory = std::env::temp_dir().join(format!(
                "fastpad-open-native-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir(&directory).unwrap();
            let path = directory.join("config.json");
            std::fs::write(&path, bytes).unwrap();
            Self { directory, path }
        }
    }
    impl Drop for OpenFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }
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
    fn scintilla_path_prefers_the_executable_directory_over_the_dev_fallback() {
        // Break caught: a shipped FastPad.exe loading Scintilla.dll from a build-machine checkout
        // path instead of the DLL staged next to it in the portable package.
        use super::select_scintilla_path;
        use std::path::Path;
        let exe_dir = Path::new(r"C:\portable\FastPad");
        let dev_dir = Path::new(r"D:\checkout\native\out\x64");
        let staged = exe_dir.join("Scintilla.dll");

        assert_eq!(
            select_scintilla_path(Some(exe_dir), Some(dev_dir), |path| path == staged),
            Some(staged.clone())
        );
        assert_eq!(
            select_scintilla_path(Some(exe_dir), Some(dev_dir), |_| false),
            Some(dev_dir.join("Scintilla.dll"))
        );
        assert_eq!(
            select_scintilla_path(Some(exe_dir), None, |_| false),
            Some(staged)
        );
        assert_eq!(select_scintilla_path(None, None, |_| true), None);
    }

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
