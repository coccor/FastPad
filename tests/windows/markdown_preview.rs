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
    pump_until("first render", Duration::from_secs(3), || {
        view.stats().block_count == 1
    });
    main.command(CommandId::New);
    assert_eq!(main.mode(), PreviewMode::Split);
    assert!(!visible(view.hwnd()));
    assert!(!main.with_app(|app| app.tabs.view().snapshot().preview_buttons));
    assert_eq!(
        view.stats().block_count,
        0,
        "a hidden preview releases its document"
    );
    main.command(CommandId::SelectTab1);
    assert!(visible(view.hwnd()));
    assert!(main.with_app(|app| app.tabs.view().snapshot().preview_buttons));
    pump_until("reloaded render", Duration::from_secs(3), || {
        view.stats().block_count == 1
    });
}

#[test]
fn a_large_document_never_shows_the_previous_tabs_preview_while_it_parses() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    let paragraph = "Paragraph text for the worker parse.\n\n";
    main.make_markdown(&paragraph.repeat(1_100_000 / paragraph.len()));
    main.command(CommandId::MarkdownPreviewSide);
    let view = main.view().unwrap();
    pump_until("worker parse", Duration::from_secs(10), || {
        view.stats().block_count > 1000
    });
    main.command(CommandId::New);
    main.make_markdown("# Small\n\n[link](https://x.dev)\n");
    pump_until("small render", Duration::from_secs(3), || {
        view.stats().block_count == 2 && !view.accessible_links().read().unwrap().is_empty()
    });
    main.command(CommandId::SelectTab1);
    assert_eq!(
        view.stats().block_count,
        0,
        "the previous tab's blocks stood in for the parsing document"
    );
    assert!(view.visible_links().is_empty());
    assert!(view.accessible_links().read().unwrap().is_empty());
    pump_until("worker parse again", Duration::from_secs(10), || {
        view.stats().block_count > 1000
    });
}

