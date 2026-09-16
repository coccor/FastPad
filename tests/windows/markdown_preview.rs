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
    DispatchMessageW, IsWindowVisible, MSG, PM_QS_INPUT, PM_REMOVE, PeekMessageW, SendMessageW,
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
        pump_pending();
        if condition() {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Dispatches everything queued, draining input before each other message as FastPad's own loop
/// prioritizes it. Posted messages are retrieved ahead of input, so a plain `PeekMessageW` loop
/// livelocks when real mouse or keyboard input reaches the (foreground) test window while a
/// deferred startup unit is queued: the unit sees input pending, reposts itself, and is retrieved
/// again before the input ever is.
fn pump_pending() {
    let mut msg = MSG::default();
    loop {
        while unsafe {
            PeekMessageW(
                &mut msg,
                std::ptr::null_mut(),
                0,
                0,
                PM_REMOVE | PM_QS_INPUT,
            )
        } != 0
        {
            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) } == 0 {
            return;
        }
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

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

fn type_text(hwnd: HWND, text: &str) {
    for unit in text.encode_utf16() {
        unsafe {
            SendMessageW(
                hwnd,
                windows_sys::Win32::UI::WindowsAndMessaging::WM_CHAR,
                unit as usize,
                0,
            )
        };
    }
}

#[test]
fn typing_updates_the_preview_only_after_the_pause() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    main.make_markdown("# A\n");
    main.command(CommandId::MarkdownPreviewSide);
    let view = main.view().unwrap();
    pump_until("initial render", Duration::from_secs(3), || {
        view.stats().block_count == 1
    });
    unsafe {
        SendMessageW(
            main.editor,
            crate::editor::scintilla_constants::SCI_DOCUMENTEND,
            0,
            0,
        );
    }
    type_text(main.editor, "\n\npara");
    assert_eq!(view.stats().block_count, 1, "SCN_MODIFIED must not parse");
    pump_until("debounced update", Duration::from_secs(3), || {
        view.stats().block_count == 2
    });
}

#[test]
fn theme_changes_recolor_the_preview() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    main.make_markdown("# A\n");
    main.command(CommandId::MarkdownPreviewSide);
    main.command(CommandId::ThemeCatppuccinMocha);
    let view = main.view().unwrap();
    assert_eq!(view.colors().link, crate::catppuccin::MOCHA.blue);
}

#[test]
fn relative_markdown_links_open_in_a_tab() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let dir = std::env::temp_dir().join(format!("fastpad-preview-links-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.md"), "[next](b.md)\n").unwrap();
    std::fs::write(dir.join("b.md"), "# B\n").unwrap();
    let main = TestMain::new();
    window::open_path(main.hwnd, &dir.join("a.md")).unwrap();
    pump_until("a.md loaded", Duration::from_secs(3), || {
        !main.with_app(|app| app.populating_file)
    });
    main.with_app(|app| app.tabs.set_active_language(Language::Markdown));
    main.command(CommandId::MarkdownPreviewSide);
    let payload = Box::into_raw(Box::new(String::from("b.md")));
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
            main.hwnd,
            window::WM_FASTPAD_PREVIEW_LINK,
            0,
            payload as isize,
        );
    }
    pump_until("b.md tab", Duration::from_secs(3), || {
        main.with_app(|app| app.tabs.active().and_then(|document| document.path.clone()))
            == Some(dir.join("b.md"))
    });
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unsupported_links_explain_themselves() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    main.make_markdown("[x](ftp://x.dev)\n");
    main.command(CommandId::MarkdownPreviewSide);
    let payload = Box::into_raw(Box::new(String::from("ftp://x.dev")));
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
            main.hwnd,
            window::WM_FASTPAD_PREVIEW_LINK,
            0,
            payload as isize,
        );
    }
    pump_until("link notice", Duration::from_secs(2), || {
        main.notices()
            .iter()
            .any(|notice| notice.contains("does not open this kind of link"))
    });
}

#[test]
fn large_documents_parse_on_a_worker_and_huge_ones_pause() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    let paragraph = "Paragraph text for the worker parse.\n\n";
    main.make_markdown(&paragraph.repeat(2_000_000 / paragraph.len()));
    main.command(CommandId::MarkdownPreviewSide);
    let view = main.view().unwrap();
    pump_until("worker parse", Duration::from_secs(10), || {
        view.stats().block_count > 1000
    });
    assert!(!view.is_paused());

    main.command(CommandId::MarkdownPreviewClose);
    main.make_markdown(&paragraph.repeat(11_000_000 / paragraph.len()));
    main.command(CommandId::MarkdownPreviewSide);
    let view = main.view().unwrap();
    assert!(view.is_paused());
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
            main.hwnd,
            window::WM_FASTPAD_PREVIEW_REFRESH,
            0,
            0,
        );
    }
    pump_until("refresh parse", Duration::from_secs(20), || {
        view.stats().block_count > 1000
    });
}

