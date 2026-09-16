use crate::Result;
use crate::app::{App, WindowIdentity};
use crate::document::{CloseDecision, Document, DocumentId, RecoveryId};
use crate::editor::{Editor, TextDirection};
use crate::perf::Milestone;
use crate::platform::{last_error, wide_null};
use crate::window::accessibility::{self, AccessibleSelectRequest, WM_FASTPAD_ACCESSIBLE_SELECT};
use crate::window::commands::CommandId;
use crate::window::find_bar;
use crate::window::menus::{self, MenuBar};
use crate::window::messages::{
    DeferredAction, classify_deferred_message, completed_milestone, deferred_start_message,
};
use crate::window::modal::prompt_close_decision;
use crate::window::palette::Palette;
use crate::window::tabs::CloseReviewKey;
use crate::window::titlebar::{HitTarget, PointerState, TitleBarLayout, TitleFontHandles};
#[cfg(test)]
use std::cell::Cell;
use std::ffi::c_void;
use std::ptr::NonNull;
use windows_sys::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::System::SystemInformation::GetTickCount;
use windows_sys::Win32::UI::Controls::{NMHDR, WM_MOUSELEAVE};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, GetLastInputInfo, LASTINPUTINFO, ReleaseCapture, SetCapture, SetFocus,
    VK_CONTROL, VK_F10, VK_MENU, VK_SHIFT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow, GWL_STYLE, GWLP_USERDATA,
    GetClientRect, GetWindowLongPtrW, HTCAPTION, IsZoomed, KillTimer, MoveWindow, OBJID_CLIENT,
    PostMessageW, PostQuitMessage, QS_INPUT, RegisterClassW, SC_CLOSE, SC_KEYMENU, SC_MAXIMIZE,
    SC_MINIMIZE, SC_RESTORE, SW_HIDE, SW_SHOWNA, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SWP_NOZORDER, SendMessageW, SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    UnregisterClassW, WHEEL_DELTA, WM_CAPTURECHANGED, WM_CLOSE, WM_COMMAND, WM_DESTROY,
    WM_DPICHANGED, WM_DWMCOLORIZATIONCOLORCHANGED, WM_EXITMENULOOP, WM_GETMINMAXINFO, WM_GETOBJECT,
    WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL,
    WM_NCCALCSIZE, WM_NCCREATE, WM_NCDESTROY, WM_NCHITTEST, WM_NCLBUTTONDBLCLK, WM_NCLBUTTONDOWN,
    WM_NCLBUTTONUP, WM_NCMOUSELEAVE, WM_NCMOUSEMOVE, WM_NCRBUTTONDOWN, WM_NCRBUTTONUP, WM_NOTIFY,
    WM_PAINT, WM_SETFOCUS, WM_SETTINGCHANGE, WM_SIZE, WM_SYSCOMMAND, WM_SYSKEYDOWN, WM_SYSKEYUP,
    WM_THEMECHANGED, WM_TIMER, WNDCLASSW, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};
#[cfg(test)]
use windows_sys::Win32::UI::WindowsAndMessaging::{MSG, PM_NOREMOVE, PeekMessageW, WM_QUIT};

pub(crate) const INPUT_MESSAGE_FIRST: u32 =
    windows_sys::Win32::UI::WindowsAndMessaging::WM_INPUT_DEVICE_CHANGE;
pub(crate) const INPUT_MESSAGE_LAST: u32 =
    windows_sys::Win32::UI::WindowsAndMessaging::WM_POINTERROUTEDRELEASED;

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

    fn lp_param(&mut self) -> *mut c_void {
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

    pub fn create(&self, context: &mut WindowCreateContext<App>) -> Result<HWND> {
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
                context.lp_param().cast(),
            )
        };
        if hwnd.is_null() {
            return Err(last_error());
        }
        // Re-runs WM_NCCALCSIZE so the initial frame drops the native caption band.
        unsafe {
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        Ok(hwnd)
    }
}

impl Drop for MainWindowClass {
    fn drop(&mut self) {
        unsafe {
            UnregisterClassW(self.class_name.as_ptr(), self.instance);
        }
    }
}

pub(crate) unsafe fn maybe_post_deferred_start(hwnd: HWND, identity: &WindowIdentity) {
    // SAFETY: The caller guarantees `hwnd` is the live FastPad main window. The raw App pointer is
    // used only for the immediate pending-flag transition before posting the deferred message.
    if !identity.is_live_for(hwnd) {
        return;
    }
    let should_post = unsafe { take_deferred_start_pending(hwnd) };
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
            layout_editor_and_find_bar(hwnd);
            invalidate_title_strip(hwnd);
            0
        }
        WM_SETFOCUS => {
            // With no tab open the editor is hidden and the frame itself keeps the focus.
            if tab_count(hwnd) > 0
                && let Some(editor_hwnd) = unsafe { editor_hwnd(hwnd) }
            {
                unsafe {
                    SetFocus(editor_hwnd);
                }
            }
            0
        }
        WM_CLOSE => {
            if file_population_active(hwnd) {
                return 0;
            }
            // A launch forwarded just before the review must be handled, not lost with the window.
            drain_ipc_requests(hwnd);
            if let Some(discarded) = review_dirty_documents(hwnd) {
                remove_session_snapshots(hwnd, &discarded);
                shutdown_ipc(hwnd);
                clear_documents_for_shutdown(hwnd);
                unsafe {
                    DestroyWindow(hwnd);
                }
            }
            0
        }
        WM_DESTROY => {
            unsafe {
                KillTimer(hwnd, crate::recovery::RECOVERY_TIMER_ID);
                PostQuitMessage(0);
            }
            0
        }
        WM_TIMER if wparam == crate::recovery::RECOVERY_TIMER_ID => {
            snapshot_when_idle(hwnd);
            0
        }
        WM_PAINT => {
            let paint_title_strip = |hwnd, _, _, _| {
                let (titles, active, scroll, empty) = tab_snapshot(hwnd);
                let title_refs = titles.iter().map(String::as_str).collect::<Vec<_>>();
                let status = current_status_text(hwnd);
                let (palette, fonts, pointer) = title_chrome(hwnd);
                unsafe {
                    crate::window::titlebar::paint(
                        hwnd,
                        &crate::window::titlebar::TitlePaint {
                            titles: &title_refs,
                            active,
                            scroll,
                            empty_hint: empty.then_some(EMPTY_TABS_HINT),
                            status: status.as_deref(),
                            palette,
                            fonts,
                            pointer,
                        },
                    )
                };
                0
            };
            let complete_first_paint = |hwnd| unsafe {
                mark_first_paint_complete(hwnd);
            };
            unsafe {
                handle_paint_with(
                    hwnd,
                    message,
                    wparam,
                    lparam,
                    paint_title_strip,
                    complete_first_paint,
                )
            }
        }
        WM_NCHITTEST => unsafe {
            crate::window::titlebar::nonclient_hit_test(
                hwnd,
                wparam,
                lparam,
                tab_count(hwnd),
                tab_scroll(hwnd),
            )
        },
        WM_NCCALCSIZE => unsafe { crate::window::titlebar::reclaim_caption(hwnd, wparam, lparam) },
        WM_GETMINMAXINFO => unsafe {
            crate::window::titlebar::constrain_maximized_window(hwnd, lparam)
        },
        WM_MOUSEMOVE => {
            drag_tab_thumb(hwnd, lparam);
            crate::window::titlebar::track_pointer_leave(hwnd, false);
            let target = client_title_target(hwnd, lparam);
            update_title_pointer(hwnd, |pointer| pointer.hover(target));
            0
        }
        WM_MOUSELEAVE => {
            update_title_pointer(hwnd, |pointer| pointer.leave(false));
            0
        }
        WM_NCMOUSEMOVE => {
            let target = HitTarget::from_nonclient_code(wparam);
            if target.is_some() {
                crate::window::titlebar::track_pointer_leave(hwnd, true);
            }
            update_title_pointer(hwnd, |pointer| pointer.hover(target));
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        WM_NCMOUSELEAVE => {
            update_title_pointer(hwnd, |pointer| pointer.leave(true));
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        WM_LBUTTONDOWN => {
            let target = client_title_target(hwnd, lparam);
            update_title_pointer(hwnd, |pointer| pointer.hover(target).press(target));
            if target == Some(HitTarget::ScrollBar) {
                begin_tab_thumb_drag(hwnd, (lparam as u32 & 0xffff) as u16 as i16 as i32);
            }
            0
        }
        WM_CAPTURECHANGED => {
            if let Some(mut app) = unsafe { app_ptr(hwnd) } {
                unsafe { app.as_mut() }.tab_thumb_grab = None;
            }
            0
        }
        // The empty tab-strip space is the only caption: double-clicking it opens a tab, VSCode
        // style, instead of maximizing.
        WM_NCLBUTTONDBLCLK if wparam == HTCAPTION as usize => {
            execute_command(hwnd, CommandId::New);
            0
        }
        // Its context menu replaces the system menu; Alt+Space still opens that.
        WM_NCRBUTTONDOWN if wparam == HTCAPTION as usize => 0,
        WM_NCRBUTTONUP if wparam == HTCAPTION as usize => {
            let mut point = windows_sys::Win32::Foundation::POINT {
                x: (lparam as u32 & 0xffff) as u16 as i16 as i32,
                y: ((lparam as u32 >> 16) & 0xffff) as u16 as i16 as i32,
            };
            unsafe {
                windows_sys::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut point);
            }
            let has_tabs = tab_count(hwnd) > 0;
            if let Some(command) = menus::show_tab_strip_menu(hwnd, point.x, point.y, has_tabs) {
                execute_command(hwnd, command);
            }
            0
        }
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            let delta = i32::from((wparam >> 16) as u16 as i16);
            // Wheel up scrolls toward the first tab; a tilt to the right toward the last.
            let delta = if message == WM_MOUSEWHEEL {
                -delta
            } else {
                delta
            };
            if scroll_tabs(hwnd, lparam, delta) {
                0
            } else {
                unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
            }
        }
        // DefWindowProc would run its own classic caption-button tracking loop over our strip.
        WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK
            if HitTarget::from_nonclient_code(wparam).is_some() =>
        {
            let target = HitTarget::from_nonclient_code(wparam);
            update_title_pointer(hwnd, |pointer| pointer.hover(target).press(target));
            0
        }
        WM_NCLBUTTONUP if HitTarget::from_nonclient_code(wparam).is_some() => {
            let target = HitTarget::from_nonclient_code(wparam);
            let mut activated = None;
            update_title_pointer(hwnd, |pointer| {
                let (next, released) = pointer.release(target);
                activated = released;
                next
            });
            if let Some(target) = activated {
                run_caption_button(hwnd, target);
            }
            0
        }
        WM_LBUTTONUP => {
            update_title_pointer(hwnd, |pointer| pointer.release(None).0);
            if end_tab_thumb_drag(hwnd) {
                return 0;
            }
            let point = crate::window::titlebar::Point::new(
                (lparam as u32 & 0xffff) as u16 as i16 as i32,
                ((lparam as u32 >> 16) & 0xffff) as u16 as i16 as i32,
            );
            if status_contains(hwnd, point.y) {
                dismiss_notifications(hwnd);
                return 0;
            }
            let layout = title_layout(hwnd);
            match layout.hit_test(point) {
                crate::window::titlebar::HitTarget::Overflow => {
                    if let Some(command) = menus::show_overflow(hwnd, point.x, layout.height) {
                        execute_command(hwnd, command);
                    }
                }
                crate::window::titlebar::HitTarget::CloseTab(index) => {
                    activate_tab(hwnd, index);
                    execute_command(hwnd, CommandId::CloseTab);
                }
                crate::window::titlebar::HitTarget::Tab(index) => activate_tab(hwnd, index),
                _ => {}
            }
            0
        }
        WM_NOTIFY => {
            handle_editor_notification(hwnd, lparam);
            0
        }
        WM_FASTPAD_ACCESSIBLE_SELECT => handle_accessible_select(hwnd, lparam),
        WM_COMMAND => {
            if let Ok(command) = CommandId::try_from((wparam & 0xffff) as u16) {
                execute_command(hwnd, command);
            }
            0
        }
        WM_SYSCOMMAND if transient_menu_syscommand(wparam, lparam) => {
            show_menu_mode(hwnd);
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        WM_EXITMENULOOP => {
            menus::detach_menu(hwnd);
            0
        }
        WM_GETOBJECT if lparam as i32 == OBJID_CLIENT => {
            let provider = ensure_accessibility(hwnd);
            if provider.is_null() {
                0
            } else {
                unsafe { accessibility::object_result(provider, wparam) }
            }
        }
        WM_DPICHANGED => {
            let suggested = unsafe { &*(lparam as *const RECT) };
            if let Some(mut app) = unsafe { app_ptr(hwnd) } {
                unsafe { app.as_mut() }.title_fonts = None;
            }
            unsafe {
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    suggested.left,
                    suggested.top,
                    suggested.right - suggested.left,
                    suggested.bottom - suggested.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                InvalidateRect(hwnd, std::ptr::null(), 1);
            }
            let dpi = (wparam & 0xffff) as u32;
            if let Some(editor) =
                unsafe { app_ptr(hwnd) }.and_then(|app| unsafe { app.as_ref() }.editor.clone())
            {
                let _ = editor.set_text_padding(dpi);
            }
            0
        }
        WM_SETTINGCHANGE | WM_THEMECHANGED | WM_DWMCOLORIZATIONCOLORCHANGED => {
            refresh_theme(hwnd);
            unsafe {
                InvalidateRect(hwnd, std::ptr::null(), 1);
            }
            0
        }
        WM_NCDESTROY => {
            menus::detach_menu(hwnd);
            let app = unsafe { take_app(hwnd) };
            if let Some(app) = app.as_ref() {
                app.invalidate_window(hwnd);
            }
            let result = unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
            drop(app);
            result
        }
        _ => {
            // A nested modal loop dispatches whatever is queued. Deferred startup units and the
            // IPC drain wait for it to end so they cannot change the document it acts on.
            if (message == crate::window::WM_FASTPAD_IPC_REQUEST
                || classify_deferred_message(message, false).is_some())
                && crate::window::modal::hold_while_modal(hwnd, message)
            {
                return 0;
            }
            if message == crate::window::WM_FASTPAD_IPC_REQUEST {
                return handle_ipc_requests(hwnd);
            }
            if message == crate::window::WM_FASTPAD_DIAGNOSTIC_JSON_COUNT
                && unsafe { app_ptr(hwnd) }
                    .is_some_and(|app| unsafe { app.as_ref() }.launch.diagnostic)
            {
                return crate::languages::json_invocation_count() as LRESULT;
            }
            if let Some(action) = classify_deferred_message(message, input_pending()) {
                return handle_deferred(hwnd, action);
            }
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
    }
}

unsafe fn handle_paint_with<D, C>(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    default_window_proc: D,
    complete_first_paint: C,
) -> LRESULT
where
    D: FnOnce(HWND, u32, WPARAM, LPARAM) -> LRESULT,
    C: FnOnce(HWND),
{
    let identity = unsafe { window_identity(hwnd) };
    let result = default_window_proc(hwnd, message, wparam, lparam);
    if identity
        .as_ref()
        .is_some_and(|identity| identity.is_live_for(hwnd))
    {
        complete_first_paint(hwnd);
    }
    result
}

unsafe fn on_nc_create(hwnd: HWND, lparam: LPARAM) -> LRESULT {
    let Some(mut app) = (unsafe { take_create_context_app(lparam) }) else {
        return 0;
    };
    if !app.bind_window(hwnd) {
        return 0;
    }
    store_app(hwnd, app);
    1
}

fn handle_deferred(hwnd: HWND, action: DeferredAction) -> LRESULT {
    // Only `WM_FASTPAD_OPEN_REQUEST` processed with no input pending produces this action, so the
    // launch file opens on exactly the same input-readiness gate as the rest of the chain. The
    // open posts the language continuation (and records FileLoaded) itself.
    if action == DeferredAction::PostNext(crate::window::WM_FASTPAD_APPLY_LANGUAGE) {
        return handle_open_request(hwnd);
    }
    // Each of these actions is produced only by its own deferred message with no input pending:
    // `PostNext(WM_FASTPAD_OPEN_REQUEST)` by `WM_FASTPAD_LOAD_SETTINGS`, `RecordFullyReady` by
    // `WM_FASTPAD_BUILD_CHROME`. Running them before the milestone keeps the milestone honest.
    if action == DeferredAction::PostNext(crate::window::WM_FASTPAD_OPEN_REQUEST) {
        load_settings(hwnd);
    }
    if action == DeferredAction::RecordFullyReady {
        build_chrome(hwnd);
    }
    if let Some(milestone) = completed_milestone(action) {
        unsafe {
            let _ = record_milestone(hwnd, milestone);
        }
    }
    // `WM_FASTPAD_APPLY_LANGUAGE` is the only message that classifies into
    // `PostNext(WM_FASTPAD_RECOVERY)`, so this is exactly the point where that message has just
    // been processed (input was not pending). It serves double duty: advancing the deferred
    // startup chain (above/below) and, here, detecting and applying the active document's
    // language after every successful Open/Save As/launch load that posts it.
    if action == DeferredAction::PostNext(crate::window::WM_FASTPAD_RECOVERY) {
        apply_detected_language(hwnd);
    }
    // Only `WM_FASTPAD_RECOVERY` processed with no input pending produces this action.
    if action == DeferredAction::PostNext(crate::window::WM_FASTPAD_START_IPC) {
        recover_snapshots(hwnd);
    }
    // Only `WM_FASTPAD_START_IPC` processed with no input pending produces this action.
    if action == DeferredAction::PostNext(crate::window::WM_FASTPAD_BUILD_CHROME) {
        start_ipc_server_with(hwnd, crate::ipc::bind_session_server);
    }
    match action {
        DeferredAction::RepostSelf(message) => {
            unsafe {
                request_input_priority(hwnd);
            }
            unsafe {
                PostMessageW(hwnd, message, 0, 0);
            }
            0
        }
        DeferredAction::PostNext(message) => unsafe {
            PostMessageW(hwnd, message, 0, 0);
            0
        },
        DeferredAction::RecordFullyReady => 0,
    }
}

fn input_pending() -> bool {
    const STATUS_SHIFT: u32 = 16;
    let queue_status = input_queue_status_mask();
    let pending = unsafe {
        ((windows_sys::Win32::UI::WindowsAndMessaging::GetQueueStatus(queue_status)
            >> STATUS_SHIFT)
            & queue_status)
            != 0
    };
    #[cfg(test)]
    if pending && queue_status != QS_INPUT {
        let mut message = MSG::default();
        let found = unsafe {
            PeekMessageW(
                &mut message,
                std::ptr::null_mut(),
                INPUT_MESSAGE_FIRST,
                INPUT_MESSAGE_LAST,
                PM_NOREMOVE | (queue_status << STATUS_SHIFT),
            )
        } != 0;
        return found && message.message != WM_QUIT;
    }
    pending
}