#[test]
fn full_mode_reloads_keep_the_preview_position() {
    // Break caught: reloading in Full mode jumped the preview to the hidden editor's top line.
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    main.make_markdown(&long_markdown());
    main.command(CommandId::MarkdownPreviewFull);
    let view = main.view().unwrap();
    pump_until("render", Duration::from_secs(3), || {
        view.stats().block_count == 400
    });
    unsafe {
        SendMessageW(
            view.hwnd(),
            WM_KEYDOWN,
            windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_NEXT as usize,
            0,
        );
        SendMessageW(
            view.hwnd(),
            WM_KEYDOWN,
            windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_NEXT as usize,
            0,
        );
    }
    pump_for(Duration::from_millis(100));
    let before = view.top_line();
    assert!(before > 0);
    window::preview_host::refresh(main.hwnd);
    pump_for(Duration::from_millis(100));
    assert_eq!(view.top_line(), before, "inline reparse");

    let paragraph = "Paragraph text for the worker parse.\n\n";
    main.set_text(&paragraph.repeat(1_100_000 / paragraph.len()));
    window::preview_host::document_reloaded(main.hwnd);
    pump_until("worker parse", Duration::from_secs(10), || {
        view.stats().block_count > 1000
    });
    for _ in 0..3 {
        unsafe {
            SendMessageW(
                view.hwnd(),
                WM_KEYDOWN,
                windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_NEXT as usize,
                0,
            )
        };
    }
    pump_for(Duration::from_millis(100));
    let before = view.top_line();
    assert!(before > 0);
    window::preview_host::refresh(main.hwnd);
    assert_eq!(
        view.stats().block_count,
        0,
        "the worker parse is outstanding"
    );
    pump_until("worker reparse", Duration::from_secs(10), || {
        view.stats().block_count > 1000
    });
    assert_eq!(view.top_line(), before, "worker reparse");
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
    let preview_line = view.top_line();
    assert!(preview_line > 0);
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
    assert!(
        syncs <= 1,
        "scroll sync echoed: {syncs} syncs for one preview scroll"
    );
    assert_eq!(
        view.top_line(),
        preview_line,
        "the editor's echo moved the preview"
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

fn sample_markdown(bytes: usize) -> String {
    let section = "## Heading\n\nParagraph with **bold**, *emphasis*, `code`, and a [link](https://x.dev).\n\n- item one\n- item two\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n```rust\nfn f() {}\n```\n\n";
    section.repeat(bytes / section.len() + 1)[..bytes]
        .rsplit_once("\n\n")
        .map_or_else(String::new, |(text, _)| format!("{text}\n"))
}

fn p95(mut samples: Vec<u64>) -> u64 {
    samples.sort_unstable();
    samples[(samples.len() * 95 / 100).min(samples.len() - 1)]
}

#[test]
#[ignore = "performance measurement: cargo test --release --test markdown_preview -- --ignored --test-threads=1"]
fn opening_a_100_kb_preview_renders_within_50_ms_p95() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    main.make_markdown(&sample_markdown(100_000));
    let mut samples = Vec::new();
    for _ in 0..30 {
        main.command(CommandId::MarkdownPreviewSide);
        let view = main.view().unwrap();
        pump_until("first frame", Duration::from_secs(5), || {
            view.stats().first_frame_micros > 0
        });
        samples.push(view.stats().first_frame_micros);
        main.command(CommandId::MarkdownPreviewClose);
    }
    let p95 = p95(samples);
    println!("preview open p95: {p95} us");
    assert!(p95 < 50_000);
}

#[test]
#[ignore = "performance measurement: cargo test --release --test markdown_preview -- --ignored --test-threads=1"]
fn one_paragraph_updates_in_a_1_mb_document_within_2_ms_p95() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    main.make_markdown(&sample_markdown(1_000_000));
    main.command(CommandId::MarkdownPreviewSide);
    let view = main.view().unwrap();
    pump_until("initial render", Duration::from_secs(10), || {
        view.stats().block_count > 0
    });
    unsafe {
        SendMessageW(
            main.editor,
            crate::editor::scintilla_constants::SCI_GOTOPOS,
            500_000,
            0,
        )
    };
    pump_for(Duration::from_millis(300));
    // Edit a line the synced preview actually shows: an off-screen edit correctly skips the
    // repaint, which would leave `last_update_micros` holding the initial full render.
    unsafe {
        let line_start = SendMessageW(
            main.editor,
            crate::editor::scintilla_constants::SCI_POSITIONFROMLINE,
            view.top_line() + 1,
            0,
        );
        SendMessageW(
            main.editor,
            crate::editor::scintilla_constants::SCI_GOTOPOS,
            line_start as usize,
            0,
        );
    }
    let mut samples = Vec::new();
    for _ in 0..50 {
        let revision = view.stats().revision;
        let painted = view.stats().painted_updates;
        type_text(main.editor, "x");
        pump_until("update", Duration::from_secs(5), || {
            view.stats().revision > revision && view.stats().painted_updates > painted
        });
        pump_for(Duration::from_millis(20));
        samples.push(view.stats().last_update_micros);
    }
    let p95 = p95(samples);
    println!("incremental update p95: {p95} us");
    assert!(p95 < 2_000);
}

#[test]
#[ignore = "performance measurement: cargo test --release --test markdown_preview -- --ignored --test-threads=1"]
fn typing_with_split_open_costs_the_same_as_without_a_preview() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    main.make_markdown(&sample_markdown(1_000_000));
    unsafe {
        SendMessageW(
            main.editor,
            crate::editor::scintilla_constants::SCI_GOTOPOS,
            500_000,
            0,
        )
    };
    // Each run starts on a fresh line and breaks the line every 40 characters (unmeasured): one
    // ever-growing line would make later samples pay for re-laying out a longer line and for
    // horizontal caret scrolling in the narrower split editor, which is not the preview's cost.
    let measure = |main: &TestMain| {
        let mut samples = Vec::new();
        type_text(main.editor, "\n\n");
        for index in 0..200 {
            if index % 40 == 0 {
                type_text(main.editor, "\n");
            }
            let started = Instant::now();
            type_text(main.editor, "x");
            unsafe { windows_sys::Win32::Graphics::Gdi::UpdateWindow(main.editor) };
            samples.push(started.elapsed().as_micros() as u64);
        }
        p95(samples)
    };
    let baseline = measure(&main);
    main.command(CommandId::MarkdownPreviewSide);
    pump_for(Duration::from_millis(500));
    let with_preview = measure(&main);
    println!("keystroke p95: off {baseline} us, split {with_preview} us");
    assert!(with_preview <= baseline + baseline / 10 + 100);
}