fn long_markdown() -> String {
    (0..400)
        .map(|index| {
            format!(
                "Paragraph {index}

"
            )
        })
        .collect()
}

#[test]
fn scrolling_the_editor_scrolls_the_preview() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    main.make_markdown(&long_markdown());
    main.command(CommandId::MarkdownPreviewSide);
    let view = main.view().unwrap();
    pump_until("render", Duration::from_secs(3), || {
        view.stats().block_count == 400
    });
    unsafe {
        SendMessageW(
            main.editor,
            crate::editor::scintilla_constants::SCI_SETFIRSTVISIBLELINE,
            300,
            0,
        );
        windows_sys::Win32::Graphics::Gdi::UpdateWindow(main.editor);
    }
    pump_until("preview follows", Duration::from_secs(3), || {
        view.top_line() >= 280
    });
}

#[test]
fn scrolling_the_preview_scrolls_the_editor_without_echo() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    main.make_markdown(&long_markdown());
    main.command(CommandId::MarkdownPreviewSide);
    let view = main.view().unwrap();
    pump_until("render", Duration::from_secs(3), || {
        view.stats().block_count == 400
    });
    let before = main.with_app(|app| app.preview.sync_count);
    unsafe {
        SendMessageW(
            view.hwnd(),
            WM_KEYDOWN,
            windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_NEXT as usize,
            0,
        );
    }
    pump_until("editor follows", Duration::from_secs(3), || {
        (unsafe {
            SendMessageW(
                main.editor,
                crate::editor::scintilla_constants::SCI_GETFIRSTVISIBLELINE,
                0,
                0,
            )
        }) > 0
    });
    pump_for(Duration::from_millis(250));
    let syncs = main.with_app(|app| app.preview.sync_count) - before;
    assert_eq!(
        syncs, 1,
        "scroll sync echoed: {syncs} syncs for one preview scroll"
    );
}

#[test]
fn closing_the_find_bar_in_full_mode_focuses_the_preview() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    main.make_markdown("# A\n");
    main.command(CommandId::MarkdownPreviewFull);
    let view = main.view().unwrap();
    main.command(CommandId::Find);
    let query = main.with_app(|app| app.find_bar.as_ref().unwrap().query_hwnd());
    assert_eq!(unsafe { GetFocus() }, query);
    unsafe { SendMessageW(query, WM_KEYDOWN, VK_ESCAPE as usize, 0) };
    assert_eq!(unsafe { GetFocus() }, view.hwnd());
}

#[test]
fn losing_capture_ends_a_divider_drag() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    main.make_markdown("# A\n");
    main.command(CommandId::MarkdownPreviewSide);
    let divider = window::preview_host::divider_rect(main.hwnd).expect("divider");
    assert!(window::preview_host::begin_divider_drag(
        main.hwnd,
        divider.left,
        divider.top
    ));
    assert!(window::preview_host::drag_divider(main.hwnd, divider.left));
    unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture() };
    assert!(!window::preview_host::drag_divider(main.hwnd, divider.left));
}

#[test]
fn menu_mode_keeps_the_frame_focus_in_full_mode() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    main.make_markdown("# A\n");
    main.command(CommandId::MarkdownPreviewFull);
    // A tapped F10 enters menu mode, which parks the focus on the frame for the menu keys.
    unsafe {
        SendMessageW(
            main.hwnd,
            windows_sys::Win32::UI::WindowsAndMessaging::WM_SYSCOMMAND,
            windows_sys::Win32::UI::WindowsAndMessaging::SC_KEYMENU as usize,
            0,
        )
    };
    assert!(main.with_app(|app| app.menu_mode.is_some()));
    assert_eq!(unsafe { GetFocus() }, main.hwnd);
}

#[test]
fn full_mode_keeps_the_editor_position() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    main.make_markdown(&long_markdown());
    unsafe {
        SendMessageW(
            main.editor,
            crate::editor::scintilla_constants::SCI_SETFIRSTVISIBLELINE,
            100,
            0,
        )
    };
    main.command(CommandId::MarkdownPreviewFull);
    let view = main.view().unwrap();
    unsafe {
        SendMessageW(
            view.hwnd(),
            WM_KEYDOWN,
            windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_END as usize,
            0,
        )
    };
    pump_for(Duration::from_millis(300));
    main.command(CommandId::MarkdownPreviewSide);
    assert_eq!(
        unsafe {
            SendMessageW(
                main.editor,
                crate::editor::scintilla_constants::SCI_GETFIRSTVISIBLELINE,
                0,
                0,
            )
        },
        100
    );
}