pub(crate) fn input_queue_status_mask() -> u32 {
    #[cfg(test)]
    {
        TEST_INPUT_QUEUE_STATUS.with(Cell::get)
    }
    #[cfg(not(test))]
    {
        QS_INPUT
    }
}

#[cfg(test)]
thread_local! {
    static TEST_INPUT_QUEUE_STATUS: Cell<u32> = const { Cell::new(QS_INPUT) };
}

#[cfg(test)]
pub(crate) fn with_test_input_queue_status<R>(queue_status: u32, run: impl FnOnce() -> R) -> R {
    struct ResetQueueStatus(u32);

    impl Drop for ResetQueueStatus {
        fn drop(&mut self) {
            TEST_INPUT_QUEUE_STATUS.with(|status| status.set(self.0));
        }
    }

    TEST_INPUT_QUEUE_STATUS.with(|status| {
        let reset = ResetQueueStatus(status.replace(queue_status));
        let result = run();
        drop(reset);
        result
    })
}

unsafe fn take_app(hwnd: HWND) -> Option<Box<App>> {
    let raw = unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *mut App };
    (!raw.is_null()).then(|| unsafe { Box::from_raw(raw) })
}

pub(crate) unsafe fn initialize_editor_with<F>(
    hwnd: HWND,
    identity: &WindowIdentity,
    create_editor: F,
) -> Result<HWND>
where
    F: FnOnce(HWND) -> Result<Editor>,
{
    // SAFETY: The caller guarantees `hwnd` is the live FastPad main window whose `GWLP_USERDATA`
    // owns an App. No App reference is held across the reentrant editor creation callback.
    if !identity.is_live_for(hwnd) {
        return Err(crate::FastPadError::Invariant(
            "main window identity was not live during editor initialization",
        ));
    }
    unsafe {
        record_milestone(hwnd, Milestone::WindowCreated)?;
    }

    let editor = create_editor(hwnd)?;
    let editor_hwnd = editor.hwnd();
    if !identity.is_live_for(hwnd) {
        return Err(crate::FastPadError::Invariant(
            "main window was destroyed during editor initialization",
        ));
    }

    let recovery_id = unsafe { app_ptr(hwnd) }
        .map(|app| unsafe { app.as_ref() }.allocate_recovery_id())
        .ok_or(crate::FastPadError::Invariant(
            "main window app state was not available",
        ))?;
    let document = Document::untitled(DocumentId(1), recovery_id, editor.current_document()?);
    if !identity.is_live_for(hwnd) {
        return Err(crate::FastPadError::Invariant(
            "main window was destroyed while adopting the initial document",
        ));
    }

    unsafe {
        install_editor(hwnd, editor, document)?;
        record_milestone(hwnd, Milestone::EditorCreated)?;
    }
    Ok(editor_hwnd)
}

pub(crate) unsafe fn input_priority_requested(hwnd: HWND, identity: &WindowIdentity) -> bool {
    // SAFETY: This helper reads a raw App pointer stored in `GWLP_USERDATA` and copies a boolean
    // flag without returning references across the FFI boundary.
    if !identity.is_live_for(hwnd) {
        return false;
    }
    unsafe { app_ptr(hwnd) }
        .map(|app| unsafe { app.as_ref() }.prioritizes_input())
        .unwrap_or(false)
}

pub(crate) unsafe fn clear_input_priority(hwnd: HWND, identity: &WindowIdentity) {
    // SAFETY: This helper mutates a boolean flag through the window-owned App pointer and does not
    // retain any reference across reentrant Win32 calls.
    if !identity.is_live_for(hwnd) {
        return;
    }
    if let Some(mut app) = unsafe { app_ptr(hwnd) } {
        unsafe { app.as_mut() }.clear_input_priority();
    }
}

unsafe fn record_milestone(hwnd: HWND, milestone: Milestone) -> Result<()> {
    // SAFETY: The App pointer is owned by the window and is only used for an immediate milestone
    // write before returning to the caller.
    let Some(mut app) = (unsafe { app_ptr(hwnd) }) else {
        return Err(crate::FastPadError::Invariant(
            "main window app state was not available",
        ));
    };
    unsafe { app.as_mut() }.startup.record_now(milestone)
}

unsafe fn mark_first_paint_complete(hwnd: HWND) {
    // SAFETY: The App pointer is used only for an immediate state transition after painting.
    if let Some(mut app) = unsafe { app_ptr(hwnd) } {
        unsafe { app.as_mut() }.mark_first_paint_complete();
    }
}

unsafe fn take_deferred_start_pending(hwnd: HWND) -> bool {
    // SAFETY: The App pointer is used only for an immediate flag read-reset before returning.
    unsafe { app_ptr(hwnd) }
        .map(|mut app| unsafe { app.as_mut() }.take_deferred_start_pending())
        .unwrap_or(false)
}

unsafe fn request_input_priority(hwnd: HWND) {
    // SAFETY: The App pointer is used only for an immediate flag write before returning.
    if let Some(mut app) = unsafe { app_ptr(hwnd) } {
        unsafe { app.as_mut() }.request_input_priority();
    }
}

unsafe fn editor_hwnd(hwnd: HWND) -> Option<HWND> {
    // SAFETY: The App pointer is used only to copy out the child HWND; no reference crosses into
    // any subsequent Win32 call.
    let app = unsafe { app_ptr(hwnd) }?;
    unsafe { app.as_ref() }.editor.as_ref().map(Editor::hwnd)
}

fn with_editor(hwnd: HWND, action: impl FnOnce(&Editor)) {
    let Some(app) = (unsafe { app_ptr(hwnd) }) else {
        return;
    };
    let Some(editor) = unsafe { app.as_ref() }.editor.as_ref() else {
        return;
    };
    action(editor);
}

/// Repositions the editor (and the find bar, if visible) to account for the title strip and an
/// optional find/replace bar reserved above it. The sole layout choke point for both; extends the
/// pre-Task-12 `WM_SIZE` editor-only positioning rather than duplicating it.
fn layout_editor_and_find_bar(hwnd: HWND) {
    let Some(editor_hwnd) = (unsafe { editor_hwnd(hwnd) }) else {
        return;
    };
    let title_height = title_layout(hwnd).height;
    let mut rect = RECT::default();
    unsafe {
        GetClientRect(hwnd, &mut rect);
    }
    let width = rect.right - rect.left;
    let find_bar_height = unsafe { app_ptr(hwnd) }
        .and_then(|app| {
            let bar = unsafe { app.as_ref() }.find_bar.as_ref()?;
            bar.layout(width, title_height);
            bar.is_visible().then_some(find_bar::FIND_BAR_HEIGHT)
        })
        .unwrap_or(0);
    let content_top = title_height + find_bar_height;
    let status_height = status_bar_height(hwnd);
    unsafe {
        MoveWindow(
            editor_hwnd,
            0,
            content_top,
            width,
            (rect.bottom - rect.top - content_top - status_height).max(0),
            1,
        );
    }
}

fn open_find_bar(hwnd: HWND, mode: find_bar::FindBarMode) {
    let Some(identity) = (unsafe { window_identity(hwnd) }) else {
        return;
    };
    // A single-line selection is a reasonable query prefill; a multi-line one is not (the bar has
    // no way to display it), so it's left alone rather than truncated or rejected.
    let prefill = unsafe { app_ptr(hwnd) }.and_then(|app| {
        let editor = unsafe { app.as_ref() }.editor.as_ref()?;
        let text = editor.selected_text().ok()?;
        (!text.is_empty() && !text.contains(['\n', '\r'])).then_some(text)
    });
    let opened = unsafe { app_ptr(hwnd) }.is_some_and(|mut app| {
        let app = unsafe { app.as_mut() };
        if app.find_bar.is_none() {
            app.find_bar = find_bar::FindBar::create(hwnd).ok();
        }
        let Some(bar) = app.find_bar.as_mut() else {
            return false;
        };
        bar.show(mode, prefill.as_deref());
        true
    });
    if !opened || !identity.is_live_for(hwnd) {
        return;
    }
    layout_editor_and_find_bar(hwnd);
    if let Some(app) = unsafe { app_ptr(hwnd) }
        && let Some(bar) = unsafe { app.as_ref() }.find_bar.as_ref()
    {
        bar.focus_query();
    }
}

pub(crate) fn close_find_bar(hwnd: HWND) {
    let Some(identity) = (unsafe { window_identity(hwnd) }) else {
        return;
    };
    let closed = unsafe { app_ptr(hwnd) }.is_some_and(|mut app| {
        let Some(bar) = unsafe { app.as_mut() }.find_bar.as_mut() else {
            return false;
        };
        bar.hide();
        true
    });
    if !closed || !identity.is_live_for(hwnd) {
        return;
    }
    layout_editor_and_find_bar(hwnd);
    if let Some(editor_hwnd) = unsafe { editor_hwnd(hwnd) } {
        unsafe {
            SetFocus(editor_hwnd);
        }
    }
}

pub(crate) fn find_next(hwnd: HWND) {
    navigate_to_match(hwnd, false);
}

pub(crate) fn find_previous(hwnd: HWND) {
    navigate_to_match(hwnd, true);
}

fn navigate_to_match(hwnd: HWND, backward: bool) {
    let Some(identity) = (unsafe { window_identity(hwnd) }) else {
        return;
    };
    let Some((editor, query)) = (unsafe { app_ptr(hwnd) }).and_then(|app| {
        let app = unsafe { app.as_ref() };
        let editor = app.editor.clone()?;
        let query = app.find_bar.as_ref()?.query_text();
        Some((editor, query))
    }) else {
        return;
    };
    if query.is_empty() {
        return;
    }
    let (Ok(selection), Ok(doc_len)) = (editor.selection(), editor.length()) else {
        return;
    };
    let origin = if backward {
        selection.start
    } else {
        selection.end
    };
    let direction = if backward {
        find_bar::SearchDirection::Backward
    } else {
        find_bar::SearchDirection::Forward
    };
    let mut state = find_bar::SearchState::new(&query, direction, origin);
    if let Ok(Some(found)) = state.next_editor_match(&editor, 0, doc_len)
        && identity.is_live_for(hwnd)
    {
        let _ = editor.set_selection(found);
        editor.scroll_caret_into_view();
    }
}

pub(crate) fn replace_current(hwnd: HWND) {
    let Some(identity) = (unsafe { window_identity(hwnd) }) else {
        return;
    };
    let Some((editor, query, replacement)) = (unsafe { app_ptr(hwnd) }).and_then(|app| {
        let app = unsafe { app.as_ref() };
        let editor = app.editor.clone()?;
        let bar = app.find_bar.as_ref()?;
        Some((editor, bar.query_text(), bar.replace_text()))
    }) else {
        return;
    };
    if query.is_empty() {
        return;
    }
    // Only replace when the current selection is exactly the query match; otherwise this Enter
    // press just navigates to the next match, matching a bare Find field's behavior.
    if let (Ok(selection), Ok(selected)) = (editor.selection(), editor.selected_text())
        && selected == query
    {
        let _ = editor.replace_target(selection, &replacement);
        if !identity.is_live_for(hwnd) {
            return;
        }
    }
    find_next(hwnd);
}

pub(crate) fn replace_all_matches(hwnd: HWND) {
    let Some(identity) = (unsafe { window_identity(hwnd) }) else {
        return;
    };
    let Some((editor, query, replacement)) = (unsafe { app_ptr(hwnd) }).and_then(|app| {
        let app = unsafe { app.as_ref() };
        let editor = app.editor.clone()?;
        let bar = app.find_bar.as_ref()?;
        Some((editor, bar.query_text(), bar.replace_text()))
    }) else {
        return;
    };
    if query.is_empty() {
        return;
    }
    let _ = editor.replace_all(&query, &replacement, 0);
    if identity.is_live_for(hwnd) {
        editor.scroll_caret_into_view();
    }
}

const EMPTY_TABS_HINT: &str =
    "No tabs are open.\nPress Ctrl+N or double-click the tab bar to start a new one.";

fn tab_count(hwnd: HWND) -> usize {
    unsafe { app_ptr(hwnd) }
        .map(|app| unsafe { app.as_ref() }.tabs.len())
        .unwrap_or(1)
}

fn tab_scroll(hwnd: HWND) -> i32 {
    unsafe { app_ptr(hwnd) }
        .map(|app| unsafe { app.as_ref() }.tabs.scroll_offset())
        .unwrap_or(0)
}

fn title_layout(hwnd: HWND) -> TitleBarLayout {
    crate::window::titlebar::layout_for_window(hwnd, tab_count(hwnd), tab_scroll(hwnd))
}

/// Titles, active index, scroll offset, and whether the editor is hidden because no tab is open.
fn tab_snapshot(hwnd: HWND) -> (Vec<String>, usize, i32, bool) {
    unsafe { app_ptr(hwnd) }
        .map(|app| {
            let app = unsafe { app.as_ref() };
            (
                app.tabs.titles().collect(),
                app.tabs.active_index(),
                app.tabs.scroll_offset(),
                app.editor.is_some() && app.tabs.is_empty(),
            )
        })
        .unwrap_or_else(|| (vec!["Untitled".to_owned()], 0, 0, false))
}

/// Scrolls the tabs when the wheel turns over the tab strip; reports whether it was over it.
fn scroll_tabs(hwnd: HWND, lparam: LPARAM, delta: i32) -> bool {
    let mut point = windows_sys::Win32::Foundation::POINT {
        x: (lparam as u32 & 0xffff) as u16 as i16 as i32,
        y: ((lparam as u32 >> 16) & 0xffff) as u16 as i16 as i32,
    };
    unsafe {
        windows_sys::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut point);
    }
    let layout = title_layout(hwnd);
    if point.y < 0 || point.y >= layout.height || point.x < 0 || point.x >= layout.overflow.left {
        return false;
    }
    let scroll = layout.scroll_by_wheel(delta, WHEEL_DELTA as i32);
    let changed = unsafe { app_ptr(hwnd) }
        .is_some_and(|app| unsafe { app.as_ref() }.tabs.set_scroll_offset(scroll));
    if changed {
        // The hovered tab moved out from under the pointer; the next mouse move finds the new one.
        update_title_pointer(hwnd, |pointer| pointer.hover(None));
        crate::window::titlebar::invalidate_strip(hwnd);
    }
    true
}

/// Starts dragging the tab scroll thumb. Pressing the track beside the thumb first jumps the
/// thumb there, centred under the pointer, so the same press can keep dragging it.
fn begin_tab_thumb_drag(hwnd: HWND, x: i32) {
    let layout = title_layout(hwnd);
    let Some(thumb) = layout.scroll_thumb() else {
        return;
    };
    let grab = if (thumb.left..thumb.right).contains(&x) {
        x - thumb.left
    } else {
        let grab = (thumb.right - thumb.left) / 2;
        if let Some(app) = unsafe { app_ptr(hwnd) } {
            unsafe { app.as_ref() }
                .tabs
                .set_scroll_offset(layout.scroll_for_thumb(x - grab));
        }
        grab
    };
    if let Some(mut app) = unsafe { app_ptr(hwnd) } {
        unsafe { app.as_mut() }.tab_thumb_grab = Some(grab);
    }
    unsafe {
        SetCapture(hwnd);
    }
    crate::window::titlebar::invalidate_strip(hwnd);
}

fn drag_tab_thumb(hwnd: HWND, lparam: LPARAM) {
    let Some(grab) =
        unsafe { app_ptr(hwnd) }.and_then(|app| unsafe { app.as_ref() }.tab_thumb_grab)
    else {
        return;
    };
    let x = (lparam as u32 & 0xffff) as u16 as i16 as i32;
    let scroll = title_layout(hwnd).scroll_for_thumb(x - grab);
    if unsafe { app_ptr(hwnd) }
        .is_some_and(|app| unsafe { app.as_ref() }.tabs.set_scroll_offset(scroll))
    {
        crate::window::titlebar::invalidate_strip(hwnd);
    }
}

/// Ends a thumb drag; reports whether one was in progress, so the release activates nothing.
fn end_tab_thumb_drag(hwnd: HWND) -> bool {
    let dragging = unsafe { app_ptr(hwnd) }
        .and_then(|mut app| unsafe { app.as_mut() }.tab_thumb_grab.take())
        .is_some();
    if dragging {
        unsafe {
            ReleaseCapture();
        }
    }
    dragging
}

/// Follows every change to the set of tabs or the active one: scrolls the active tab into view,
/// shows the editor only while a tab is open, and repaints.
fn refresh_tabs(hwnd: HWND) {
    let Some((count, active, editor_hwnd)) = (unsafe { app_ptr(hwnd) }).map(|app| {
        let app = unsafe { app.as_ref() };
        (
            app.tabs.len(),
            app.tabs.active_index(),
            app.editor.as_ref().map(Editor::hwnd),
        )
    }) else {
        return;
    };
    let scroll = if count == 0 {
        0
    } else {
        title_layout(hwnd).scroll_to_reveal(active)
    };
    if let Some(app) = unsafe { app_ptr(hwnd) } {
        unsafe { app.as_ref() }.tabs.set_scroll_offset(scroll);
    }
    if let Some(editor_hwnd) = editor_hwnd {
        let visible = unsafe { GetWindowLongPtrW(editor_hwnd, GWL_STYLE) } as u32 & WS_VISIBLE != 0;
        if count == 0 && visible {
            close_find_bar(hwnd);
            unsafe {
                ShowWindow(editor_hwnd, SW_HIDE);
                if GetFocus() == editor_hwnd {
                    SetFocus(hwnd);
                }
            }
        } else if count > 0 && !visible {
            unsafe {
                ShowWindow(editor_hwnd, SW_SHOWNA);
                if GetFocus() == hwnd {
                    SetFocus(editor_hwnd);
                }
            }
        }
    }
    unsafe {
        InvalidateRect(hwnd, std::ptr::null(), 0);
    }
}