#[test]
#[ignore = "performance measurement: cargo test --release --test markdown_preview -- --ignored --test-threads=1"]
fn private_bytes_do_not_grow_across_open_close_cycles() {
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    fn private_bytes() -> u64 {
        let mut counters = PROCESS_MEMORY_COUNTERS_EX {
            cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
            ..Default::default()
        };
        unsafe {
            GetProcessMemoryInfo(
                GetCurrentProcess(),
                (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX).cast(),
                counters.cb,
            )
        };
        counters.PrivateUsage as u64
    }
    // The first open loads Direct2D, DirectWrite, Direct3D, and the GPU driver for the session:
    // about 45 MB of private bytes that closing does not return. Unloading them on close still
    // left about 21 MB and made every reopen cost about 120 ms. Closing must return everything
    // else, so repeated open/close cycles may not grow. Two warm-up cycles come first (the heap
    // settles on the first reopen: a one-time step of up to about 2 MB with a 1 MB document), then
    // a reference cycle. Single samples jitter by up to about 3 MB, so the median of the next five
    // closes is compared with the reference.
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    main.make_markdown(&sample_markdown(1_000_000));
    pump_for(Duration::from_millis(300));
    let never_opened = private_bytes();
    let cycle = || {
        main.command(CommandId::MarkdownPreviewSide);
        let view = main.view().unwrap();
        pump_until("render", Duration::from_secs(10), || {
            view.stats().block_count > 0
        });
        pump_for(Duration::from_millis(300));
        let open = private_bytes();
        main.command(CommandId::MarkdownPreviewClose);
        pump_for(Duration::from_millis(500));
        (open, private_bytes())
    };
    for warm_up in 1..=2 {
        let (open, closed) = cycle();
        println!("private bytes: warm-up {warm_up} open {open}, closed {closed}");
    }
    let (_, settled) = cycle();
    println!("private bytes: never {never_opened}, settled close {settled}");
    let mut closes = Vec::new();
    for round in 1..=5 {
        let (open, closed) = cycle();
        println!("private bytes: round {round} open {open}, closed {closed}");
        closes.push(closed);
    }
    closes.sort_unstable();
    let median = closes[closes.len() / 2];
    println!(
        "private bytes: median close {median}, growth {} bytes, closed vs never {} bytes",
        median as i64 - settled as i64,
        median as i64 - never_opened as i64
    );
    assert!(median.saturating_sub(settled) < 2 * 1024 * 1024);
}
/// Guards the zero-startup-cost rule at runtime, complementing the import-table guard: launching
/// with a Markdown file must not load the preview's graphics libraries.
#[test]
fn launching_with_a_markdown_file_loads_no_preview_graphics_library() {
    use support::process::{FastPadProcess, process_has_module_loaded};
    /// Removes the temporary folder even when an assertion fails.
    struct TempRoot(std::path::PathBuf);
    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let root =
        TempRoot(std::env::temp_dir().join(format!("fastpad-startup-{}", std::process::id())));
    let root = &root.0;
    let local_app_data = root.join("LocalAppData");
    std::fs::create_dir_all(&local_app_data).unwrap();
    let path = root.join("startup.md");
    std::fs::write(&path, "# Title\n\nBody with a [link](https://x.dev).\n").unwrap();
    let mut process = FastPadProcess::spawn_with_local_app_data(
        [std::ffi::OsStr::new("--new-window"), path.as_os_str()],
        &local_app_data,
    )
    .unwrap();
    process
        .wait_for_main_window(Duration::from_secs(10))
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    for module in ["d2d1.dll", "dwrite.dll", "windowscodecs.dll"] {
        assert!(
            !process_has_module_loaded(process.id(), module).unwrap(),
            "{module} was loaded before any preview was opened"
        );
    }
    process.close().unwrap();
    let _ = std::fs::remove_dir_all(root);
}
