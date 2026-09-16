#![cfg(windows)]
mod support;

// Build the crate in-process with cfg(test), as json_commands.rs does, so tests can reach App.
include!("../../src/lib.rs");

use crate::document::Language;
use crate::preview::PreviewMode;
use crate::window::commands::CommandId;
use std::time::{Duration, Instant};
use support::acceptance::AcceptanceHarness;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetFocus, VK_ESCAPE};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, IsWindowVisible, MSG, PM_REMOVE, PeekMessageW, SendMessageW,
    TranslateMessage, WM_COMMAND, WM_KEYDOWN,
};

/// Direct2D, DirectWrite, and WIC must load only when a preview opens, never through the import
/// table: a static import would load them into every launch and slow cold startup.
#[test]
fn binary_does_not_statically_import_preview_graphics_libraries() {
    AcceptanceHarness::new().assert_no_preview_imports();
}

struct TestMain {
    hwnd: HWND,
    editor: HWND,
    identity: app::WindowIdentity,
    _class: window::MainWindowClass,
}

impl TestMain {
    fn new() -> Self {
        let app = app::App::new(
            launch::LaunchOptions::default(),
            perf::StartupMetrics::begin().unwrap(),
        );
        let identity = app.window_identity();
        let instance = unsafe {
            windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null())
        };
        let class = window::MainWindowClass::register(instance).unwrap();
        let mut context = window::WindowCreateContext::new(Box::new(app));
        let hwnd = class.create(&mut context).unwrap();
        let editor = unsafe {
            window::initialize_editor_with(hwnd, &identity, editor::Editor::create).unwrap()
        };
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::ShowWindow(
                hwnd,
                windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOW,
            );
        }
        Self {
            hwnd,
            editor,
            identity,
            _class: class,
        }
    }

    fn with_app<R>(&self, run: impl FnOnce(&mut app::App) -> R) -> R {
        assert!(self.identity.is_live_for(self.hwnd));
        let raw = unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(
                self.hwnd,
                windows_sys::Win32::UI::WindowsAndMessaging::GWLP_USERDATA,
            )
        } as *mut app::App;
        assert!(!raw.is_null());
        run(unsafe { &mut *raw })
    }

    fn command(&self, command: CommandId) {
        unsafe { SendMessageW(self.hwnd, WM_COMMAND, command as usize, 0) };
    }

    fn set_text(&self, text: &str) {
        let bytes = std::ffi::CString::new(text).unwrap();
        unsafe {
            SendMessageW(
                self.editor,
                crate::editor::scintilla_constants::SCI_SETTEXT,
                0,
                bytes.as_ptr() as isize,
            )
        };
    }

    /// Makes the active tab Markdown without loading Lexilla, then lets the host react.
    fn make_markdown(&self, text: &str) {
        self.set_text(text);
        self.with_app(|app| app.tabs.set_active_language(Language::Markdown));
        window::preview_host::sync_visibility(self.hwnd);
    }

    fn mode(&self) -> PreviewMode {
        self.with_app(|app| app.preview.mode)
    }

    fn view(&self) -> Option<preview::view::PreviewView> {
        self.with_app(|app| app.preview.view)
    }

    fn notices(&self) -> Vec<String> {
        self.with_app(|app| {
            app.notifications
                .pending()
                .iter()
                .map(|notice| notice.message.clone())
                .collect()
        })
    }
}

impl Drop for TestMain {
    fn drop(&mut self) {
        if self.identity.is_live_for(self.hwnd) {
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(self.hwnd) };
        }
    }
}

fn pump_until(what: &str, timeout: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    loop {
        let mut msg = MSG::default();
        while unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if condition() {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[allow(dead_code, reason = "used by the scroll sync tests")]
fn pump_for(duration: Duration) {
    let deadline = Instant::now() + duration;
    pump_until("pump_for", duration + Duration::from_secs(1), || {
        Instant::now() >= deadline
    });
}

fn visible(hwnd: HWND) -> bool {
    unsafe { IsWindowVisible(hwnd) != 0 }
}

#[test]
fn side_full_and_pressed_full_move_through_split_full_and_off() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    main.make_markdown("# Title\n\nBody\n");
    assert!(window::preview_host::buttons_visible(main.hwnd));

    window::preview_host::click_button(main.hwnd, window::titlebar::HitTarget::PreviewSide);
    assert_eq!(main.mode(), PreviewMode::Split);
    let view = main.view().expect("preview window");
    assert!(visible(view.hwnd()) && visible(main.editor));
    pump_until("first preview frame", Duration::from_secs(3), || {
        view.stats().block_count == 2
    });

    window::preview_host::click_button(main.hwnd, window::titlebar::HitTarget::PreviewFull);
    assert_eq!(main.mode(), PreviewMode::Full);
    assert!(!visible(main.editor));
    assert_eq!(unsafe { GetFocus() }, view.hwnd());

    window::preview_host::click_button(main.hwnd, window::titlebar::HitTarget::PreviewFull);
    assert_eq!(main.mode(), PreviewMode::Off);
    assert!(main.view().is_none());
    assert!(visible(main.editor));
    assert_eq!(unsafe { GetFocus() }, main.editor);
    assert_eq!(
        support::win32::scintilla_text(main.editor).unwrap(),
        "# Title\n\nBody\n"
    );
}

#[test]
fn ctrl_shift_v_cycles_off_split_full_off() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    main.make_markdown("text\n");
    for expected in [PreviewMode::Split, PreviewMode::Full, PreviewMode::Off] {
        main.command(CommandId::MarkdownPreviewCycle);
        assert_eq!(main.mode(), expected);
    }
}

#[test]
fn a_plain_text_tab_gets_a_notice_instead_of_a_preview() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    main.command(CommandId::MarkdownPreviewSide);
    assert_eq!(main.mode(), PreviewMode::Off);
    assert!(
        main.notices()
            .iter()
            .any(|notice| notice.contains("Markdown preview is available"))
    );
}

#[test]
fn a_non_markdown_tab_hides_the_preview_and_keeps_the_mode() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    main.make_markdown("# One\n");
    main.command(CommandId::MarkdownPreviewSide);
    let view = main.view().unwrap();
    main.command(CommandId::New);
    assert_eq!(main.mode(), PreviewMode::Split);
    assert!(!visible(view.hwnd()));
    assert!(!main.with_app(|app| app.tabs.view().snapshot().preview_buttons));
    main.command(CommandId::SelectTab1);
    assert!(visible(view.hwnd()));
    assert!(main.with_app(|app| app.tabs.view().snapshot().preview_buttons));
}

#[test]
fn escape_in_full_mode_returns_to_split() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    main.make_markdown("text\n");
    main.command(CommandId::MarkdownPreviewFull);
    let view = main.view().unwrap();
    unsafe { SendMessageW(view.hwnd(), WM_KEYDOWN, VK_ESCAPE as usize, 0) };
    pump_until("Esc handling", Duration::from_secs(2), || {
        main.mode() == PreviewMode::Split
    });
}