fn execute_command(hwnd: HWND, command: CommandId) {
    if file_population_active(hwnd) {
        return;
    }
    if command.needs_document() && tab_count(hwnd) == 0 {
        return;
    }
    if let Some(index) = command.tab_index() {
        if index < tab_count(hwnd) {
            activate_tab(hwnd, index);
        }
        return;
    }
    match command {
        CommandId::Open => {
            let identity = unsafe { window_identity(hwnd) };
            // Modal Show reenters the window procedure. Only an owned identity crosses it.
            let selection = crate::window::modal::choose_open_path(hwnd);
            if identity
                .as_ref()
                .is_some_and(|identity| identity.is_live_for(hwnd))
            {
                match selection {
                    Ok(Some(path)) => {
                        if let Err(error) = App::open_path(hwnd, &path) {
                            report_open_failure(hwnd, &path, &error);
                        }
                    }
                    Ok(None) => {}
                    Err(error) => push_notice(
                        hwnd,
                        format!("FastPad could not show the Open dialog: {error}"),
                    ),
                }
            }
        }
        CommandId::New => {
            if let Err(error) = create_new_document(hwnd) {
                push_notice(hwnd, format!("FastPad could not create a new tab: {error}"));
            }
        }
        CommandId::CloseTab => close_active_document(hwnd),
        CommandId::CloseAllTabs => close_all_documents(hwnd),
        CommandId::Save => {
            let _ = save_active_document(hwnd);
        }
        CommandId::SaveAs => {
            let _ = save_active_document_as(hwnd);
        }
        CommandId::Undo => with_editor(hwnd, |editor| {
            let _ = editor.undo();
        }),
        CommandId::Redo => with_editor(hwnd, |editor| {
            let _ = editor.redo();
        }),
        CommandId::Cut => with_editor(hwnd, |editor| {
            let _ = editor.cut();
        }),
        CommandId::Copy => with_editor(hwnd, |editor| {
            let _ = editor.copy();
        }),
        CommandId::Paste => with_editor(hwnd, |editor| {
            let _ = editor.paste();
        }),
        CommandId::Find => open_find_bar(hwnd, find_bar::FindBarMode::Find),
        CommandId::Replace => open_find_bar(hwnd, find_bar::FindBarMode::Replace),
        CommandId::LanguagePlainText => apply_language(hwnd, crate::document::Language::PlainText),
        CommandId::LanguageJson => apply_language(hwnd, crate::document::Language::Json),
        CommandId::LanguageMarkdown => apply_language(hwnd, crate::document::Language::Markdown),
        CommandId::ValidateJson => validate_active_json(hwnd),
        CommandId::FormatJson => format_active_json(hwnd),
        CommandId::NextTab => cycle_tab(hwnd, true),
        CommandId::PreviousTab => cycle_tab(hwnd, false),
        CommandId::ZoomIn => with_editor(hwnd, |editor| {
            let _ = editor.zoom_in();
        }),
        CommandId::ZoomOut => with_editor(hwnd, |editor| {
            let _ = editor.zoom_out();
        }),
        CommandId::ZoomReset => with_editor(hwnd, |editor| {
            let _ = editor.reset_zoom();
        }),
        CommandId::TextLeftToRight => with_editor(hwnd, |editor| {
            let _ = editor.set_text_direction(TextDirection::LeftToRight);
        }),
        CommandId::TextRightToLeft => with_editor(hwnd, |editor| {
            let _ = editor.set_text_direction(TextDirection::RightToLeft);
        }),
        _ => {
            if let Some(mut app) = unsafe { app_ptr(hwnd) } {
                unsafe { app.as_mut() }.execute(command);
            }
        }
    }
}

fn file_population_active(hwnd: HWND) -> bool {
    unsafe { app_ptr(hwnd) }.is_some_and(|app| unsafe { app.as_ref() }.populating_file)
}

/// Detects the active document's language from its path (an untitled document has no path and
/// stays whatever it already is, i.e. plain text) and applies it. Reached only after `input_pending`
/// is false for `WM_FASTPAD_APPLY_LANGUAGE` (see `handle_deferred`), so this never runs ahead of
/// queued user input.
fn apply_detected_language(hwnd: HWND) {
    let path = unsafe { app_ptr(hwnd) }
        .and_then(|app| unsafe { app.as_ref() }.tabs.active()?.path.clone());
    let Some(path) = path else {
        return;
    };
    apply_language(hwnd, crate::languages::detect_language(&path));
}

/// Applies `language`'s lexer to the active editor via the (lazily created, per Task 13's
/// `find_bar`/`menu_bar`-style `Option<T>` precedent) `App::language_manager`, then records the
/// outcome on the active document's metadata: success updates `Document::language` to match what
/// is now actually shown; failure leaves the document's language metadata unchanged (the editor
/// itself is also left unchanged by `LanguageManager::apply` on failure) and surfaces the error.
fn apply_language(hwnd: HWND, language: crate::document::Language) {
    let Some(editor) =
        (unsafe { app_ptr(hwnd) }).and_then(|app| unsafe { app.as_ref() }.editor.clone())
    else {
        return;
    };
    let dark = effective_dark(hwnd);
    let result = unsafe { app_ptr(hwnd) }.map(|mut app| {
        let app = unsafe { app.as_mut() };
        if app.language_manager.is_none() {
            app.language_manager = Some(crate::languages::LanguageManager::new());
        }
        app.language_manager
            .as_mut()
            .expect("just populated above if it was absent")
            .apply(&editor, language, dark)
    });
    match result {
        Some(Ok(())) => {
            if let Some(mut app) = unsafe { app_ptr(hwnd) } {
                unsafe { app.as_mut() }.tabs.set_active_language(language);
            }
            // Lexer style tables reset every style's font face; restore the configured one.
            apply_editor_settings(hwnd);
        }
        Some(Err(_)) => push_notice(
            hwnd,
            "FastPad could not enable syntax highlighting for this file. It will remain in plain \
             text."
                .to_owned(),
        ),
        None => {}
    }
}

/// Runs only inside `WM_FASTPAD_LOAD_SETTINGS`: resolves and parses `fastpad.ini`, applies the
/// editor view settings in place, and queues every rejected line as a non-modal notification.
fn load_settings(hwnd: HWND) {
    let (settings, warnings) = crate::config::load();
    apply_loaded_settings(hwnd, settings, warnings);
    start_recovery_timer(hwnd);
}

/// Applies an already-loaded settings/warnings pair, split out of `load_settings` so tests can drive
/// the reporting path directly instead of mutating the process-wide `LOCALAPPDATA` environment
/// variable to fake a corrupt `fastpad.ini` on disk.
fn apply_loaded_settings(
    hwnd: HWND,
    settings: crate::config::Settings,
    warnings: Vec<crate::config::SettingWarning>,
) {
    if let Some(mut app) = unsafe { app_ptr(hwnd) } {
        let app = unsafe { app.as_mut() };
        app.settings = settings;
        for warning in &warnings {
            app.notifications.push(settings_warning_message(warning));
        }
    }
    apply_editor_settings(hwnd);
}

fn settings_warning_message(warning: &crate::config::SettingWarning) -> String {
    if warning.line == 0 {
        format!("fastpad.ini: {}", warning.message)
    } else {
        format!("fastpad.ini line {}: {}", warning.line, warning.message)
    }
}

/// Also recolors the line numbers: this runs after every lexer change, whose style reset gives
/// the gutter full-contrast text. Before chrome exists the neutral palette matches Scintilla's own
/// black-on-white defaults.
fn apply_editor_settings(hwnd: HWND) {
    let Some((editor, settings, palette)) = (unsafe { app_ptr(hwnd) }).and_then(|app| {
        let app = unsafe { app.as_ref() };
        let palette = Palette::for_cached_theme(app.theme, app.settings.theme);
        Some((app.editor.clone()?, app.settings.clone(), palette))
    }) else {
        return;
    };
    let _ = editor.set_line_numbers(settings.line_numbers);
    let _ = editor.apply_view_settings(
        &settings.font_face,
        settings.font_size,
        settings.tab_width,
        settings.word_wrap,
    );
    let _ =
        editor.set_line_number_colors(palette.line_number_foreground, palette.editor_background);
}

/// Runs only inside `WM_FASTPAD_BUILD_CHROME`: the first system theme query, the status model,
/// and a repaint that makes any queued notifications visible.
fn build_chrome(hwnd: HWND) {
    let theme = crate::platform::theme::SystemTheme::detect();
    if let Some(mut app) = unsafe { app_ptr(hwnd) } {
        let app = unsafe { app.as_mut() };
        app.theme = Some(theme);
        app.status = Some(crate::window::status::StatusModel::new(theme));
    }
    apply_theme(hwnd);
    layout_editor_and_find_bar(hwnd);
    unsafe {
        InvalidateRect(hwnd, std::ptr::null(), 1);
    }
}

/// Re-queries the system theme after chrome exists and restyles the editor only on a real change.
fn refresh_theme(hwnd: HWND) {
    let changed = unsafe { app_ptr(hwnd) }.is_some_and(|mut app| {
        let app = unsafe { app.as_mut() };
        if app.theme.is_none() {
            return false;
        }
        let theme = crate::platform::theme::SystemTheme::detect();
        if app.theme == Some(theme) {
            return false;
        }
        app.theme = Some(theme);
        if let Some(status) = app.status.as_mut() {
            status.theme = theme;
        }
        true
    });
    if changed {
        apply_theme(hwnd);
    }
}

/// Before chrome exists there is no cached theme, so the `System` preference falls back to the
/// one-shot registry read `apply_language` has always used.
fn effective_dark(hwnd: HWND) -> bool {
    let (theme, preference) = unsafe { app_ptr(hwnd) }
        .map(|app| {
            let app = unsafe { app.as_ref() };
            (app.theme, app.settings.theme)
        })
        .unwrap_or((None, crate::config::ThemePreference::System));
    match (theme, preference) {
        (Some(theme), preference) => theme.effective_dark(preference),
        (None, crate::config::ThemePreference::System) => {
            crate::platform::theme::system_uses_dark_mode()
        }
        (None, crate::config::ThemePreference::Light) => false,
        (None, crate::config::ThemePreference::Dark) => true,
    }
}

fn apply_theme(hwnd: HWND) {
    let Some((editor, language, palette, frame_change)) =
        (unsafe { app_ptr(hwnd) }).and_then(|mut app| {
            let app = unsafe { app.as_mut() };
            let editor = app.editor.clone()?;
            let palette = Palette::for_cached_theme(app.theme, app.settings.theme);
            let frame_change = app.dark_frame_applied != palette.dark_frame;
            app.dark_frame_applied = palette.dark_frame;
            let language = app
                .tabs
                .active()
                .map_or(crate::document::Language::PlainText, |document| {
                    document.language
                });
            Some((editor, language, palette, frame_change))
        })
    else {
        return;
    };
    let _ = editor.set_base_colors(palette.editor_foreground, palette.editor_background);
    let _ =
        editor.set_line_number_colors(palette.line_number_foreground, palette.editor_background);
    let _ = editor.set_chrome_colors(
        palette.selection_background,
        palette.inactive_selection_background,
        palette.caret_line_background,
    );
    let _ = editor.set_selection_text_colors(palette.selection_foreground);
    if frame_change {
        crate::window::titlebar::apply_frame_theme(hwnd, editor.hwnd(), palette.dark_frame);
    }
    if language != crate::document::Language::PlainText {
        apply_language(hwnd, language);
    }
}

/// Copies what a title-strip paint needs out of App, creating the per-DPI fonts on first use.
/// Before chrome is built the palette is the neutral compiled one (no theme queries).
fn title_chrome(hwnd: HWND) -> (Palette, TitleFontHandles, PointerState) {
    let dpi = unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd) }.max(96);
    unsafe { app_ptr(hwnd) }
        .map(|mut app| {
            let app = unsafe { app.as_mut() };
            if app
                .title_fonts
                .as_ref()
                .is_none_or(|fonts| fonts.dpi() != dpi)
            {
                app.title_fonts = Some(crate::window::titlebar::TitleFonts::create(dpi));
            }
            (
                Palette::for_cached_theme(app.theme, app.settings.theme),
                app.title_fonts
                    .as_ref()
                    .map(crate::window::titlebar::TitleFonts::handles)
                    .unwrap_or_default(),
                app.title_pointer,
            )
        })
        .unwrap_or_else(|| {
            (
                Palette::neutral(),
                TitleFontHandles::default(),
                PointerState::default(),
            )
        })
}

fn client_title_target(hwnd: HWND, lparam: LPARAM) -> Option<HitTarget> {
    let point = crate::window::titlebar::Point::new(
        (lparam as u32 & 0xffff) as u16 as i16 as i32,
        ((lparam as u32 >> 16) & 0xffff) as u16 as i16 as i32,
    );
    Some(title_layout(hwnd).hit_test(point))
}

fn update_title_pointer(hwnd: HWND, update: impl FnOnce(PointerState) -> PointerState) {
    let changed = unsafe { app_ptr(hwnd) }.is_some_and(|mut app| {
        let app = unsafe { app.as_mut() };
        let next = update(app.title_pointer);
        let changed = next != app.title_pointer;
        app.title_pointer = next;
        changed
    });
    if changed {
        crate::window::titlebar::invalidate_strip(hwnd);
    }
}

fn run_caption_button(hwnd: HWND, target: HitTarget) {
    let command = match target {
        HitTarget::Minimize => SC_MINIMIZE,
        HitTarget::Maximize if unsafe { IsZoomed(hwnd) } != 0 => SC_RESTORE,
        HitTarget::Maximize => SC_MAXIMIZE,
        HitTarget::Close => SC_CLOSE,
        _ => return,
    };
    unsafe {
        SendMessageW(hwnd, WM_SYSCOMMAND, command as usize, 0);
    }
}

/// The status line exists only after `WM_FASTPAD_BUILD_CHROME` and only while notifications
/// are pending.
fn current_status_text(hwnd: HWND) -> Option<String> {
    let app = unsafe { app_ptr(hwnd) }?;
    let app = unsafe { app.as_ref() };
    app.status.as_ref()?;
    crate::window::status::status_text(&app.notifications)
}

fn status_bar_height(hwnd: HWND) -> i32 {
    if current_status_text(hwnd).is_none() {
        return 0;
    }
    crate::window::status::status_height(unsafe {
        windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd)
    })
}

fn status_contains(hwnd: HWND, y: i32) -> bool {
    let height = status_bar_height(hwnd);
    if height == 0 {
        return false;
    }
    let mut rect = RECT::default();
    unsafe {
        GetClientRect(hwnd, &mut rect);
    }
    y >= rect.bottom - height
}

fn dismiss_notifications(hwnd: HWND) {
    if let Some(mut app) = unsafe { app_ptr(hwnd) } {
        unsafe { app.as_mut() }.notifications.dismiss_all();
    }
    layout_editor_and_find_bar(hwnd);
    unsafe {
        InvalidateRect(hwnd, std::ptr::null(), 1);
    }
}

/// Validates the active document's current text as JSON and reports the outcome. Read-only: never
/// touches the editor's text, selection, or undo stack either way.
fn validate_active_json(hwnd: HWND) {
    let Some(editor) =
        (unsafe { app_ptr(hwnd) }).and_then(|app| unsafe { app.as_ref() }.editor.clone())
    else {
        return;
    };
    let Ok(text) = editor.text() else {
        return;
    };
    match crate::languages::validate_json(&text) {
        Ok(()) => push_notice(hwnd, "This document contains valid JSON.".to_owned()),
        Err(issue) => push_notice(hwnd, json_issue_message(&issue)),
    }
}

/// Formats the active document's full JSON text in place. Reads the current text and selection,
/// formats completely in memory first, and only on success mutates the editor: one full-buffer
/// `replace_target` bracketed by `begin_undo_action`/`end_undo_action` (Task 12's
/// `Editor::replace_all` precedent), so a single Undo restores the exact original bytes. The
/// selection is restored afterward, clamped to the (likely different) new length and then snapped
/// down to the nearest UTF-8 character boundary (`floor_char_boundary`): the pre-format byte
/// offsets have no guaranteed relationship to character boundaries in the reformatted text (JSON
/// string values keep their literal, possibly multi-byte, UTF-8 content), so clamping alone is not
/// enough to avoid handing Scintilla a mid-character position. Invalid JSON never starts an undo
/// action and leaves the document's bytes completely unchanged: the failure is detected before any
/// editor mutation is attempted.
fn format_active_json(hwnd: HWND) {
    let Some(editor) =
        (unsafe { app_ptr(hwnd) }).and_then(|app| unsafe { app.as_ref() }.editor.clone())
    else {
        return;
    };
    let Ok(text) = editor.text() else {
        return;
    };
    let selection = editor.selection().unwrap_or(0..0);
    match crate::languages::format_json(&text) {
        Ok(formatted) => {
            editor.begin_undo_action();
            let result = editor.replace_target(0..text.len(), &formatted);
            editor.end_undo_action();
            if result.is_err() {
                return;
            }
            let new_length = formatted.len();
            let start = floor_char_boundary(&formatted, selection.start.min(new_length));
            let end = floor_char_boundary(&formatted, selection.end.min(new_length));
            let _ = editor.set_selection(start..end);
        }
        Err(issue) => push_notice(hwnd, json_issue_message(&issue)),
    }
}

/// The largest UTF-8 character boundary in `text` at or before `position`. `position` may be
/// `text.len()` (a valid boundary, the end of the string) but must not exceed it. Used to snap a
/// byte offset carried over from a *different* string (the pre-format text) into a valid position
/// in `text` (the post-format text): after formatting, an old offset has no guaranteed
/// relationship to character boundaries in the reformatted bytes — `serde_json` passes multi-byte
/// UTF-8 through unescaped, so it can coincidentally land mid-character. `0` is always a valid
/// boundary, so this loop always terminates.
fn floor_char_boundary(text: &str, mut position: usize) -> usize {
    while !text.is_char_boundary(position) {
        position -= 1;
    }
    position
}

fn json_issue_message(issue: &crate::languages::JsonIssue) -> String {
    if issue.line == 0 && issue.column == 0 {
        format!("FastPad could not process this JSON: {}", issue.message)
    } else {
        format!(
            "This document is not valid JSON (line {}, column {}): {}",
            issue.line, issue.column, issue.message
        )
    }
}

fn handle_open_request(hwnd: HWND) -> LRESULT {
    let request = unsafe { app_ptr(hwnd) }.and_then(|mut app| {
        let app = unsafe { app.as_mut() };
        if app.launch_open_completed {
            return None;
        }
        app.launch_open_completed = true;
        Some(app.launch.request.clone())
    });
    let Some(request) = request else {
        return 0;
    };
    match request {
        crate::launch::LaunchRequest::Open(path) => {
            let path = std::path::Path::new(&path);
            match App::open_path(hwnd, path) {
                Ok(()) => return 0,
                Err(error) => {
                    report_open_failure(hwnd, path, &error);
                    // The requested-file unit is finished either way; the milestone stays honest.
                    unsafe {
                        let _ = record_milestone(hwnd, Milestone::FileLoaded);
                    }
                }
            }
        }
        crate::launch::LaunchRequest::New => unsafe {
            let _ = record_milestone(hwnd, Milestone::FileLoaded);
        },
    }
    if unsafe { window_identity(hwnd) }.is_some_and(|identity| identity.is_live_for(hwnd)) {
        unsafe {
            PostMessageW(hwnd, crate::window::WM_FASTPAD_APPLY_LANGUAGE, 0, 0);
        }
    }
    0
}

pub(crate) fn open_path(hwnd: HWND, path: &std::path::Path) -> Result<()> {
    let identity = unsafe { window_identity(hwnd) }.ok_or(crate::FastPadError::Invariant(
        "main window app state was not available",
    ))?;
    if file_population_active(hwnd) {
        return Err(crate::FastPadError::Invariant(
            "file population is already active",
        ));
    }
    let existing = unsafe { app_ptr(hwnd) }.and_then(|app| {
        let app = unsafe { app.as_ref() };
        app.tabs
            .find_path(path)
            .map(|id| (id, app.tabs.view().snapshot().revision))
    });
    if let Some((id, revision)) = existing {
        return if activate_document(hwnd, id, revision) {
            unsafe {
                PostMessageW(hwnd, crate::window::WM_FASTPAD_APPLY_LANGUAGE, 0, 0);
            }
            Ok(())
        } else {
            Err(crate::FastPadError::Invariant(
                "existing file could not be activated",
            ))
        };
    }

    // All fallible disk/decode/text validation occurs before touching active state.
    let loaded = crate::file::loader::load(path)?;
    // A NUL byte cannot round-trip through Scintilla's UTF-8 buffer: the file is unsupported.
    std::ffi::CString::new(loaded.text.as_str())
        .map_err(|_| crate::FastPadError::UnsupportedEncoding)?;
    let (editor, candidate_ids) = {
        let mut app = unsafe { app_ptr(hwnd) }.ok_or(crate::FastPadError::Invariant(
            "main window app state was not available",
        ))?;
        let app = unsafe { app.as_mut() };
        let editor = app
            .editor
            .clone()
            .ok_or(crate::FastPadError::Invariant("editor was not initialized"))?;
        let candidate_ids = app
            .tabs
            .active()
            .filter(|active| !active.dirty && active.path.is_none())
            .map(|active| (active.id, active.recovery_id));
        (editor, candidate_ids)
    };
    // With no tab open this is the hidden placeholder document.
    let previous = editor.current_document()?;
    let reused_ids = match candidate_ids {
        Some(ids) if editor.text()?.is_empty() => Some(ids),
        _ => None,
    };
    let reuse = reused_ids.is_some();
    if !identity.is_live_for(hwnd) {
        return Err(crate::FastPadError::Invariant(
            "main window was destroyed during file open",
        ));
    }
    let (id, recovery_id) = if let Some(ids) = reused_ids {
        ids
    } else {
        let mut app = unsafe { app_ptr(hwnd) }.ok_or(crate::FastPadError::Invariant(
            "main window app state was not available",
        ))?;
        unsafe { app.as_mut() }.allocate_document_identity()
    };
    let mut document = Document::untitled(id, recovery_id, editor.create_document()?);
    document.path = Some(loaded.path);
    document.encoding = loaded.encoding;
    if !identity.is_live_for(hwnd) {
        return Err(crate::FastPadError::Invariant(
            "main window was destroyed during file open",
        ));
    }
    unsafe { app_ptr(hwnd).unwrap().as_mut() }.populating_file = true;
    let result = editor
        .use_document(&document.handle)
        .and_then(|_| editor.populate_clean(&loaded.text));
    if result.is_err() && identity.is_live_for(hwnd) {
        let _ = editor.use_document(&previous);
    }
    if !identity.is_live_for(hwnd) {
        return Err(crate::FastPadError::Invariant(
            "main window was destroyed during file population",
        ));
    }
    let (commit, retired) = {
        let mut app = unsafe { app_ptr(hwnd) }.ok_or(crate::FastPadError::Invariant(
            "main window app state was not available",
        ))?;
        let app = unsafe { app.as_mut() };
        app.populating_file = false;
        result?;
        if reuse {
            let retired = app.tabs.replace_active_untitled(document);
            let commit = if retired.is_some() {
                Ok(())
            } else {
                Err(crate::FastPadError::Invariant(
                    "the reused tab closed during file open",
                ))
            };
            (commit, retired)
        } else {
            (
                app.tabs
                    .push(document)
                    .map_err(|_| crate::FastPadError::Invariant("duplicate document path")),
                None,
            )
        }
    };
    drop(retired);
    if commit.is_err() {
        let _ = editor.use_document(&previous);
    }
    commit?;
    unsafe {
        let _ = record_milestone(hwnd, Milestone::FileLoaded);
        PostMessageW(hwnd, crate::window::WM_FASTPAD_APPLY_LANGUAGE, 0, 0);
    }
    refresh_tabs(hwnd);
    Ok(())
}

fn create_new_document(hwnd: HWND) -> Result<()> {
    let identity = unsafe { window_identity(hwnd) }.ok_or(crate::FastPadError::Invariant(
        "main window app state was not available",
    ))?;
    let (editor, id, recovery_id) = {
        let Some(mut app) = (unsafe { app_ptr(hwnd) }) else {
            return Err(crate::FastPadError::Invariant(
                "main window app state was not available",
            ));
        };
        let app = unsafe { app.as_mut() };
        let editor = app
            .editor
            .clone()
            .ok_or(crate::FastPadError::Invariant("editor was not initialized"))?;
        let (id, recovery_id) = app.allocate_document_identity();
        (editor, id, recovery_id)
    };

    let document = Document::untitled(id, recovery_id, editor.create_document()?);
    editor.use_document(&document.handle)?;
    if !identity.is_live_for(hwnd) {
        return Err(crate::FastPadError::Invariant(
            "main window was destroyed while creating a document",
        ));
    }
    let Some(mut app) = (unsafe { app_ptr(hwnd) }) else {
        return Err(crate::FastPadError::Invariant(
            "main window app state was not available",
        ));
    };
    let app = unsafe { app.as_mut() };
    app.tabs
        .push(document)
        .map_err(|_| crate::FastPadError::Invariant("duplicate document path"))?;
    refresh_tabs(hwnd);
    Ok(())
}

fn activate_tab(hwnd: HWND, index: usize) {
    let target = unsafe { app_ptr(hwnd) }.and_then(|app| {
        let view = unsafe { app.as_ref() }.tabs.view().snapshot();
        view.tabs.get(index).map(|tab| (tab.id, view.revision))
    });
    if let Some((id, revision)) = target {
        let _ = activate_document(hwnd, id, revision);
    }
}

/// Activates the tab after (or before) the active one, wrapping around the ends of the strip.
fn cycle_tab(hwnd: HWND, forward: bool) {
    let Some(active) =
        (unsafe { app_ptr(hwnd) }).map(|app| unsafe { app.as_ref() }.tabs.active_index())
    else {
        return;
    };
    let count = tab_count(hwnd);
    if count < 2 {
        return;
    }
    let target = if forward {
        (active + 1) % count
    } else {
        (active + count - 1) % count
    };
    activate_tab(hwnd, target);
}

fn activate_document(hwnd: HWND, id: DocumentId, revision: u64) -> bool {
    if file_population_active(hwnd) {
        return false;
    }
    let Some(identity) = (unsafe { window_identity(hwnd) }) else {
        return false;
    };
    let target = unsafe { app_ptr(hwnd) }.and_then(|mut app| {
        let app = unsafe { app.as_mut() };
        if app.tabs.view().snapshot().revision != revision {
            return None;
        }
        let editor = app.editor.clone()?;
        app.tabs.activate(id).ok()?;
        Some((editor, app.tabs.active_handle()?.clone()))
    });
    let Some((editor, handle)) = target else {
        return false;
    };
    if editor.use_document(&handle).is_err() || !identity.is_live_for(hwnd) {
        return false;
    }
    refresh_tabs(hwnd);
    true
}

fn handle_accessible_select(hwnd: HWND, lparam: LPARAM) -> LRESULT {
    if lparam == 0 {
        return 0;
    }
    let request = unsafe { *(lparam as *const AccessibleSelectRequest) };
    isize::from(activate_document(
        hwnd,
        request.document_id,
        request.revision,
    ))
}

fn close_active_document(hwnd: HWND) {
    let Some(identity) = (unsafe { window_identity(hwnd) }) else {
        return;
    };
    let snapshot = unsafe { app_ptr(hwnd) }.and_then(|app| {
        let app = unsafe { app.as_ref() };
        let review = app.tabs.active_close_review()?;
        let document = app.tabs.document(review.id)?;
        Some((review, document.dirty, document.title(), app.editor.clone()))
    });
    let Some((review, dirty, title, Some(editor))) = snapshot else {
        return;
    };
    let decision = if dirty {
        prompt_close_decision(hwnd, &title)
    } else {
        CloseDecision::Discard
    };
    if decision == CloseDecision::Cancel || !identity.is_live_for(hwnd) {
        return;
    }
    // Saving clears the dirty flag, which advances the generation the prompt reviewed.
    let review = if decision == CloseDecision::Save {
        if !save_reviewed_document(hwnd, review.id) {
            return;
        }
        match unsafe { app_ptr(hwnd) }
            .and_then(|app| unsafe { app.as_ref() }.tabs.active_close_review())
        {
            Some(saved) if saved.id == review.id => saved,
            _ => return,
        }
    } else {
        review
    };

    let switched = unsafe { app_ptr(hwnd) }.and_then(|mut app| {
        let app = unsafe { app.as_mut() };
        let closed = app.tabs.close_reviewed(review, decision).ok()?;
        let snapshots = app
            .recovery_root
            .as_deref()
            .map(|root| {
                crate::recovery::snapshots_removed_on_close(
                    root,
                    &closed,
                    decision == CloseDecision::Discard,
                )
            })
            .unwrap_or_default();
        Some((closed, app.tabs.active_handle().cloned(), snapshots))
    });
    let Some((closed, active, snapshots)) = switched else {
        return;
    };
    // The view keeps its own reference to whatever it shows, so the last closed document is
    // swapped for an empty placeholder rather than lingering in the hidden editor.
    let active = active.or_else(|| editor.create_document().ok());
    if let Some(active) = active {
        let _ = editor.use_document(&active);
    }
    drop(closed);
    crate::recovery::remove_snapshot_files(&snapshots);
    if identity.is_live_for(hwnd) {
        refresh_tabs(hwnd);
    }
}

/// Closes tabs one at a time, reviewing each dirty one, until none remain or a close is refused.
fn close_all_documents(hwnd: HWND) {
    let Some(identity) = (unsafe { window_identity(hwnd) }) else {
        return;
    };
    loop {
        let before = tab_count(hwnd);
        if before == 0 {
            return;
        }
        close_active_document(hwnd);
        if !identity.is_live_for(hwnd) || tab_count(hwnd) >= before {
            return;
        }
    }
}

/// Activates `id` (the prompt's modal loop can have activated another tab) and saves it. Reports
/// success only when that same document is the active, no-longer-dirty one afterwards.
fn save_reviewed_document(hwnd: HWND, id: DocumentId) -> bool {
    if !activate_document_by_id(hwnd, id) || !save_active_document(hwnd) {
        return false;
    }
    unsafe { app_ptr(hwnd) }.is_some_and(|app| {
        let app = unsafe { app.as_ref() };
        app.tabs.active().is_some_and(|active| active.id == id)
            && app
                .tabs
                .document(id)
                .is_some_and(|document| !document.dirty)
    })
}

/// Makes `id` the active document, or reports false when it no longer exists.
fn activate_document_by_id(hwnd: HWND, id: DocumentId) -> bool {
    let target = unsafe { app_ptr(hwnd) }.and_then(|app| {
        let app = unsafe { app.as_ref() };
        if app.tabs.active().is_some_and(|active| active.id == id) {
            return Some(None);
        }
        app.tabs.document(id)?;
        Some(Some(app.tabs.view().snapshot().revision))
    });
    match target {
        Some(None) => true,
        Some(Some(revision)) => activate_document(hwnd, id, revision),
        None => false,
    }
}

fn save_active_document(hwnd: HWND) -> bool {
    let Some(identity) = (unsafe { window_identity(hwnd) }) else {
        return false;
    };
    let has_path = unsafe { app_ptr(hwnd) }
        .and_then(|app| Some(unsafe { app.as_ref() }.tabs.active()?.path.is_some()));
    match has_path {
        Some(true) => complete_save(hwnd, &identity, None),
        Some(false) => save_active_document_as(hwnd),
        None => false,
    }
}

fn save_active_document_as(hwnd: HWND) -> bool {
    let Some(identity) = (unsafe { window_identity(hwnd) }) else {
        return false;
    };
    let Some((target, suggested)) = (unsafe { app_ptr(hwnd) }).and_then(|app| {
        let app = unsafe { app.as_ref() };
        let document = app.tabs.active()?;
        Some((
            document.id,
            document
                .path
                .as_deref()
                .and_then(std::path::Path::file_name)
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Untitled.txt".to_owned()),
        ))
    }) else {
        return false;
    };
    // Modal Show reenters the window procedure. Only an owned identity crosses it.
    let selection = crate::window::modal::choose_save_path(hwnd, &suggested);
    if !identity.is_live_for(hwnd) {
        return false;
    }
    let path = match selection {
        Ok(Some(path)) => path,
        // Cancelling the dialog is not an error and says nothing.
        Ok(None) => return false,
        Err(error) => {
            push_notice(
                hwnd,
                format!("FastPad could not open the Save As dialog: {error}"),
            );
            return false;
        }
    };
    // The dialog's modal loop can have activated another tab; save the document that was chosen.
    if !activate_document_by_id(hwnd, target) {
        return false;
    }
    complete_save(hwnd, &identity, Some(path))
}

/// Test-only entry point that drives Save As with an explicit path, bypassing the native dialog.
/// The real dialog interaction is covered by `select_save_file`'s tests; this exists because the
/// shell's own "Confirm Save As" collision handling for an existing target could not be driven
/// reliably through synthetic window messages on this host, so the collision-rejection tail below
/// (identical production code `save_active_document_as` reaches after a real dialog selection) is
/// exercised directly instead, mirroring `open_path`'s existing non-dialog test entry point.
#[cfg(test)]
#[allow(
    dead_code,
    reason = "consumed by the source-linked save_file integration target"
)]
pub(crate) fn save_path_as(hwnd: HWND, path: &std::path::Path) {
    let Some(identity) = (unsafe { window_identity(hwnd) }) else {
        return;
    };
    complete_save(hwnd, &identity, Some(path.to_path_buf()));
}

/// Shared tail of plain Save and Save As. `new_path` is `Some` only for Save As: the active
/// document's path is renamed (and checked against other open tabs' canonical paths) before the
/// write. Plain Save (`new_path: None`) writes to the document's existing path unchanged.
///
/// For Save As, every failure after a successful rename (missing editor, a failed
/// `editor.text()` read, or a failed `save_atomic`) reverts the tab's path back to whatever it
/// held before this call: a failed write must never leave the tab claiming a path nothing was
/// actually written to, orphaning it from the path it was last genuinely saved at.
fn complete_save(
    hwnd: HWND,
    identity: &WindowIdentity,
    new_path: Option<std::path::PathBuf>,
) -> bool {
    let is_save_as = new_path.is_some();
    let mut original_path: Option<std::path::PathBuf> = None;
    if let Some(path) = new_path {
        original_path = unsafe { app_ptr(hwnd) }
            .and_then(|app| unsafe { app.as_ref() }.tabs.active()?.path.clone());
        let outcome = unsafe { app_ptr(hwnd) }
            .map(|mut app| unsafe { app.as_mut() }.tabs.set_active_path(path));
        match outcome {
            Some(Ok(())) => {}
            Some(Err(_)) => {
                push_notice(
                    hwnd,
                    "This file is already open in another tab. Choose a different name.".to_owned(),
                );
                return false;
            }
            None => return false,
        }
    }
    if !identity.is_live_for(hwnd) {
        return false;
    }
    let Some((editor, path, encoding)) = (unsafe { app_ptr(hwnd) }).and_then(|app| {
        let app = unsafe { app.as_ref() };
        let editor = app.editor.clone()?;
        let document = app.tabs.active()?;
        Some((editor, document.path.clone()?, document.encoding))
    }) else {
        if is_save_as {
            revert_active_path(hwnd, original_path);
        }
        return false;
    };
    let Ok(text) = editor.text() else {
        if is_save_as {
            revert_active_path(hwnd, original_path);
        }
        return false;
    };
    let bytes = crate::file::encoding::encode(&text, encoding);
    let result = crate::file::saver::save_atomic(&path, &bytes);
    if !identity.is_live_for(hwnd) {
        return false;
    }
    match result {
        Ok(()) => {
            remove_saved_document_snapshots(hwnd);
            editor.set_save_point();
            // A recovered tab undone to the empty save point gets no save-point notification.
            let cleaned = identity.is_live_for(hwnd)
                && unsafe { app_ptr(hwnd) }
                    .is_some_and(|mut app| unsafe { app.as_mut() }.tabs.set_active_dirty(false));
            if cleaned && !is_save_as {
                invalidate_title_strip(hwnd);
            }
            if is_save_as {
                unsafe {
                    PostMessageW(hwnd, crate::window::WM_FASTPAD_APPLY_LANGUAGE, 0, 0);
                }
                invalidate_title_strip(hwnd);
            }
            true
        }
        Err(_) => {
            if is_save_as {
                revert_active_path(hwnd, original_path);
            }
            push_notice(
                hwnd,
                "FastPad could not save this file. The previous version on disk was not modified."
                    .to_owned(),
            );
            false
        }
    }
}

fn revert_active_path(hwnd: HWND, original_path: Option<std::path::PathBuf>) {
    if let Some(mut app) = unsafe { app_ptr(hwnd) } {
        unsafe { app.as_mut() }
            .tabs
            .revert_active_path(original_path);
    }
}

/// Returns the documents explicitly discarded, or `None` when the close was cancelled.
fn review_dirty_documents(hwnd: HWND) -> Option<Vec<DocumentId>> {
    let identity = unsafe { window_identity(hwnd) }?;
    let mut reviewed = Vec::<CloseReviewKey>::new();
    let mut discarded = Vec::<DocumentId>::new();
    loop {
        let pending = unsafe { app_ptr(hwnd) }.and_then(|app| {
            let app = unsafe { app.as_ref() };
            let review = app.tabs.next_dirty_review(&reviewed)?;
            let title = app.tabs.document(review.id)?.title();
            Some((review, title))
        });
        let Some((review, title)) = pending else {
            return Some(discarded);
        };
        let decision = prompt_close_decision(hwnd, &title);
        if decision == CloseDecision::Cancel || !identity.is_live_for(hwnd) {
            return None;
        }
        // A failed or cancelled save aborts the whole window close rather than losing the text.
        if decision == CloseDecision::Save {
            if !save_reviewed_document(hwnd, review.id) {
                return None;
            }
            continue;
        }
        let current = unsafe { app_ptr(hwnd) }
            .map(|app| unsafe { app.as_ref() }.tabs.dirty_review_is_current(review))
            .unwrap_or(false);
        if current {
            reviewed.push(review.key());
            discarded.retain(|id| *id != review.id);
            if decision == CloseDecision::Discard {
                discarded.push(review.id);
            }
        }
    }
}

fn start_recovery_timer(hwnd: HWND) {
    let Some(interval) = (unsafe { app_ptr(hwnd) }).map(|mut app| {
        let app = unsafe { app.as_mut() };
        if app.recovery_owner.is_none() {
            app.recovery_owner = crate::recovery::create_owner_mutex(app.recovery_owner_id()).ok();
        }
        app.settings.recovery_interval_seconds
    }) else {
        return;
    };
    unsafe {
        SetTimer(
            hwnd,
            crate::recovery::RECOVERY_TIMER_ID,
            crate::recovery::timer_period_ms(interval),
            None,
        );
    }
}

/// Resolves (once) and returns the Recovery directory; tests pre-seed `App::recovery_root`.
fn recovery_root(hwnd: HWND) -> Option<std::path::PathBuf> {
    let mut app = unsafe { app_ptr(hwnd) }?;
    let app = unsafe { app.as_mut() };
    if app.recovery_root.is_none() {
        app.recovery_root = crate::recovery::recovery_root().ok();
    }
    app.recovery_root.clone()
}

fn snapshot_when_idle(hwnd: HWND) {
    let mut info = LASTINPUTINFO {
        cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    if unsafe { GetLastInputInfo(&mut info) } == 0
        || !crate::recovery::input_idle(info.dwTime, unsafe { GetTickCount() })
    {
        return;
    }
    snapshot_next_document(hwnd);
}

/// Writes at most one dirty document whose generation has not been recorded yet.
fn snapshot_next_document(hwnd: HWND) {
    let Some(identity) = (unsafe { window_identity(hwnd) }) else {
        return;
    };
    if file_population_active(hwnd) || crate::window::modal::modal_active(hwnd) {
        return;
    }
    let Some(root) = recovery_root(hwnd) else {
        return;
    };
    let job = unsafe { app_ptr(hwnd) }.and_then(|app| {
        let app = unsafe { app.as_ref() };
        let editor = app.editor.clone()?;
        let active = app.tabs.active()?;
        let document = crate::recovery::next_snapshot_document(
            app.tabs.documents(),
            app.last_snapshot_attempt,
        )?;
        let origin = document.recovery_origin.as_ref();
        Some(SnapshotJob {
            editor,
            id: document.id,
            generation: document.generation,
            recovery_id: document.recovery_id,
            original_path: document
                .path
                .clone()
                .or_else(|| origin.and_then(|origin| origin.original_path.clone())),
            encoding: document.encoding,
            source_snapshot: origin.map(|origin| origin.snapshot_path.clone()),
            inactive: (document.id != active.id)
                .then(|| (document.handle.clone(), active.handle.clone())),
        })
    });
    let Some(job) = job else {
        return;
    };
    // Recorded before the write so a document that keeps failing still yields the next tick.
    if let Some(mut app) = unsafe { app_ptr(hwnd) } {
        unsafe { app.as_mut() }.last_snapshot_attempt = Some(job.id);
    }
    let started = std::time::Instant::now();
    let text = match &job.inactive {
        None => job.editor.text(),
        Some((target, active)) => read_inactive_text(hwnd, &identity, &job.editor, target, active),
    };
    let Ok(text) = text else {
        return;
    };
    if !identity.is_live_for(hwnd) {
        return;
    }
    let snapshot =
        crate::recovery::Snapshot::new(job.recovery_id, job.original_path, job.encoding, text);
    let written = crate::recovery::write_snapshot(&root, &snapshot);
    drop(snapshot);
    let elapsed = started.elapsed();
    if let Some(mut app) = unsafe { app_ptr(hwnd) } {
        let app = unsafe { app.as_mut() };
        app.last_snapshot_duration = Some(elapsed);
        if written.is_ok() {
            app.tabs.record_recovery_generation(job.id, job.generation);
        }
    }
    // Once a recovered tab has its own snapshot, its source would only resurrect a stale duplicate.
    if let (Ok(written), Some(source)) = (written, job.source_snapshot)
        && written != source
    {
        crate::recovery::remove_snapshot_files(&[source]);
    }
}

struct SnapshotJob {
    editor: Editor,
    id: DocumentId,
    generation: u64,
    recovery_id: RecoveryId,
    original_path: Option<std::path::PathBuf>,
    encoding: crate::file::encoding::Encoding,
    source_snapshot: Option<std::path::PathBuf>,
    inactive: Option<(crate::editor::EditorDocument, crate::editor::EditorDocument)>,
}

/// Scintilla can only read the document shown in the view, so an inactive tab is swapped in and
/// out with notifications suppressed, restoring the visible selection and scroll position.
fn read_inactive_text(
    hwnd: HWND,
    identity: &WindowIdentity,
    editor: &Editor,
    target: &crate::editor::EditorDocument,
    active: &crate::editor::EditorDocument,
) -> Result<String> {
    use crate::editor::scintilla_constants::{SCI_GETFIRSTVISIBLELINE, SCI_SETFIRSTVISIBLELINE};
    let selection = editor.selection();
    let first_line = unsafe { SendMessageW(editor.hwnd(), SCI_GETFIRSTVISIBLELINE, 0, 0) };
    set_file_population(hwnd, true);
    let text = editor.use_document(target).and_then(|_| editor.text());
    let restored = if identity.is_live_for(hwnd) {
        editor.use_document(active)
    } else {
        Err(crate::FastPadError::Invariant(
            "main window was destroyed during a recovery snapshot",
        ))
    };
    if restored.is_ok() {
        if let Ok(selection) = selection {
            let _ = editor.set_selection(selection);
        }
        unsafe {
            SendMessageW(
                editor.hwnd(),
                SCI_SETFIRSTVISIBLELINE,
                first_line as usize,
                0,
            );
        }
    }
    if identity.is_live_for(hwnd) {
        set_file_population(hwnd, false);
    }
    restored?;
    text
}

fn set_file_population(hwnd: HWND, active: bool) {
    if let Some(mut app) = unsafe { app_ptr(hwnd) } {
        unsafe { app.as_mut() }.populating_file = active;
    }
}

/// Opens every valid foreign snapshot as a recovered tab and reports them with one notice.
fn recover_snapshots(hwnd: HWND) {
    let Some(identity) = (unsafe { window_identity(hwnd) }) else {
        return;
    };
    let Some(root) = recovery_root(hwnd) else {
        return;
    };
    let Ok(candidates) =
        crate::recovery::discover_snapshots_with(&root, crate::recovery::owner_is_alive)
    else {
        return;
    };
    let mut recovered = 0;
    for candidate in candidates {
        let owned = unsafe { app_ptr(hwnd) }.is_none_or(|app| {
            unsafe { app.as_ref() }.owns_recovery_id(candidate.snapshot.recovery_id)
        });
        if owned {
            continue;
        }
        if open_recovered_snapshot(hwnd, &identity, candidate).is_ok() {
            recovered += 1;
        }
        if !identity.is_live_for(hwnd) {
            return;
        }
    }
    if recovered == 0 {
        return;
    }
    let chrome_built = unsafe { app_ptr(hwnd) }.is_some_and(|mut app| {
        let app = unsafe { app.as_mut() };
        app.notifications
            .push(crate::recovery::recovered_notice(recovered));
        app.status.is_some()
    });
    if chrome_built {
        layout_editor_and_find_bar(hwnd);
    }
    unsafe {
        InvalidateRect(hwnd, std::ptr::null(), 1);
    }
}

const IPC_UNAVAILABLE_NOTICE: &str =
    "FastPad could not start its single-instance listener; later launches open separate windows.";

/// Binds the pipe server only for the process that owns the session instance mutex.
fn start_ipc_server_with(hwnd: HWND, bind: impl FnOnce() -> Result<crate::ipc::IpcServer>) {
    let Some(mut app) = (unsafe { app_ptr(hwnd) }) else {
        return;
    };
    // Binding makes no window calls, so this App borrow cannot be re-entered.
    let app = unsafe { app.as_mut() };
    if app.instance_mutex.is_none() || app.ipc.is_some() {
        return;
    }
    match bind() {
        Ok(server) => app.ipc = Some(server),
        Err(_) => stop_ipc(app),
    }
}

fn stop_ipc(app: &mut App) {
    app.ipc = None;
    // Releasing the mutex sends later launches to independent processes instead of a dead pipe.
    app.instance_mutex = None;
    app.notifications.push(IPC_UNAVAILABLE_NOTICE);
}

/// Copies the pipe event out of App for one wait; callers must not dispatch while using it.
pub(crate) fn ipc_wait_handle(
    hwnd: HWND,
    identity: &WindowIdentity,
) -> Option<windows_sys::Win32::Foundation::HANDLE> {
    if !identity.is_live_for(hwnd) {
        return None;
    }
    let app = unsafe { app_ptr(hwnd) }?;
    unsafe { app.as_ref() }
        .ipc
        .as_ref()
        .map(crate::ipc::IpcServer::event)
}

/// Services a signaled pipe event: queues decoded requests and posts one drain message.
pub(crate) fn service_ipc(hwnd: HWND, identity: &WindowIdentity) {
    if !identity.is_live_for(hwnd) {
        return;
    }
    let Some(mut app) = (unsafe { app_ptr(hwnd) }) else {
        return;
    };
    let queued = {
        let app = unsafe { app.as_mut() };
        let Some(server) = app.ipc.as_mut() else {
            return;
        };
        match server.poll() {
            Ok(requests) => {
                let queued = !requests.is_empty();
                app.ipc_requests.extend(requests);
                Some(queued)
            }
            Err(_) => {
                stop_ipc(app);
                None
            }
        }
    };
    match queued {
        Some(true) => unsafe {
            PostMessageW(hwnd, crate::window::WM_FASTPAD_IPC_REQUEST, 0, 0);
        },
        Some(false) => {}
        None => refresh_notifications(hwnd),
    }
}

fn handle_ipc_requests(hwnd: HWND) -> LRESULT {
    let Some(identity) = (unsafe { window_identity(hwnd) }) else {
        return 0;
    };
    let requests = unsafe { app_ptr(hwnd) }
        .map(|mut app| std::mem::take(&mut unsafe { app.as_mut() }.ipc_requests))
        .unwrap_or_default();
    for request in requests {
        if !identity.is_live_for(hwnd) {
            return 0;
        }
        match request {
            crate::ipc::IpcRequest::Open(path) => {
                if let Err(error) = App::open_path(hwnd, &path) {
                    report_open_failure(hwnd, &path, &error);
                }
            }
            crate::ipc::IpcRequest::New => execute_command(hwnd, CommandId::New),
            crate::ipc::IpcRequest::Activate => {}
        }
        if identity.is_live_for(hwnd) {
            bring_to_foreground(hwnd);
        }
    }
    0
}

fn bring_to_foreground(hwnd: HWND) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IsIconic, SW_RESTORE, SetForegroundWindow, ShowWindow,
    };
    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
        SetForegroundWindow(hwnd);
    }
}

/// Reports a rejected Open (missing file, unsupported encoding, NUL bytes) by naming the file.
fn report_open_failure(hwnd: HWND, path: &std::path::Path, error: &crate::FastPadError) {
    push_notice(
        hwnd,
        format!("FastPad could not open {}: {error}", path.display()),
    );
}

fn push_notice(hwnd: HWND, message: String) {
    if let Some(mut app) = unsafe { app_ptr(hwnd) } {
        unsafe { app.as_mut() }.notifications.push(message);
    }
    refresh_notifications(hwnd);
}

fn refresh_notifications(hwnd: HWND) {
    let chrome_built =
        unsafe { app_ptr(hwnd) }.is_some_and(|app| unsafe { app.as_ref() }.status.is_some());
    if chrome_built {
        layout_editor_and_find_bar(hwnd);
    }
    unsafe {
        InvalidateRect(hwnd, std::ptr::null(), 1);
    }
}

fn open_recovered_snapshot(
    hwnd: HWND,
    identity: &WindowIdentity,
    candidate: crate::recovery::SnapshotCandidate,
) -> Result<()> {
    if file_population_active(hwnd) {
        return Err(crate::FastPadError::Invariant(
            "file population is already active",
        ));
    }
    let crate::recovery::SnapshotCandidate { path, snapshot } = candidate;
    let (editor, id, recovery_id) = {
        let mut app = unsafe { app_ptr(hwnd) }.ok_or(crate::FastPadError::Invariant(
            "main window app state was not available",
        ))?;
        let app = unsafe { app.as_mut() };
        let editor = app
            .editor
            .clone()
            .ok_or(crate::FastPadError::Invariant("editor was not initialized"))?;
        let (id, recovery_id) = app.allocate_document_identity();
        (editor, id, recovery_id)
    };
    let previous = editor.current_document()?;
    let mut document = Document::untitled(id, recovery_id, editor.create_document()?);
    document.encoding = snapshot.encoding;
    document.dirty = true;
    document.recovery_generation = Some(document.generation);
    document.recovery_origin = Some(crate::document::RecoveryOrigin {
        snapshot_path: path,
        original_path: snapshot.original_path,
    });
    if !identity.is_live_for(hwnd) {
        return Err(crate::FastPadError::Invariant(
            "main window was destroyed during recovery",
        ));
    }
    set_file_population(hwnd, true);
    // Undo collection stays on so the loaded text leaves the save point: the tab starts dirty.
    let result = editor
        .use_document(&document.handle)
        .and_then(|_| editor.set_text(&snapshot.text));
    drop(snapshot.text);
    if result.is_err() && identity.is_live_for(hwnd) {
        let _ = editor.use_document(&previous);
    }
    if !identity.is_live_for(hwnd) {
        return Err(crate::FastPadError::Invariant(
            "main window was destroyed during recovery",
        ));
    }
    set_file_population(hwnd, false);
    result?;
    let pushed = unsafe { app_ptr(hwnd) }
        .ok_or(crate::FastPadError::Invariant(
            "main window app state was not available",
        ))
        .and_then(|mut app| {
            unsafe { app.as_mut() }
                .tabs
                .push(document)
                .map_err(|_| crate::FastPadError::Invariant("duplicate document path"))
        });
    if pushed.is_err() {
        let _ = editor.use_document(&previous);
    }
    pushed?;
    refresh_tabs(hwnd);
    Ok(())
}

/// A successful save supersedes both the document's own snapshot and any recovery source.
fn remove_saved_document_snapshots(hwnd: HWND) {
    let files = unsafe { app_ptr(hwnd) }.map(|mut app| {
        let app = unsafe { app.as_mut() };
        let own = app.recovery_root.as_deref().map(|root| {
            let active = app.tabs.active()?;
            Some(crate::recovery::snapshot::snapshot_path(
                root,
                active.recovery_id,
            ))
        });
        let source = app
            .tabs
            .take_active_recovery_origin()
            .map(|origin| origin.snapshot_path);
        own.flatten().into_iter().chain(source).collect::<Vec<_>>()
    });
    crate::recovery::remove_snapshot_files(&files.unwrap_or_default());
}

fn remove_session_snapshots(hwnd: HWND, discarded: &[DocumentId]) {
    let files = unsafe { app_ptr(hwnd) }.and_then(|app| {
        let app = unsafe { app.as_ref() };
        let root = app.recovery_root.as_deref()?;
        Some(
            app.tabs
                .documents()
                .flat_map(|document| {
                    crate::recovery::snapshots_removed_on_close(
                        root,
                        document,
                        discarded.contains(&document.id),
                    )
                })
                .collect::<Vec<_>>(),
        )
    });
    crate::recovery::remove_snapshot_files(&files.unwrap_or_default());
}

/// Services the pipe and handles everything already queued, before a close review begins.
fn drain_ipc_requests(hwnd: HWND) {
    let Some(identity) = (unsafe { window_identity(hwnd) }) else {
        return;
    };
    service_ipc(hwnd, &identity);
    if identity.is_live_for(hwnd) {
        handle_ipc_requests(hwnd);
    }
}

/// Stops accepting forwarded launches before releasing the mutex that invites the next primary to
/// bind the pipe; the reverse order would let it bind while this server still exists.
fn shutdown_ipc(hwnd: HWND) {
    let Some(mut app) = (unsafe { app_ptr(hwnd) }) else {
        return;
    };
    let app = unsafe { app.as_mut() };
    drop(app.ipc.take());
    drop(app.instance_mutex.take());
}

fn clear_documents_for_shutdown(hwnd: HWND) {
    if let Some(mut app) = unsafe { app_ptr(hwnd) } {
        unsafe { app.as_mut() }.tabs.clear_for_shutdown();
    }
}

fn handle_editor_notification(hwnd: HWND, lparam: LPARAM) {
    if file_population_active(hwnd) {
        return;
    }
    if lparam == 0 {
        return;
    }
    let notification = unsafe { &*(lparam as *const NMHDR) };
    if unsafe { editor_hwnd(hwnd) } != Some(notification.hwndFrom) {
        return;
    }
    if notification.code == crate::editor::scintilla_constants::SCN_ZOOM {
        with_editor(hwnd, |editor| {
            let _ = editor.remeasure_line_numbers();
        });
        return;
    }
    if notification.code == crate::editor::scintilla_constants::SCN_MODIFIED {
        let modification = unsafe { &*(lparam as *const TextModificationNotification) };
        let text_changes = crate::editor::scintilla_constants::SC_MOD_INSERTTEXT
            | crate::editor::scintilla_constants::SC_MOD_DELETETEXT;
        if modification.modification_type & text_changes as i32 != 0
            && let Some(mut app) = unsafe { app_ptr(hwnd) }
        {
            let app = unsafe { app.as_mut() };
            app.tabs.note_active_text_change();
            if modification.lines_added != 0
                && let Some(editor) = app.editor.as_ref()
            {
                let _ = editor.refresh_line_numbers();
            }
        }
        return;
    }
    let dirty = match notification.code {
        crate::editor::scintilla_constants::SCN_SAVEPOINTLEFT => true,
        crate::editor::scintilla_constants::SCN_SAVEPOINTREACHED => false,
        _ => return,
    };
    let changed = unsafe { app_ptr(hwnd) }
        .map(|mut app| {
            let app = unsafe { app.as_mut() };
            app.tabs.set_active_dirty(dirty)
        })
        .unwrap_or(false);
    if changed {
        invalidate_title_strip(hwnd);
    }
}

#[repr(C)]
struct TextModificationNotification {
    header: NMHDR,
    position: isize,
    character: i32,
    modifiers: i32,
    modification_type: i32,
    _text: *const u8,
    _length: isize,
    lines_added: isize,
}

fn invalidate_title_strip(hwnd: HWND) {
    unsafe {
        InvalidateRect(hwnd, std::ptr::null(), 0);
    }
}

fn ensure_accessibility(hwnd: HWND) -> *mut c_void {
    unsafe { app_ptr(hwnd) }
        .map(|mut app| unsafe { app.as_mut() }.ensure_accessibility())
        .unwrap_or(std::ptr::null_mut())
}

fn show_menu_mode(hwnd: HWND) {
    let menu = unsafe { app_ptr(hwnd) }.and_then(|mut app| {
        let app = unsafe { app.as_mut() };
        if app.menu_bar.is_none() {
            app.menu_bar = MenuBar::create().ok();
        }
        app.menu_bar.as_ref().map(MenuBar::raw)
    });
    if let Some(menu) = menu {
        menus::attach_menu(hwnd, menu);
    }
}

pub(crate) unsafe fn translate_accelerator(
    hwnd: HWND,
    identity: &WindowIdentity,
    message: &windows_sys::Win32::UI::WindowsAndMessaging::MSG,
) -> bool {
    if !identity.is_live_for(hwnd) {
        return false;
    }
    if menu_activation_message(hwnd, message)
        && unsafe { PostMessageW(hwnd, WM_SYSCOMMAND, SC_KEYMENU as usize, 0) } != 0
    {
        return true;
    }
    let accelerator = unsafe { app_ptr(hwnd) }.and_then(|app| {
        unsafe { app.as_ref() }
            .accelerators
            .as_ref()
            .map(|table| table.raw())
    });
    accelerator.is_some_and(|accelerator| menus::translate_accelerator(accelerator, hwnd, message))
}

fn menu_activation_message(
    hwnd: HWND,
    message: &windows_sys::Win32::UI::WindowsAndMessaging::MSG,
) -> bool {
    let no_control_or_shift = unsafe { GetKeyState(VK_CONTROL as i32) } >= 0
        && unsafe { GetKeyState(VK_SHIFT as i32) } >= 0;
    let f10 = matches!(message.message, WM_KEYDOWN | WM_SYSKEYDOWN)
        && message.wParam == VK_F10 as usize
        && no_control_or_shift
        && unsafe { GetKeyState(VK_MENU as i32) } >= 0;
    let Some(mut app) = (unsafe { app_ptr(hwnd) }) else {
        return f10;
    };
    let app = unsafe { app.as_mut() };
    if f10 {
        app.set_menu_alt_pending(false);
        return true;
    }
    if message.message == WM_SYSKEYDOWN && message.wParam == VK_MENU as usize {
        app.set_menu_alt_pending(no_control_or_shift);
        return false;
    }
    if message.message == WM_SYSKEYUP && message.wParam == VK_MENU as usize {
        return app.take_menu_alt_pending();
    }
    if matches!(message.message, WM_KEYDOWN | WM_SYSKEYDOWN) {
        app.set_menu_alt_pending(false);
    }
    false
}

fn transient_menu_syscommand(wparam: WPARAM, lparam: LPARAM) -> bool {
    wparam & 0xfff0 == SC_KEYMENU as usize && lparam == 0
}

unsafe fn install_editor(hwnd: HWND, editor: Editor, document: Document) -> Result<()> {
    // SAFETY: The App pointer is re-fetched after editor creation so initialization never mutates
    // an App reference borrowed across a reentrant Win32 call.
    let Some(mut app) = (unsafe { app_ptr(hwnd) }) else {
        return Err(crate::FastPadError::Invariant(
            "main window app state was not available",
        ));
    };
    let app = unsafe { app.as_mut() };
    app.tabs
        .push(document)
        .map_err(|_| crate::FastPadError::Invariant("duplicate document path"))?;
    app.editor = Some(editor);
    Ok(())
}

pub(super) unsafe fn app_ptr(hwnd: HWND) -> Option<NonNull<App>> {
    // SAFETY: `GWLP_USERDATA` is written exactly once from `WM_NCCREATE` with a `Box<App>` owned
    // by the window and cleared in `WM_NCDESTROY`. Callers must not keep references alive across
    // reentrant Win32 calls; they may only copy values or perform immediate mutation.
    NonNull::new(unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App })
}

pub(super) unsafe fn window_identity(hwnd: HWND) -> Option<WindowIdentity> {
    // SAFETY: Clone only the App's stable identity token. The temporary App reference ends before
    // callers cross any reentrant Win32 boundary.
    let app = unsafe { app_ptr(hwnd) }?;
    Some(unsafe { app.as_ref() }.window_identity())
}

unsafe fn take_create_context_app(lparam: LPARAM) -> Option<Box<App>> {
    let create = unsafe { &mut *(lparam as *mut CREATESTRUCTW) };
    let context = create.lpCreateParams as *mut WindowCreateContext<App>;
    if context.is_null() {
        return None;
    }
    unsafe { (*context).value.take() }
}

fn store_app(hwnd: HWND, value: Box<App>) {
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(value) as isize);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MainWindowClass, WindowCreateContext, execute_command, handle_paint_with,
        mark_first_paint_complete, take_deferred_start_pending,
    };
    use crate::app::App;
    use crate::document::{CloseDecision, Language, RecoveryId};
    use crate::editor::scintilla_constants::SCI_GETMODIFY;
    use crate::file::encoding::Encoding;
    use crate::languages::LanguageManager;
    use crate::launch::LaunchOptions;
    use crate::perf::StartupMetrics;
    use crate::recovery::snapshot::snapshot_path;
    use crate::recovery::{Snapshot, write_snapshot};
    use crate::window::commands::CommandId;
    use crate::window::menus::answer_next_popup_menu;
    use crate::window::modal::{answer_next_close_prompt, answer_next_save_dialog};
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use windows_sys::Win32::Foundation::{HWND, LRESULT, RECT};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DestroyWindow, DispatchMessageW, GWLP_USERDATA, GetClientRect, GetWindowLongPtrW, IsWindow,
        MSG, PM_REMOVE, PeekMessageW, SendMessageW, WM_CLOSE, WM_PAINT,
    };

    /// Dispatches everything already posted to `hwnd`, leaving any WM_QUIT for the harness.
    fn pump_posted_messages(hwnd: HWND) {
        let mut message = MSG::default();
        while unsafe { PeekMessageW(&mut message, hwnd, 0, 0, PM_REMOVE) } != 0 {
            unsafe {
                DispatchMessageW(&message);
            }
        }
    }

    #[test]
    fn a_forwarded_request_is_handled_before_the_close_review_starts() {
        // Break caught: a launch forwarded just before WM_CLOSE is dropped when the window closes,
        // so the file the user double-clicked silently never opens.
        let _scintilla = load_native_scintilla();
        let window = ProductionWindow::new(make_app());
        let editor = install_test_editor(&window);
        editor.set_text("dirty").unwrap();
        app_mut(window.hwnd)
            .ipc_requests
            .push(crate::ipc::IpcRequest::New);
        let tabs_at_prompt = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&tabs_at_prompt);
        answer_next_close_prompt(move |hwnd| {
            observed.store(app_mut(hwnd).tabs.len(), Ordering::SeqCst);
            CloseDecision::Cancel
        });

        unsafe {
            SendMessageW(window.hwnd, WM_CLOSE, 0, 0);
        }

        assert_ne!(
            unsafe { IsWindow(window.hwnd) },
            0,
            "Cancel must abort the close"
        );
        assert_eq!(
            tabs_at_prompt.load(Ordering::SeqCst),
            2,
            "the queued request must be handled before the review starts"
        );
        assert!(app_mut(window.hwnd).ipc_requests.is_empty());
    }

    #[test]
    fn a_confirmed_close_releases_the_pipe_server_and_the_instance_mutex() {
        // Break caught: dropping the instance mutex before the pipe server lets the next launch
        // claim the session and fail to bind a name this process still owns.
        let _scintilla = load_native_scintilla();
        let window = ProductionWindow::new(make_app());
        let _editor = install_test_editor(&window);
        let names = crate::ipc::server::tests::unique_names();
        app_mut(window.hwnd).instance_mutex = Some(unnamed_mutex());
        super::start_ipc_server_with(window.hwnd, || {
            crate::ipc::IpcServer::bind(&names, &crate::ipc::CurrentUserAcl::current()?)
        });
        assert!(app_mut(window.hwnd).ipc.is_some());

        unsafe {
            SendMessageW(window.hwnd, WM_CLOSE, 0, 0);
        }

        assert_eq!(unsafe { IsWindow(window.hwnd) }, 0);
        crate::ipc::IpcServer::bind(&names, &crate::ipc::CurrentUserAcl::current().unwrap())
            .expect("the pipe name must be free once the window has closed");
    }

    #[test]
    fn deferred_startup_work_is_held_until_the_modal_prompt_closes() {
        // Break caught: a nested modal loop dispatches deferred chain units, so recovery can push
        // and activate tabs while a close prompt or file dialog is deciding about another one.
        let _scintilla = load_native_scintilla();
        let window = ProductionWindow::new(make_app());
        let editor = install_test_editor(&window);
        let root = RecoveryScratch::new("modal-deferred");
        write_snapshot(
            root.path(),
            &Snapshot::new(
                RecoveryId::from_u128(0x5151),
                None,
                Encoding::Utf8,
                "recovered elsewhere",
            ),
        )
        .unwrap();
        app_mut(window.hwnd).recovery_root = Some(root.path().to_path_buf());
        editor.set_text("dirty").unwrap();
        let before = app_mut(window.hwnd).tabs.len();

        answer_next_close_prompt(|hwnd| {
            unsafe {
                SendMessageW(hwnd, crate::window::WM_FASTPAD_RECOVERY, 0, 0);
            }
            CloseDecision::Cancel
        });
        execute_command(window.hwnd, CommandId::CloseTab);

        assert_eq!(
            app_mut(window.hwnd).tabs.len(),
            before,
            "the recovery unit ran inside the modal loop"
        );

        pump_posted_messages(window.hwnd);

        assert_eq!(
            app_mut(window.hwnd).tabs.len(),
            before + 1,
            "the held recovery unit must run once the modal loop ends"
        );
    }

    #[test]
    fn queued_ipc_requests_wait_for_the_overflow_menu_to_close() {
        // Break caught: the overflow menu's own modal loop dispatches a forwarded request, so a
        // new tab becomes active underneath it and the command the user picks acts on that tab
        // instead of the one they opened the menu on.
        let _scintilla = load_native_scintilla();
        let window = ProductionWindow::new(make_app());
        let editor = install_test_editor(&window);
        editor.set_text("dirty").unwrap();
        app_mut(window.hwnd)
            .ipc_requests
            .push(crate::ipc::IpcRequest::New);
        let before = app_mut(window.hwnd).tabs.len();

        answer_next_popup_menu(|hwnd| {
            unsafe {
                SendMessageW(hwnd, crate::window::WM_FASTPAD_IPC_REQUEST, 0, 0);
            }
            None
        });
        assert!(crate::window::menus::show_overflow(window.hwnd, 0, 0).is_none());

        assert_eq!(
            app_mut(window.hwnd).tabs.len(),
            before,
            "a forwarded request was handled inside the overflow menu's modal loop"
        );

        pump_posted_messages(window.hwnd);

        assert_eq!(
            app_mut(window.hwnd).tabs.len(),
            before + 1,
            "the held request must run once the overflow menu closes"
        );
    }

    #[test]
    fn closing_every_tab_hides_the_editor_until_the_empty_strip_opens_a_new_one() {
        // Break caught: the last tab being silently replaced (its close button looks inert), the
        // hidden editor still taking edits, or the empty strip's double-click and context menu
        // not reaching New and Close all tabs.
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GWL_STYLE, GetWindowLongPtrW, HTCAPTION, WM_NCLBUTTONDBLCLK, WM_NCRBUTTONUP, WS_VISIBLE,
        };
        let _scintilla = load_native_scintilla();
        let window = ProductionWindow::new(make_app());
        let editor = install_test_editor(&window);
        let editor_visible =
            || unsafe { GetWindowLongPtrW(editor.hwnd(), GWL_STYLE) } as u32 & WS_VISIBLE != 0;
        assert!(editor_visible());

        execute_command(window.hwnd, CommandId::CloseTab);

        assert!(app_mut(window.hwnd).tabs.is_empty());
        assert!(!editor_visible());
        execute_command(window.hwnd, CommandId::Paste);
        execute_command(window.hwnd, CommandId::CloseTab);
        assert_eq!(editor.text().unwrap(), "");

        unsafe {
            SendMessageW(window.hwnd, WM_NCLBUTTONDBLCLK, HTCAPTION as usize, 0);
            SendMessageW(window.hwnd, WM_NCLBUTTONDBLCLK, HTCAPTION as usize, 0);
        }
        assert_eq!(app_mut(window.hwnd).tabs.len(), 2);
        assert!(editor_visible());

        answer_next_popup_menu(|_| Some(CommandId::CloseAllTabs));
        unsafe {
            SendMessageW(window.hwnd, WM_NCRBUTTONUP, HTCAPTION as usize, 0);
        }
        assert!(app_mut(window.hwnd).tabs.is_empty());
        assert!(!editor_visible());
    }

    #[test]
    fn keyboard_shortcuts_reach_their_commands_through_the_accelerator_table() {
        // Break caught: every shortcut dead in the running app while command-level tests pass,
        // because nothing exercised the accelerator translation the message loop depends on.
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
            GetKeyboardState, SetKeyboardState, VK_CONTROL,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::{MSG, WM_KEYDOWN};
        let _scintilla = load_native_scintilla();
        let window = ProductionWindow::new(make_app());
        let editor = install_test_editor(&window);
        let identity = unsafe { super::window_identity(window.hwnd).unwrap() };
        let mut keys = [0u8; 256];
        unsafe { GetKeyboardState(keys.as_mut_ptr()) };
        let original = keys;
        keys[VK_CONTROL as usize] = 0x80;
        unsafe { SetKeyboardState(keys.as_ptr()) };
        let message = MSG {
            hwnd: editor.hwnd(),
            message: WM_KEYDOWN,
            wParam: usize::from(b'T'),
            ..Default::default()
        };
        let translated = unsafe { super::translate_accelerator(window.hwnd, &identity, &message) };
        unsafe { SetKeyboardState(original.as_ptr()) };

        assert!(translated, "Ctrl+T was not translated");
        assert_eq!(app_mut(window.hwnd).tabs.len(), 2);
    }

    #[test]
    fn tab_shortcuts_cycle_with_wrap_around_and_select_by_position() {
        // Break caught: Ctrl+Tab stopping at the last tab, or Ctrl+9 with fewer than nine tabs
        // activating some other tab instead of doing nothing.
        let _scintilla = load_native_scintilla();
        let window = ProductionWindow::new(make_app());
        let _editor = install_test_editor(&window);
        execute_command(window.hwnd, CommandId::New);
        execute_command(window.hwnd, CommandId::New);
        let active = || app_mut(window.hwnd).tabs.active_index();
        assert_eq!(active(), 2);

        execute_command(window.hwnd, CommandId::NextTab);
        assert_eq!(active(), 0);
        execute_command(window.hwnd, CommandId::PreviousTab);
        assert_eq!(active(), 2);
        execute_command(window.hwnd, CommandId::PreviousTab);
        assert_eq!(active(), 1);
        execute_command(window.hwnd, CommandId::SelectTab1);
        assert_eq!(active(), 0);
        execute_command(window.hwnd, CommandId::SelectTab3);
        assert_eq!(active(), 2);
        execute_command(window.hwnd, CommandId::SelectTab9);
        assert_eq!(active(), 2);
    }

    #[test]
    fn zoom_resizes_the_line_number_gutter_and_direction_mirrors_the_editor() {
        // Break caught: zoomed digits clipped by a gutter measured at the unzoomed size, or the
        // direction shortcuts leaving the editor window unmirrored.
        use crate::editor::scintilla_constants::SCI_GETZOOM;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GWL_EXSTYLE, GetWindowLongPtrW, WS_EX_LAYOUTRTL,
        };
        let _scintilla = load_native_scintilla();
        let window = ProductionWindow::new(make_app());
        let editor = install_test_editor(&window);
        let zoom = || unsafe { SendMessageW(editor.hwnd(), SCI_GETZOOM, 0, 0) };
        let unzoomed_width = line_number_margin_width(&editor);

        execute_command(window.hwnd, CommandId::ZoomIn);
        execute_command(window.hwnd, CommandId::ZoomIn);
        assert_eq!(zoom(), 2);
        assert!(line_number_margin_width(&editor) > unzoomed_width);
        execute_command(window.hwnd, CommandId::ZoomOut);
        assert_eq!(zoom(), 1);
        execute_command(window.hwnd, CommandId::ZoomReset);
        assert_eq!(zoom(), 0);
        assert_eq!(line_number_margin_width(&editor), unzoomed_width);

        let mirrored = || {
            (unsafe { GetWindowLongPtrW(editor.hwnd(), GWL_EXSTYLE) }) as u32 & WS_EX_LAYOUTRTL != 0
        };
        assert!(!mirrored());
        execute_command(window.hwnd, CommandId::TextRightToLeft);
        assert!(mirrored());
        execute_command(window.hwnd, CommandId::TextLeftToRight);
        assert!(!mirrored());
    }

    #[test]
    fn dragging_the_tab_scroll_thumb_scrolls_the_tabs_without_activating_one() {
        // Break caught: a scroll bar that is only painted, so overflowing tabs cannot be reached
        // without a mouse wheel, or a drag release that also clicks the tab under the pointer.
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
        };
        let _scintilla = load_native_scintilla();
        let window = ProductionWindow::new(make_app());
        let _editor = install_test_editor(&window);
        for _ in 0..40 {
            execute_command(window.hwnd, CommandId::New);
        }
        super::activate_tab(window.hwnd, 0);
        let layout = super::title_layout(window.hwnd);
        assert_eq!(layout.scroll, 0);
        let thumb = layout.scroll_thumb().expect("40 tabs overflow the strip");
        let pack = |x: i32, y: i32| (x as u16 as u32 | ((y as u16 as u32) << 16)) as isize;
        let y = thumb.center().y;
        let far_right = layout.tabs.right + 500;

        unsafe {
            SendMessageW(window.hwnd, WM_LBUTTONDOWN, 1, pack(thumb.center().x, y));
            SendMessageW(window.hwnd, WM_MOUSEMOVE, 1, pack(far_right, y));
        }
        assert_eq!(super::tab_scroll(window.hwnd), layout.max_scroll);
        unsafe {
            SendMessageW(window.hwnd, WM_LBUTTONUP, 0, pack(far_right, y));
            SendMessageW(window.hwnd, WM_MOUSEMOVE, 0, pack(layout.tabs.left, y));
        }
        assert_eq!(
            super::tab_scroll(window.hwnd),
            layout.max_scroll,
            "moving after the release must not keep dragging"
        );
        assert_eq!(app_mut(window.hwnd).tabs.active_index(), 0);

        // Pressing the track away from the thumb jumps there.
        let track = super::title_layout(window.hwnd).scroll_bar.unwrap();
        unsafe {
            SendMessageW(window.hwnd, WM_LBUTTONDOWN, 1, pack(track.left, y));
            SendMessageW(window.hwnd, WM_LBUTTONUP, 0, pack(track.left, y));
        }
        assert_eq!(super::tab_scroll(window.hwnd), 0);
    }

    #[test]
    fn queued_ipc_requests_wait_for_the_modal_prompt_to_close() {
        // Break caught: a forwarded launch is dispatched inside a close prompt (opening tabs the
        // review never saw) or dropped entirely instead of staying queued.
        let _scintilla = load_native_scintilla();
        let window = ProductionWindow::new(make_app());
        let editor = install_test_editor(&window);
        editor.set_text("dirty").unwrap();
        app_mut(window.hwnd)
            .ipc_requests
            .push(crate::ipc::IpcRequest::New);
        let before = app_mut(window.hwnd).tabs.len();

        answer_next_close_prompt(|hwnd| {
            unsafe {
                SendMessageW(hwnd, crate::window::WM_FASTPAD_IPC_REQUEST, 0, 0);
            }
            CloseDecision::Cancel
        });
        execute_command(window.hwnd, CommandId::CloseTab);

        assert_eq!(
            app_mut(window.hwnd).tabs.len(),
            before,
            "a forwarded request was handled inside the modal loop"
        );
        assert_eq!(
            app_mut(window.hwnd).ipc_requests.len(),
            1,
            "the request must stay queued while a modal loop runs"
        );

        pump_posted_messages(window.hwnd);

        assert_eq!(app_mut(window.hwnd).tabs.len(), before + 1);
        assert!(app_mut(window.hwnd).ipc_requests.is_empty());
    }

    #[test]
    fn recovery_snapshot_ticks_are_skipped_inside_a_modal_prompt() {
        // Break caught: the recovery WM_TIMER fires inside a modal loop and swaps documents in and
        // out of the view under the operation the modal dialog is about to complete.
        let _scintilla = load_native_scintilla();
        let window = ProductionWindow::new(make_app());
        let editor = install_test_editor(&window);
        let root = RecoveryScratch::new("modal-snapshot");
        app_mut(window.hwnd).recovery_root = Some(root.path().to_path_buf());
        editor.set_text("typed").unwrap();
        let snapshot = snapshot_path(
            root.path(),
            app_mut(window.hwnd).tabs.active().unwrap().recovery_id,
        );

        answer_next_close_prompt(|hwnd| {
            super::snapshot_next_document(hwnd);
            CloseDecision::Cancel
        });
        execute_command(window.hwnd, CommandId::CloseTab);

        assert!(
            !snapshot.exists(),
            "a snapshot tick ran inside the modal loop"
        );

        super::snapshot_next_document(window.hwnd);

        assert!(snapshot.exists(), "snapshots must resume after the modal");
    }

    #[test]
    fn save_as_writes_the_document_chosen_before_the_dialog_opened() {
        // Break caught: the Save As dialog's modal loop activates another tab (a recovered one, a
        // forwarded open), and complete_save then renames and overwrites whatever is active now.
        let _scintilla = load_native_scintilla();
        let window = ProductionWindow::new(make_app());
        let editor = install_test_editor(&window);
        let root = RecoveryScratch::new("modal-save-as");
        let target = root.path().join("chosen.txt");
        editor.set_text("alpha").unwrap();
        let chosen = app_mut(window.hwnd).tabs.active().unwrap().id;
        let destination = target.clone();
        answer_next_save_dialog(move |hwnd| {
            super::create_new_document(hwnd).unwrap();
            Some(destination)
        });

        assert!(super::save_active_document_as(window.hwnd));

        assert_eq!(std::fs::read(&target).unwrap(), b"alpha");
        let app = app_mut(window.hwnd);
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.tabs.active().unwrap().id, chosen);
        assert_eq!(
            app.tabs.document(chosen).unwrap().path.as_deref(),
            Some(target.as_path())
        );
    }

    #[test]
    fn failed_language_activation_leaves_document_language_unchanged_and_records_a_warning() {
        // Break caught: a failed Lexilla load/lexer-creation must not record the requested
        // language on Document metadata when the editor itself was left exactly as it was
        // (LanguageManager::apply never installs a lexer before a real pointer is in hand), and
        // must surface something the caller can show in a notification instead of failing silently.
        let _scintilla = load_native_scintilla();
        let window = ProductionWindow::new(make_app());
        let identity = unsafe { super::window_identity(window.hwnd).unwrap() };
        unsafe {
            super::initialize_editor_with(window.hwnd, &identity, crate::editor::Editor::create)
        }
        .unwrap();

        // Seed a LanguageManager pointed at a Lexilla.dll path that cannot possibly load, so the
        // activation below fails deterministically without depending on the real native DLL.
        let missing = std::env::temp_dir().join(format!(
            "fastpad-main-window-missing-lexilla-test-{}.dll",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&missing);
        unsafe {
            super::app_ptr(window.hwnd)
                .unwrap()
                .as_mut()
                .language_manager = Some(LanguageManager::with_dll_path_for_test(missing));
        }
        let language_before = unsafe { super::app_ptr(window.hwnd).unwrap().as_ref() }
            .tabs
            .active()
            .unwrap()
            .language;
        assert_eq!(language_before, Language::PlainText);

        execute_command(window.hwnd, CommandId::LanguageJson);

        assert_eq!(
            notices(window.hwnd),
            vec![
                "FastPad could not enable syntax highlighting for this file. It will remain in \
                 plain text."
                    .to_owned()
            ]
        );
        let language_after = unsafe { super::app_ptr(window.hwnd).unwrap().as_ref() }
            .tabs
            .active()
            .unwrap()
            .language;
        assert_eq!(language_after, Language::PlainText);
    }

    #[test]
    fn corrupt_settings_are_reported_non_modally_and_reserve_status_height() {
        // Break caught: nothing else asserts that invalid fastpad.ini lines actually reach the
        // user. If load_settings stopped queuing warnings, or the painted status line stopped
        // showing them, or layout stopped reserving room for it, every other test would still pass.
        let _scintilla = load_native_scintilla();
        let window = ProductionWindow::new(make_app());
        let identity = unsafe { super::window_identity(window.hwnd).unwrap() };
        unsafe {
            super::initialize_editor_with(window.hwnd, &identity, crate::editor::Editor::create)
        }
        .unwrap();
        let editor_hwnd = unsafe { super::app_ptr(window.hwnd).unwrap().as_ref() }
            .editor
            .as_ref()
            .unwrap()
            .hwnd();

        // No status line exists before WM_FASTPAD_BUILD_CHROME, regardless of pending warnings.
        assert_eq!(super::current_status_text(window.hwnd), None);

        let warnings = vec![
            crate::config::SettingWarning {
                line: 3,
                message: "invalid value for tab_width: \"nope\"".to_owned(),
            },
            crate::config::SettingWarning {
                line: 5,
                message: "unknown setting key: bogus".to_owned(),
            },
        ];
        super::apply_loaded_settings(window.hwnd, crate::config::default_settings(), warnings);

        assert_eq!(
            unsafe { super::app_ptr(window.hwnd).unwrap().as_ref() }
                .notifications
                .len(),
            2
        );
        assert_eq!(
            super::current_status_text(window.hwnd),
            None,
            "chrome has not been built yet, so there is still nowhere to paint the warning"
        );

        super::build_chrome(window.hwnd);

        let status = super::current_status_text(window.hwnd);
        assert!(
            status
                .as_deref()
                .is_some_and(|text| text.contains("fastpad.ini line 3")),
            "expected the first warning's line reference in {status:?}"
        );
        let mut client = RECT::default();
        let mut shown = RECT::default();
        unsafe {
            GetClientRect(window.hwnd, &mut client);
            GetClientRect(editor_hwnd, &mut shown);
        }
        assert!(
            shown.bottom - shown.top < client.bottom - client.top,
            "the editor should be shorter than the client area while the status line is showing"
        );

        super::dismiss_notifications(window.hwnd);

        assert_eq!(super::current_status_text(window.hwnd), None);
        let mut dismissed = RECT::default();
        unsafe {
            GetClientRect(editor_hwnd, &mut dismissed);
        }
        let dpi = unsafe { GetDpiForWindow(window.hwnd) };
        assert_eq!(
            (dismissed.bottom - dismissed.top) - (shown.bottom - shown.top),
            crate::window::status::status_height(dpi),
            "dismissing notifications should return exactly the reserved status height"
        );
    }

    fn line_number_margin_width(editor: &crate::editor::Editor) -> isize {
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW(
                editor.hwnd(),
                crate::editor::scintilla_constants::SCI_GETMARGINWIDTHN,
                0,
                0,
            )
        }
    }

    #[test]
    fn line_numbers_follow_edits_and_the_line_numbers_setting() {
        // Break caught: typing or pasting past line 99 without re-sizing the gutter clips the
        // numbers, and line_numbers=false in fastpad.ini leaving the gutter visible.
        let _scintilla = load_native_scintilla();
        let window = ProductionWindow::new(make_app());
        let editor = install_test_editor(&window);
        let two_digits = line_number_margin_width(&editor);
        assert!(two_digits > 0, "line numbers are shown by default");

        editor.replace_target(0..0, &"\n".repeat(150)).unwrap();
        let three_digits = line_number_margin_width(&editor);
        assert!(
            three_digits > two_digits,
            "151 lines need a wider gutter than {two_digits}px, got {three_digits}px"
        );

        editor.undo().unwrap();
        assert_eq!(line_number_margin_width(&editor), two_digits);

        let mut settings = crate::config::default_settings();
        settings.line_numbers = false;
        super::apply_loaded_settings(window.hwnd, settings, Vec::new());
        assert_eq!(line_number_margin_width(&editor), 0);
    }

    #[test]
    fn format_json_command_reformats_with_two_spaces_in_one_undo_step() {
        // Break caught: Format JSON not actually rewriting the buffer, or splitting the rewrite
        // into more than one undo action (which would force repeated Ctrl+Z to fully undo it).
        let _scintilla = load_native_scintilla();
        let window = ProductionWindow::new(make_app());
        let editor = install_test_editor(&window);
        editor.populate_clean("{\"a\":[1,2]}").unwrap();

        execute_command(window.hwnd, CommandId::FormatJson);

        assert_eq!(
            editor.text().unwrap(),
            "{\n  \"a\": [\n    1,\n    2\n  ]\n}"
        );
        assert!(notices(window.hwnd).is_empty());
        assert!(editor.can_undo().unwrap());
        editor.undo().unwrap();
        assert_eq!(editor.text().unwrap(), "{\"a\":[1,2]}");
        assert!(!editor.can_undo().unwrap());
    }

    #[test]
    fn format_json_command_on_invalid_json_leaves_bytes_unchanged_and_reports_an_issue() {
        // Break caught: Format JSON starting an undo action or mutating the buffer before
        // discovering the source does not parse, and/or swallowing the failure instead of
        // surfacing it.
        let _scintilla = load_native_scintilla();
        let window = ProductionWindow::new(make_app());
        let editor = install_test_editor(&window);
        editor.populate_clean("{ bad").unwrap();

        execute_command(window.hwnd, CommandId::FormatJson);

        assert_eq!(editor.text().unwrap(), "{ bad");
        assert!(!editor.can_undo().unwrap());
        let issues = notices(window.hwnd);
        assert_eq!(issues.len(), 1);
        assert!(issues[0].contains("line 1"), "{}", issues[0]);
    }

    #[test]
    fn format_json_command_clamps_the_restored_selection_to_the_new_shorter_length() {
        // Break caught: restoring the pre-format selection verbatim after formatting shrank the
        // document, leaving an out-of-range SCI_SETSEL instead of a clamped one.
        let _scintilla = load_native_scintilla();
        let window = ProductionWindow::new(make_app());
        let editor = install_test_editor(&window);
        let padding = " ".repeat(50);
        let source = format!("{{{padding}\"a\":1}}");
        editor.populate_clean(&source).unwrap();
        editor.set_selection(55..58).unwrap();

        execute_command(window.hwnd, CommandId::FormatJson);

        let formatted_len = editor.text().unwrap().len();
        assert!(formatted_len < 55, "expected formatting to shrink the text");
        assert_eq!(editor.selection().unwrap(), formatted_len..formatted_len);
    }

    #[test]
    fn format_json_command_snaps_the_restored_selection_to_a_utf8_char_boundary() {
        // Break caught: reusing a pre-format byte offset verbatim (once only clamped to the new
        // length) against the post-format text can land mid-character, since it has no guaranteed
        // relationship to character boundaries in the reformatted bytes. Here the caret sits at an
        // ordinary, valid boundary in the *compact* source (right after "héllo"'s closing quote);
        // at that exact raw byte offset, the *pretty-printed* text — which keeps "é"'s literal
        // two-byte UTF-8 encoding but reflows the surrounding whitespace — instead lands squarely
        // between "é"'s two bytes.
        let _scintilla = load_native_scintilla();
        let window = ProductionWindow::new(make_app());
        let editor = install_test_editor(&window);
        let source = "{\"a\":\"h\u{e9}llo\",\"b\":1}";
        let formatted = crate::languages::format_json(source).unwrap();

        let boundary_in_source = source.find("\",\"b\"").unwrap(); // right before the closing '"'
        assert!(source.is_char_boundary(boundary_in_source));

        let e_char_start = formatted.find('\u{e9}').unwrap();
        assert_eq!(
            boundary_in_source,
            e_char_start + 1,
            "test setup: expected the reused raw byte offset to land inside é's encoding"
        );
        assert!(!formatted.is_char_boundary(boundary_in_source));

        editor.populate_clean(source).unwrap();
        editor
            .set_selection(boundary_in_source..boundary_in_source)
            .unwrap();

        execute_command(window.hwnd, CommandId::FormatJson);

        assert_eq!(editor.text().unwrap(), formatted);
        let restored = editor.selection().unwrap();
        assert!(
            formatted.is_char_boundary(restored.start) && formatted.is_char_boundary(restored.end),
            "restored selection {restored:?} is not on a UTF-8 character boundary"
        );
        // Snapped backward to the boundary immediately before "é", not forward past it.
        assert_eq!(restored, e_char_start..e_char_start);
    }

    #[test]
    fn validate_json_command_reports_success_and_never_mutates_the_document() {
        let _scintilla = load_native_scintilla();
        let window = ProductionWindow::new(make_app());
        let editor = install_test_editor(&window);
        editor.populate_clean("{\"a\":1}").unwrap();

        execute_command(window.hwnd, CommandId::ValidateJson);

        let reported = notices(window.hwnd);
        assert_eq!(reported.len(), 1);
        assert!(reported[0].contains("valid JSON"), "{reported:?}");
        assert_eq!(editor.text().unwrap(), "{\"a\":1}");
        assert!(!editor.can_undo().unwrap());
    }

    #[test]
    fn validate_json_command_reports_the_line_and_column_for_invalid_json() {
        let _scintilla = load_native_scintilla();
        let window = ProductionWindow::new(make_app());
        let editor = install_test_editor(&window);
        editor.populate_clean("{\n  bad\n}").unwrap();

        execute_command(window.hwnd, CommandId::ValidateJson);

        let issues = notices(window.hwnd);
        assert_eq!(issues.len(), 1);
        assert!(
            issues[0].contains("line 2") && issues[0].contains("column 3"),
            "{}",
            issues[0]
        );
        assert_eq!(editor.text().unwrap(), "{\n  bad\n}");
    }

    #[test]
    fn recovery_discovery_opens_foreign_snapshots_as_dirty_recovered_tabs_with_one_notice() {
        // Break caught: recovered text opened clean, untitled-looking, without a notice, or this
        // process's own live snapshots reopened as duplicates.
        let _scintilla = load_native_scintilla();
        let window = ProductionWindow::new(make_app());
        let editor = install_test_editor(&window);
        let root = RecoveryScratch::new("discover");
        let source = write_snapshot(
            root.path(),
            &Snapshot::new(
                RecoveryId::from_u128(0x77),
                Some(PathBuf::from(r"C:\docs\notes.md")),
                Encoding::Utf16Le,
                "recovered body",
            ),
        )
        .unwrap();
        let own_id = app_mut(window.hwnd).allocate_recovery_id();
        let own = write_snapshot(
            root.path(),
            &Snapshot::new(own_id, None, Encoding::Utf8, "live"),
        )
        .unwrap();
        std::fs::write(root.path().join("torn.fps"), b"FPS1").unwrap();
        app_mut(window.hwnd).recovery_root = Some(root.path().to_path_buf());

        super::recover_snapshots(window.hwnd);

        let (tabs, title, dirty, path, encoding, origin, notices) = {
            let app = app_mut(window.hwnd);
            let active = app.tabs.active().unwrap();
            (
                app.tabs.len(),
                active.title(),
                active.dirty,
                active.path.clone(),
                active.encoding,
                active.recovery_origin.clone(),
                app.notifications.len(),
            )
        };
        assert_eq!(tabs, 2);
        assert_eq!(title, "Recovered: notes.md *");
        assert!(dirty);
        assert_eq!(path, None);
        assert_eq!(encoding, Encoding::Utf16Le);
        assert_eq!(origin.unwrap().snapshot_path, source);
        assert_eq!(notices, 1);
        assert_eq!(editor.text().unwrap(), "recovered body");
        assert_ne!(
            unsafe { SendMessageW(editor.hwnd(), SCI_GETMODIFY, 0, 0) },
            0,
            "Scintilla itself must treat the recovered text as unsaved"
        );
        assert!(source.exists() && own.exists());
        assert!(root.path().join("torn.fps.invalid").exists());
    }

    #[test]
    fn snapshots_write_one_changed_dirty_document_per_tick_without_disturbing_the_view() {
        // Break caught: several documents written per tick, unchanged generations rewritten, or an
        // inactive tab's snapshot swapping the visible document or selection.
        let _scintilla = load_native_scintilla();
        let window = ProductionWindow::new(make_app());
        let editor = install_test_editor(&window);
        let root = RecoveryScratch::new("tick");
        app_mut(window.hwnd).recovery_root = Some(root.path().to_path_buf());
        editor.set_text("alpha").unwrap();
        super::create_new_document(window.hwnd).unwrap();
        editor.set_text("beta\nline").unwrap();
        editor.set_selection(2..3).unwrap();
        let ids = app_mut(window.hwnd)
            .tabs
            .documents()
            .map(|document| document.recovery_id)
            .collect::<Vec<_>>();
        let first = snapshot_path(root.path(), ids[0]);
        let second = snapshot_path(root.path(), ids[1]);

        super::snapshot_next_document(window.hwnd);

        assert_eq!(read_snapshot_text(&first), "alpha");
        assert!(!second.exists());
        assert_eq!(editor.text().unwrap(), "beta\nline");
        assert_eq!(editor.selection().unwrap(), 2..3);
        assert!(app_mut(window.hwnd).last_snapshot_duration.is_some());

        super::snapshot_next_document(window.hwnd);
        assert_eq!(read_snapshot_text(&second), "beta\nline");

        std::fs::remove_file(&first).unwrap();
        std::fs::remove_file(&second).unwrap();
        super::snapshot_next_document(window.hwnd);
        assert!(!first.exists() && !second.exists());
    }

    #[test]
    fn saving_a_recovered_tab_never_touches_the_original_and_removes_its_snapshots() {
        // Break caught: Save silently writing the original path, or a saved recovered document
        // leaving snapshots that resurrect it after the next crash.
        let _scintilla = load_native_scintilla();
        let window = ProductionWindow::new(make_app());
        let editor = install_test_editor(&window);
        let root = RecoveryScratch::new("save");
        let original = root.path().join("original.txt");
        std::fs::write(&original, b"original").unwrap();
        let source = write_snapshot(
            root.path(),
            &Snapshot::new(
                RecoveryId::from_u128(0x99),
                Some(original.clone()),
                Encoding::Utf8,
                "recovered",
            ),
        )
        .unwrap();
        app_mut(window.hwnd).recovery_root = Some(root.path().to_path_buf());
        super::recover_snapshots(window.hwnd);
        editor.set_text("recovered and edited").unwrap();

        super::snapshot_next_document(window.hwnd);
        let own = snapshot_path(
            root.path(),
            app_mut(window.hwnd).tabs.active().unwrap().recovery_id,
        );
        assert_eq!(read_snapshot_text(&own), "recovered and edited");
        assert!(
            !source.exists(),
            "the stale source would reopen as a duplicate"
        );

        let target = root.path().join("saved.txt");
        super::save_path_as(window.hwnd, &target);

        assert_eq!(std::fs::read(&target).unwrap(), b"recovered and edited");
        assert_eq!(std::fs::read(&original).unwrap(), b"original");
        assert!(!own.exists());
        let app = app_mut(window.hwnd);
        assert_eq!(app.tabs.active().unwrap().title(), "saved.txt");
        assert_eq!(app.tabs.active().unwrap().recovery_origin, None);
    }

    #[test]
    fn clean_window_close_removes_this_sessions_snapshots() {
        // Break caught: snapshots outliving a clean exit and restoring tabs on the next launch.
        let _scintilla = load_native_scintilla();
        let window = ProductionWindow::new(make_app());
        let editor = install_test_editor(&window);
        let root = RecoveryScratch::new("close");
        app_mut(window.hwnd).recovery_root = Some(root.path().to_path_buf());
        editor.set_text("typed").unwrap();
        super::snapshot_next_document(window.hwnd);
        let own = snapshot_path(
            root.path(),
            app_mut(window.hwnd).tabs.active().unwrap().recovery_id,
        );
        assert!(own.exists());
        editor.set_save_point();

        unsafe {
            SendMessageW(window.hwnd, WM_CLOSE, 0, 0);
        }

        assert_eq!(unsafe { IsWindow(window.hwnd) }, 0);
        assert!(!own.exists());
    }

    #[test]
    fn undoing_a_recovered_tab_keeps_it_dirty_and_its_source_through_clean_close_cleanup() {
        // Break caught: undo reaching Scintilla's empty save point marks the recovered tab clean,
        // so closing skips the prompt and deletes the only copy of its text.
        let _scintilla = load_native_scintilla();
        let window = ProductionWindow::new(make_app());
        let editor = install_test_editor(&window);
        let root = RecoveryScratch::new("undo");
        let source = write_snapshot(
            root.path(),
            &Snapshot::new(
                RecoveryId::from_u128(0x55),
                None,
                Encoding::Utf8,
                "only copy",
            ),
        )
        .unwrap();
        app_mut(window.hwnd).recovery_root = Some(root.path().to_path_buf());
        super::recover_snapshots(window.hwnd);

        while editor.can_undo().unwrap() {
            editor.undo().unwrap();
        }

        assert_eq!(editor.text().unwrap(), "");
        assert_eq!(
            unsafe { SendMessageW(editor.hwnd(), SCI_GETMODIFY, 0, 0) },
            0,
            "test setup: Scintilla reached its save point"
        );
        let app = app_mut(window.hwnd);
        let active = app.tabs.active().unwrap().id;
        assert!(app.tabs.active().unwrap().dirty);
        assert_eq!(
            app.tabs.next_dirty_review(&[]).map(|review| review.id),
            Some(active),
            "window close must still prompt for the recovered tab"
        );
        super::remove_session_snapshots(window.hwnd, &[]);
        assert!(source.exists());
    }

    #[test]
    fn discovery_skips_snapshots_whose_owner_process_is_still_running() {
        // Break caught: a second instance opening, quarantining, or later deleting a live
        // instance's snapshots.
        let _scintilla = load_native_scintilla();
        let window = ProductionWindow::new(make_app());
        let _editor = install_test_editor(&window);
        let root = RecoveryScratch::new("live-owner");
        let process_start = 0x5EED_0000_0000_0000 | u64::from(std::process::id());
        let live = RecoveryId::compose(process_start, 4_000_000_001, 1);
        let torn = snapshot_path(
            root.path(),
            RecoveryId::compose(process_start, 4_000_000_001, 2),
        );
        let owner = crate::recovery::create_owner_mutex(live).unwrap();
        let valid = write_snapshot(
            root.path(),
            &Snapshot::new(live, None, Encoding::Utf8, "live elsewhere"),
        )
        .unwrap();
        std::fs::write(&torn, b"FPS1").unwrap();
        app_mut(window.hwnd).recovery_root = Some(root.path().to_path_buf());

        super::recover_snapshots(window.hwnd);

        assert_eq!(app_mut(window.hwnd).tabs.len(), 1);
        assert!(valid.exists() && torn.exists());

        drop(owner);
        super::recover_snapshots(window.hwnd);

        assert_eq!(app_mut(window.hwnd).tabs.len(), 2);
        assert!(valid.exists());
        assert!(
            !torn.exists(),
            "a dead owner's torn snapshot is quarantined"
        );
    }

    fn app_mut<'a>(hwnd: HWND) -> &'a mut App {
        unsafe { super::app_ptr(hwnd).unwrap().as_mut() }
    }

    /// The non-modal notification messages currently queued on the window (spec 239).
    fn notices(hwnd: HWND) -> Vec<String> {
        app_mut(hwnd)
            .notifications
            .pending()
            .iter()
            .map(|notice| notice.message.clone())
            .collect()
    }

    fn read_snapshot_text(path: &std::path::Path) -> String {
        Snapshot::decode(&std::fs::read(path).unwrap())
            .unwrap()
            .text
    }

    struct RecoveryScratch(PathBuf);

    impl RecoveryScratch {
        fn new(label: &str) -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "fastpad-window-recovery-{label}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for RecoveryScratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Installs a real Scintilla editor onto `window` (mirroring
    /// `failed_language_activation_leaves_document_language_unchanged_and_records_a_warning`'s own
    /// setup) and returns it for direct `text`/`set_text`/`selection` calls in JSON command tests.
    fn install_test_editor(window: &ProductionWindow) -> crate::editor::Editor {
        let identity = unsafe { super::window_identity(window.hwnd).unwrap() };
        unsafe {
            super::initialize_editor_with(window.hwnd, &identity, crate::editor::Editor::create)
        }
        .unwrap();
        unsafe { super::app_ptr(window.hwnd).unwrap().as_ref() }
            .editor
            .clone()
            .unwrap()
    }

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
    fn production_nc_create_transfers_app_and_nc_destroy_clears_the_window() {
        // Break caught: bypassing the real WM_NCCREATE/WM_NCDESTROY ownership path can leave the
        // production window without App state or leave the HWND alive after teardown.
        let window = ProductionWindow::new(make_app());
        assert_ne!(unsafe { GetWindowLongPtrW(window.hwnd, GWLP_USERDATA) }, 0);
        unsafe {
            DestroyWindow(window.hwnd);
        }
        assert_eq!(unsafe { IsWindow(window.hwnd) }, 0);
    }

    #[test]
    fn initial_editor_installation_updates_a_retained_empty_tab_view() {
        // Break caught: accessibility requested before editor creation retains an obsolete view.
        let _scintilla = load_native_scintilla();
        let window = ProductionWindow::new(make_app());
        let view = unsafe { super::app_ptr(window.hwnd).unwrap().as_ref() }
            .tabs
            .view();
        assert!(view.snapshot().tabs.is_empty());
        let identity = unsafe { super::window_identity(window.hwnd).unwrap() };
        unsafe {
            super::initialize_editor_with(window.hwnd, &identity, crate::editor::Editor::create)
        }
        .unwrap();
        assert_eq!(view.snapshot().tabs.len(), 1);
    }

    #[test]
    fn native_wm_close_releases_all_owned_documents_before_editor_destruction() {
        // Break caught: clearing tabs after DestroyWindow skips real releases at the dead endpoint.
        if std::env::var_os("FASTPAD_REQUIRE_APPVERIF").is_some() {
            let verifier = crate::platform::wide_null("verifier.dll");
            assert!(
                !unsafe { GetModuleHandleW(verifier.as_ptr()) }.is_null(),
                "Application Verifier must actually be loaded for a claimed verifier run"
            );
        }
        let _scintilla = load_native_scintilla();
        let window = ProductionWindow::new(make_app());
        let identity = unsafe { super::window_identity(window.hwnd).unwrap() };
        let editor = unsafe {
            super::initialize_editor_with(window.hwnd, &identity, crate::editor::Editor::create)
        }
        .unwrap();
        super::create_new_document(window.hwnd).unwrap();
        let (_, releases) = crate::editor::scintilla::release_observation::during(|| unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW(
                window.hwnd,
                windows_sys::Win32::UI::WindowsAndMessaging::WM_CLOSE,
                0,
                0,
            )
        });
        assert_eq!(releases.len(), 2);
        assert_ne!(releases[0].document, releases[1].document);
        assert!(
            releases
                .iter()
                .all(|release| release.hwnd == editor && release.window_was_live)
        );
        assert_eq!(unsafe { IsWindow(editor) }, 0);
        assert_eq!(unsafe { IsWindow(window.hwnd) }, 0);
    }

    #[test]
    fn original_window_identity_stays_invalid_after_replacement_creation() {
        // Break caught: an IsWindow-only liveness check can accept a recycled HWND and read the
        // replacement window's GWLP_USERDATA as the original App.
        let original_app = make_app();
        let original_identity = original_app.window_identity();
        let original = ProductionWindow::new(original_app);
        assert!(original_identity.is_live_for(original.hwnd));

        unsafe {
            DestroyWindow(original.hwnd);
        }
        assert!(original_identity.is_invalidated());
        drop(original);

        let replacement_app = make_app();
        let replacement_identity = replacement_app.window_identity();
        let replacement = ProductionWindow::new(replacement_app);
        assert!(replacement_identity.is_live_for(replacement.hwnd));
        assert!(original_identity.is_invalidated());
        assert!(!original_identity.is_live_for(replacement.hwnd));
    }

    #[test]
    fn reentrant_paint_completion_does_not_mutate_replacement_app() {
        // Break caught: removing the post-DefWindowProc identity gate lets an old WM_PAINT
        // completion mutate the App found in a recycled HWND's replacement GWLP_USERDATA slot.
        const PAINT_RESULT: LRESULT = 73;
        let mut original = Some(ProductionWindow::new(make_app()));
        let original_hwnd = original.as_ref().unwrap().hwnd;
        let replacement = RefCell::new(None::<ProductionWindow>);

        let default_window_proc = |hwnd, _, _, _| {
            assert_ne!(unsafe { DestroyWindow(hwnd) }, 0);
            drop(original.take());
            replacement.replace(Some(ProductionWindow::new(make_app())));
            PAINT_RESULT
        };
        let complete_first_paint = |_| {
            let replacement = replacement.borrow();
            let replacement = replacement.as_ref().unwrap();
            unsafe {
                mark_first_paint_complete(replacement.hwnd);
            }
        };
        let result = unsafe {
            handle_paint_with(
                original_hwnd,
                WM_PAINT,
                0,
                0,
                default_window_proc,
                complete_first_paint,
            )
        };

        assert_eq!(result, PAINT_RESULT);
        let replacement = replacement.borrow();
        let replacement = replacement.as_ref().unwrap();
        assert!(!unsafe { take_deferred_start_pending(replacement.hwnd) });
    }

    fn unnamed_mutex() -> crate::platform::OwnedHandle {
        let raw = unsafe {
            windows_sys::Win32::System::Threading::CreateMutexW(
                std::ptr::null(),
                0,
                std::ptr::null(),
            )
        };
        unsafe { crate::platform::OwnedHandle::from_raw_owned(raw) }.unwrap()
    }

    #[test]
    fn ipc_bind_failure_releases_the_instance_mutex_and_notifies_exactly_once() {
        // Break caught: keeping the mutex after a failed bind makes every later launch wait on a
        // pipe that will never exist; retrying or re-notifying spams the status line.
        let window = ProductionWindow::new(make_app());
        unsafe { super::app_ptr(window.hwnd).unwrap().as_mut() }.instance_mutex =
            Some(unnamed_mutex());

        super::start_ipc_server_with(window.hwnd, || {
            Err(crate::FastPadError::Ipc("simulated bind failure"))
        });
        super::start_ipc_server_with(window.hwnd, || unreachable!("no mutex means no server"));

        let app = unsafe { super::app_ptr(window.hwnd).unwrap().as_ref() };
        assert!(app.ipc.is_none());
        assert!(app.instance_mutex.is_none());
        assert_eq!(app.notifications.len(), 1);
    }

    #[test]
    fn process_without_instance_mutex_never_binds_a_server() {
        // Break caught: a --new-window or fallback process squats the primary's pipe name.
        let window = ProductionWindow::new(make_app());
        super::start_ipc_server_with(window.hwnd, || unreachable!("no mutex means no server"));
        let app = unsafe { super::app_ptr(window.hwnd).unwrap().as_ref() };
        assert!(app.ipc.is_none());
        assert_eq!(app.notifications.len(), 0);
    }

    fn deliver_frame(window: &ProductionWindow, names: &crate::ipc::InstanceNames, frame: Vec<u8>) {
        use std::time::{Duration, Instant};
        use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
        use windows_sys::Win32::System::Threading::WaitForSingleObject;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
        };
        let pipe = names.clone();
        let client = std::thread::spawn(move || {
            crate::ipc::client::send_frame(&pipe, &frame, Duration::from_secs(2))
        });
        let identity = unsafe { super::window_identity(window.hwnd).unwrap() };
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut quiet_since = None;
        while Instant::now() < deadline {
            let event = super::ipc_wait_handle(window.hwnd, &identity).unwrap();
            if unsafe { WaitForSingleObject(event, 20) } == WAIT_OBJECT_0 {
                super::service_ipc(window.hwnd, &identity);
                quiet_since = None;
            } else if client.is_finished() {
                let since = *quiet_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= Duration::from_millis(150) {
                    break;
                }
            }
            let mut message = MSG::default();
            while unsafe { PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0
            {
                unsafe {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
        }
        client.join().unwrap().unwrap();
    }

    #[test]
    fn ipc_requests_reach_the_window_as_tabs_and_malformed_frames_change_nothing() {
        // Break caught: decoded requests never leave the pipe, duplicate opens add tabs, Activate
        // mutates tabs, or a malformed frame reaches application state.
        use crate::ipc::{IpcRequest, encode_frame};
        let _scintilla = load_native_scintilla();
        let window = ProductionWindow::new(make_app());
        let editor = install_test_editor(&window);
        unsafe {
            SendMessageW(
                editor.hwnd(),
                windows_sys::Win32::UI::WindowsAndMessaging::WM_CHAR,
                b'x' as usize,
                0,
            );
        }
        unsafe { super::app_ptr(window.hwnd).unwrap().as_mut() }.instance_mutex =
            Some(unnamed_mutex());
        let names = crate::ipc::server::tests::unique_names();
        super::start_ipc_server_with(window.hwnd, || {
            crate::ipc::IpcServer::bind(&names, &crate::ipc::CurrentUserAcl::current()?)
        });
        let scratch = RecoveryScratch::new("ipc-open");
        let file = scratch.path().join("forwarded.txt");
        std::fs::write(&file, b"forwarded text").unwrap();
        let tabs = || {
            unsafe { super::app_ptr(window.hwnd).unwrap().as_ref() }
                .tabs
                .len()
        };

        let open = encode_frame(&IpcRequest::Open(file.clone())).unwrap();
        deliver_frame(&window, &names, open.clone());
        assert_eq!(tabs(), 2);
        assert_eq!(editor.text().unwrap(), "forwarded text");
        deliver_frame(&window, &names, open);
        assert_eq!(tabs(), 2);
        deliver_frame(
            &window,
            &names,
            encode_frame(&IpcRequest::Activate).unwrap(),
        );
        assert_eq!(tabs(), 2);
        deliver_frame(&window, &names, b"FPI1\x09\0\0\0\0".to_vec());
        assert_eq!(tabs(), 2);
        deliver_frame(&window, &names, encode_frame(&IpcRequest::New).unwrap());
        assert_eq!(tabs(), 3);
        assert!(
            unsafe { super::app_ptr(window.hwnd).unwrap().as_ref() }
                .ipc_requests
                .is_empty()
        );
    }

    fn make_app() -> Box<App> {
        Box::new(App::new(
            LaunchOptions::default(),
            StartupMetrics::with_frequency(1, 0),
        ))
    }

    fn load_native_scintilla() -> crate::platform::OwnedModule {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("native/out/x64/Scintilla.dll");
        let path = crate::platform::wide_null(path.to_str().unwrap());
        let module = unsafe {
            windows_sys::Win32::System::LibraryLoader::LoadLibraryExW(
                path.as_ptr(),
                std::ptr::null_mut(),
                windows_sys::Win32::System::LibraryLoader::LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR
                    | windows_sys::Win32::System::LibraryLoader::LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
        };
        unsafe { crate::platform::OwnedModule::from_raw_owned(module) }.unwrap()
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

    struct ProductionWindow {
        hwnd: HWND,
        _class: MainWindowClass,
    }

    impl ProductionWindow {
        fn new(app: Box<App>) -> Self {
            let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
            let class = MainWindowClass::register(instance).unwrap();
            let mut context = WindowCreateContext::new(app);
            let hwnd = class.create(&mut context).unwrap();

            Self {
                hwnd,
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
