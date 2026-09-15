use crate::Result;
use crate::app::{App, WindowIdentity};
use crate::document::{CloseDecision, Document, DocumentId, RecoveryId};
use crate::editor::Editor;
use crate::perf::Milestone;
use crate::platform::{last_error, wide_null};
use crate::window::accessibility::{self, AccessibleSelectRequest, WM_FASTPAD_ACCESSIBLE_SELECT};
use crate::window::commands::CommandId;
use crate::window::find_bar;
use crate::window::menus::{self, MenuBar};
use crate::window::messages::{
    DeferredAction, classify_deferred_message, completed_milestone, deferred_start_message,
};
use crate::window::tabs::CloseReviewKey;
#[cfg(test)]
use std::cell::Cell;
use std::ffi::c_void;
use std::ptr::NonNull;
use windows_sys::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::UI::Controls::NMHDR;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, SetFocus, VK_CONTROL, VK_F10, VK_MENU, VK_SHIFT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetClientRect,
    GetWindowLongPtrW, IDCANCEL, IDNO, IDYES, MB_ICONWARNING, MB_YESNOCANCEL, MessageBoxW,
    MoveWindow, OBJID_CLIENT, PostMessageW, PostQuitMessage, QS_INPUT, RegisterClassW, SC_KEYMENU,
    SWP_NOACTIVATE, SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos, UnregisterClassW, WM_CLOSE,
    WM_COMMAND, WM_DESTROY, WM_DPICHANGED, WM_EXITMENULOOP, WM_GETMINMAXINFO, WM_GETOBJECT,
    WM_KEYDOWN, WM_LBUTTONUP, WM_NCCALCSIZE, WM_NCCREATE, WM_NCDESTROY, WM_NCHITTEST, WM_NOTIFY,
    WM_PAINT, WM_SETFOCUS, WM_SETTINGCHANGE, WM_SIZE, WM_SYSCOMMAND, WM_SYSKEYDOWN, WM_SYSKEYUP,
    WM_THEMECHANGED, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};
#[cfg(not(test))]
use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_ICONINFORMATION, MB_OK};
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
            0
        }
        WM_SETFOCUS => {
            if let Some(editor_hwnd) = unsafe { editor_hwnd(hwnd) } {
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
            if review_dirty_documents(hwnd) {
                clear_documents_for_shutdown(hwnd);
                unsafe {
                    DestroyWindow(hwnd);
                }
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
            let paint_title_strip = |hwnd, _, _, _| {
                let (titles, active) = tab_snapshot(hwnd);
                let title_refs = titles.iter().map(String::as_str).collect::<Vec<_>>();
                unsafe { crate::window::titlebar::paint(hwnd, &title_refs, active) };
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
            crate::window::titlebar::nonclient_hit_test(hwnd, wparam, lparam, tab_count(hwnd))
        },
        WM_NCCALCSIZE => unsafe { crate::window::titlebar::reclaim_caption(hwnd, wparam, lparam) },
        WM_GETMINMAXINFO => unsafe {
            crate::window::titlebar::constrain_maximized_window(hwnd, lparam)
        },
        WM_LBUTTONUP => {
            let point = crate::window::titlebar::Point::new(
                (lparam as u32 & 0xffff) as u16 as i16 as i32,
                ((lparam as u32 >> 16) & 0xffff) as u16 as i16 as i32,
            );
            let layout = crate::window::titlebar::layout_for_window(hwnd, tab_count(hwnd));
            match layout.hit_test(point) {
                crate::window::titlebar::HitTarget::Overflow => {
                    if let Some(command) = menus::show_overflow(hwnd, point.x, layout.height) {
                        execute_command(hwnd, command);
                    }
                }
                crate::window::titlebar::HitTarget::NewTab => execute_command(hwnd, CommandId::New),
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
            0
        }
        WM_SETTINGCHANGE | WM_THEMECHANGED => {
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
            if message == crate::window::WM_FASTPAD_OPEN_REQUEST && !input_pending() {
                return handle_open_request(hwnd);
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

    let document = Document::untitled(DocumentId(1), RecoveryId(1), editor.current_document()?);
    if !identity.is_live_for(hwnd) {
        return Err(crate::FastPadError::Invariant(
            "main window was destroyed while adopting the initial document",
        ));
    }

    unsafe {
        install_editor(hwnd, editor, document)?;
        record_milestone(hwnd, Milestone::EditorCreated)?;
    }
    install_open_input_hook(hwnd, editor_hwnd, identity.clone())?;
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
    let title_height = crate::window::titlebar::layout_for_window(hwnd, tab_count(hwnd)).height;
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
    unsafe {
        MoveWindow(
            editor_hwnd,
            0,
            content_top,
            width,
            (rect.bottom - rect.top - content_top).max(0),
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

fn tab_count(hwnd: HWND) -> usize {
    unsafe { app_ptr(hwnd) }
        .map(|app| unsafe { app.as_ref() }.tabs.len())
        .unwrap_or(1)
}

fn tab_snapshot(hwnd: HWND) -> (Vec<String>, usize) {
    unsafe { app_ptr(hwnd) }
        .map(|app| {
            let app = unsafe { app.as_ref() };
            (app.tabs.titles().collect(), app.tabs.active_index())
        })
        .unwrap_or_else(|| (vec!["Untitled".to_owned()], 0))
}

fn execute_command(hwnd: HWND, command: CommandId) {
    if file_population_active(hwnd) {
        return;
    }
    match command {
        CommandId::Open => {
            let identity = unsafe { window_identity(hwnd) };
            // Modal Show reenters the window procedure. Only an owned identity crosses it.
            let selection = crate::window::commands::choose_open_path(hwnd);
            if identity
                .as_ref()
                .is_some_and(|identity| identity.is_live_for(hwnd))
                && let Ok(Some(path)) = selection
            {
                let _ = App::open_path(hwnd, &path);
            }
        }
        CommandId::New => {
            let _ = create_new_document(hwnd);
        }
        CommandId::CloseTab => close_active_document(hwnd),
        CommandId::Save => save_active_document(hwnd),
        CommandId::SaveAs => save_active_document_as(hwnd),
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
    let path =
        unsafe { app_ptr(hwnd) }.and_then(|app| unsafe { app.as_ref() }.tabs.active().path.clone());
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
    let dark = system_uses_dark_mode();
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
        }
        Some(Err(_)) => {
            show_language_error(
                hwnd,
                "FastPad could not enable syntax highlighting for this file. It will remain in \
                 plain text.",
            );
        }
        None => {}
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
        Ok(()) => show_json_valid(hwnd),
        Err(issue) => show_json_issue(hwnd, &json_issue_message(&issue)),
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
        Err(issue) => show_json_issue(hwnd, &json_issue_message(&issue)),
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

// A real MessageBoxW is a blocking, modal native dialog; see `show_save_error`'s longer comment
// for why the test build records the message instead of showing it.
#[cfg(not(test))]
fn show_json_issue(hwnd: HWND, message: &str) {
    let text = wide_null(message);
    let caption = wide_null("FastPad");
    unsafe {
        MessageBoxW(hwnd, text.as_ptr(), caption.as_ptr(), MB_ICONERROR | MB_OK);
    }
}

#[cfg(test)]
fn show_json_issue(_hwnd: HWND, message: &str) {
    JSON_ISSUES.with(|issues| issues.borrow_mut().push(message.to_owned()));
}

#[cfg(test)]
thread_local! {
    static JSON_ISSUES: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Test-only accessor for the messages `show_json_issue` would otherwise have shown as a real
/// MessageBoxW. Clears the recorded list.
#[cfg(test)]
pub(crate) fn take_json_issues() -> Vec<String> {
    JSON_ISSUES.with(|issues| std::mem::take(&mut *issues.borrow_mut()))
}

#[cfg(not(test))]
fn show_json_valid(hwnd: HWND) {
    let text = wide_null("This document contains valid JSON.");
    let caption = wide_null("FastPad");
    unsafe {
        MessageBoxW(
            hwnd,
            text.as_ptr(),
            caption.as_ptr(),
            MB_ICONINFORMATION | MB_OK,
        );
    }
}

#[cfg(test)]
fn show_json_valid(_hwnd: HWND) {
    JSON_VALID_COUNT.with(|count| count.set(count.get() + 1));
}

#[cfg(test)]
thread_local! {
    static JSON_VALID_COUNT: Cell<usize> = const { Cell::new(0) };
}

/// Test-only accessor for how many times `show_json_valid` would otherwise have shown a real
/// MessageBoxW. Resets the count.
#[cfg(test)]
pub(crate) fn take_json_valid_count() -> usize {
    JSON_VALID_COUNT.with(|count| count.replace(0))
}

/// Reads `HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize\AppsUseLightTheme` on
/// every call (no caching, no `WM_SETTINGCHANGE` reactivity, no live re-styling of already-open
/// documents — a real theme subsystem is future scope beyond this task). Defaults to light when
/// the value cannot be read.
fn system_uses_dark_mode() -> bool {
    dark_mode_from_apps_use_light_theme(read_apps_use_light_theme())
}

/// Pure: `1` (or missing/unreadable) means the light theme is in use; `0` means dark.
fn dark_mode_from_apps_use_light_theme(apps_use_light_theme: Option<u32>) -> bool {
    apps_use_light_theme == Some(0)
}

fn read_apps_use_light_theme() -> Option<u32> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    let subkey = wide_null(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    let value_name = wide_null("AppsUseLightTheme");
    let mut data: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            value_name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&raw mut data).cast::<c_void>(),
            &mut size,
        )
    };
    (status == 0).then_some(data)
}

// A real MessageBoxW is a blocking, modal native dialog; see `show_save_error`'s longer comment
// for why the test build records the message instead of showing it.
#[cfg(not(test))]
fn show_language_error(hwnd: HWND, message: &str) {
    let text = wide_null(message);
    let caption = wide_null("FastPad");
    unsafe {
        MessageBoxW(
            hwnd,
            text.as_ptr(),
            caption.as_ptr(),
            MB_ICONWARNING | MB_OK,
        );
    }
}

#[cfg(test)]
fn show_language_error(_hwnd: HWND, message: &str) {
    LANGUAGE_ERRORS.with(|errors| errors.borrow_mut().push(message.to_owned()));
}

#[cfg(test)]
thread_local! {
    static LANGUAGE_ERRORS: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Test-only accessor for the messages `show_language_error` would otherwise have shown as a real
/// MessageBoxW. Clears the recorded list.
#[cfg(test)]
pub(crate) fn take_language_errors() -> Vec<String> {
    LANGUAGE_ERRORS.with(|errors| std::mem::take(&mut *errors.borrow_mut()))
}

fn handle_open_request(hwnd: HWND) -> LRESULT {
    let request = unsafe { app_ptr(hwnd) }.and_then(|mut app| {
        let app = unsafe { app.as_mut() };
        if app.launch_open_completed {
            return None;
        }
        if matches!(app.launch.request, crate::launch::LaunchRequest::Open(_))
            && !app.first_input_accepted
        {
            app.deferred_open_waiting = true;
            return None;
        }
        app.launch_open_completed = true;
        app.deferred_open_waiting = false;
        Some(app.launch.request.clone())
    });
    let Some(request) = request else {
        return 0;
    };
    match request {
        crate::launch::LaunchRequest::Open(path) => {
            if App::open_path(hwnd, std::path::Path::new(&path)).is_ok() {
                return 0;
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
    std::ffi::CString::new(loaded.text.as_str())
        .map_err(|_| crate::FastPadError::Invariant("Scintilla text may not contain NUL bytes"))?;
    let (editor, previous, candidate, active_ids) = {
        let mut app = unsafe { app_ptr(hwnd) }.ok_or(crate::FastPadError::Invariant(
            "main window app state was not available",
        ))?;
        let app = unsafe { app.as_mut() };
        let editor = app
            .editor
            .clone()
            .ok_or(crate::FastPadError::Invariant("editor was not initialized"))?;
        let active = app.tabs.active();
        let previous = active.handle.clone();
        let candidate = !active.dirty && active.path.is_none();
        let active_ids = (active.id, active.recovery_id);
        (editor, previous, candidate, active_ids)
    };
    let reuse = candidate && editor.text()?.is_empty();
    if !identity.is_live_for(hwnd) {
        return Err(crate::FastPadError::Invariant(
            "main window was destroyed during file open",
        ));
    }
    let (id, recovery_id) = if reuse {
        active_ids
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
            (Ok(()), Some(retired))
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
    invalidate_title_strip(hwnd);
    Ok(())
}

struct OpenInputHook {
    parent: HWND,
    identity: WindowIdentity,
}
const OPEN_INPUT_HOOK_ID: usize = 0x4650_4f49;

fn install_open_input_hook(parent: HWND, editor: HWND, identity: WindowIdentity) -> Result<()> {
    let data = std::rc::Rc::into_raw(std::rc::Rc::new(OpenInputHook { parent, identity })) as usize;
    if unsafe {
        windows_sys::Win32::UI::Shell::SetWindowSubclass(
            editor,
            Some(open_input_proc),
            OPEN_INPUT_HOOK_ID,
            data,
        )
    } == 0
    {
        unsafe {
            drop(std::rc::Rc::from_raw(data as *const OpenInputHook));
        }
        return Err(last_error());
    }
    Ok(())
}

unsafe extern "system" fn open_input_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    data: usize,
) -> LRESULT {
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass};
    let raw = data as *const OpenInputHook;
    unsafe {
        std::rc::Rc::increment_strong_count(raw);
    }
    let hook = unsafe { std::rc::Rc::from_raw(raw) };
    if message == WM_NCDESTROY {
        unsafe {
            RemoveWindowSubclass(hwnd, Some(open_input_proc), OPEN_INPUT_HOOK_ID);
            std::rc::Rc::decrement_strong_count(raw);
        }
    }
    let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
    if message == windows_sys::Win32::UI::WindowsAndMessaging::WM_CHAR
        && (wparam >= 0x20 || wparam == 9 || wparam == 13)
        && hook.identity.is_live_for(hook.parent)
    {
        let resume = unsafe { app_ptr(hook.parent) }.is_some_and(|mut app| {
            let app = unsafe { app.as_mut() };
            if !app.first_input_accepted {
                app.first_input_accepted = true;
                let _ = app.startup.record_now(Milestone::FirstInputAccepted);
            }
            std::mem::take(&mut app.deferred_open_waiting)
        });
        if resume {
            unsafe {
                PostMessageW(hook.parent, crate::window::WM_FASTPAD_OPEN_REQUEST, 0, 0);
            }
        }
    }
    result
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
    invalidate_title_strip(hwnd);
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
        Some((editor, app.tabs.active_handle().clone()))
    });
    let Some((editor, handle)) = target else {
        return false;
    };
    if editor.use_document(&handle).is_err() || !identity.is_live_for(hwnd) {
        return false;
    }
    invalidate_title_strip(hwnd);
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

    let current_len = unsafe { app_ptr(hwnd) }.and_then(|app| {
        let app = unsafe { app.as_ref() };
        (app.tabs.active_close_review() == Some(review)).then_some(app.tabs.len())
    });
    let Some(current_len) = current_len else {
        return;
    };
    let replacement = if current_len == 1 {
        let ids = unsafe { app_ptr(hwnd) }
            .map(|mut app| unsafe { app.as_mut() }.allocate_document_identity());
        let Some((id, recovery_id)) = ids else {
            return;
        };
        let Ok(handle) = editor.create_document() else {
            return;
        };
        Some(Document::untitled(id, recovery_id, handle))
    } else {
        None
    };
    if !identity.is_live_for(hwnd) {
        return;
    }

    let switched = unsafe { app_ptr(hwnd) }.and_then(|mut app| {
        let app = unsafe { app.as_mut() };
        let closed = app
            .tabs
            .close_reviewed(review, decision, replacement)
            .ok()?;
        Some((closed, app.tabs.active_handle().clone()))
    });
    let Some((closed, active)) = switched else {
        return;
    };
    let _ = editor.use_document(&active);
    drop(active);
    drop(closed);
    if identity.is_live_for(hwnd) {
        invalidate_title_strip(hwnd);
    }
}

fn save_active_document(hwnd: HWND) {
    let Some(identity) = (unsafe { window_identity(hwnd) }) else {
        return;
    };
    let has_path =
        unsafe { app_ptr(hwnd) }.map(|app| unsafe { app.as_ref() }.tabs.active().path.is_some());
    match has_path {
        Some(true) => complete_save(hwnd, &identity, None),
        Some(false) => save_active_document_as(hwnd),
        None => {}
    }
}

fn save_active_document_as(hwnd: HWND) {
    let Some(identity) = (unsafe { window_identity(hwnd) }) else {
        return;
    };
    let Some(suggested) = (unsafe { app_ptr(hwnd) }).map(|app| {
        let app = unsafe { app.as_ref() };
        app.tabs
            .active()
            .path
            .as_deref()
            .and_then(std::path::Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled.txt".to_owned())
    }) else {
        return;
    };
    // Modal Show reenters the window procedure. Only an owned identity crosses it.
    let selection = crate::window::commands::choose_save_path(hwnd, &suggested);
    if !identity.is_live_for(hwnd) {
        return;
    }
    let Ok(Some(path)) = selection else {
        // Cancellation is not an error; a real error is silently dropped, matching Open's
        // existing precedent above.
        return;
    };
    complete_save(hwnd, &identity, Some(path));
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
fn complete_save(hwnd: HWND, identity: &WindowIdentity, new_path: Option<std::path::PathBuf>) {
    let is_save_as = new_path.is_some();
    let mut original_path: Option<std::path::PathBuf> = None;
    if let Some(path) = new_path {
        original_path = unsafe { app_ptr(hwnd) }
            .and_then(|app| unsafe { app.as_ref() }.tabs.active().path.clone());
        let outcome = unsafe { app_ptr(hwnd) }
            .map(|mut app| unsafe { app.as_mut() }.tabs.set_active_path(path));
        match outcome {
            Some(Ok(())) => {}
            Some(Err(_)) => {
                show_save_error(
                    hwnd,
                    "This file is already open in another tab. Choose a different name.",
                );
                return;
            }
            None => return,
        }
    }
    if !identity.is_live_for(hwnd) {
        return;
    }
    let Some((editor, path, encoding)) = (unsafe { app_ptr(hwnd) }).and_then(|app| {
        let app = unsafe { app.as_ref() };
        let editor = app.editor.clone()?;
        let document = app.tabs.active();
        Some((editor, document.path.clone()?, document.encoding))
    }) else {
        if is_save_as {
            revert_active_path(hwnd, original_path);
        }
        return;
    };
    let Ok(text) = editor.text() else {
        if is_save_as {
            revert_active_path(hwnd, original_path);
        }
        return;
    };
    let bytes = crate::file::encoding::encode(&text, encoding);
    let result = crate::file::saver::save_atomic(&path, &bytes);
    if !identity.is_live_for(hwnd) {
        return;
    }
    match result {
        Ok(()) => {
            editor.set_save_point();
            if is_save_as {
                unsafe {
                    PostMessageW(hwnd, crate::window::WM_FASTPAD_APPLY_LANGUAGE, 0, 0);
                }
                invalidate_title_strip(hwnd);
            }
        }
        Err(_) => {
            if is_save_as {
                revert_active_path(hwnd, original_path);
            }
            show_save_error(
                hwnd,
                "FastPad could not save this file. The previous version on disk was not modified.",
            );
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

// A real MessageBoxW is a blocking, modal native dialog. Driving it deterministically from an
// automated integration test proved unreliable on this host (see the long comment in
// tests/windows/save_file.rs), so the test build records the message instead of showing it;
// production behavior (the real MessageBoxW) is unchanged.
#[cfg(not(test))]
fn show_save_error(hwnd: HWND, message: &str) {
    let text = wide_null(message);
    let caption = wide_null("FastPad");
    unsafe {
        MessageBoxW(hwnd, text.as_ptr(), caption.as_ptr(), MB_ICONERROR | MB_OK);
    }
}

#[cfg(test)]
fn show_save_error(_hwnd: HWND, message: &str) {
    SAVE_ERRORS.with(|errors| errors.borrow_mut().push(message.to_owned()));
}

#[cfg(test)]
thread_local! {
    static SAVE_ERRORS: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Test-only accessor for the messages `show_save_error` would otherwise have shown as a real
/// MessageBoxW. Clears the recorded list.
#[cfg(test)]
#[allow(
    dead_code,
    reason = "consumed by the source-linked save_file integration target"
)]
pub(crate) fn take_save_errors() -> Vec<String> {
    SAVE_ERRORS.with(|errors| std::mem::take(&mut *errors.borrow_mut()))
}

fn review_dirty_documents(hwnd: HWND) -> bool {
    let Some(identity) = (unsafe { window_identity(hwnd) }) else {
        return false;
    };
    let mut reviewed = Vec::<CloseReviewKey>::new();
    loop {
        let pending = unsafe { app_ptr(hwnd) }.and_then(|app| {
            let app = unsafe { app.as_ref() };
            let review = app.tabs.next_dirty_review(&reviewed)?;
            let title = app.tabs.document(review.id)?.title();
            Some((review, title))
        });
        let Some((review, title)) = pending else {
            return true;
        };
        if prompt_close_decision(hwnd, &title) == CloseDecision::Cancel {
            return false;
        }
        if !identity.is_live_for(hwnd) {
            return false;
        }
        let current = unsafe { app_ptr(hwnd) }
            .map(|app| unsafe { app.as_ref() }.tabs.dirty_review_is_current(review))
            .unwrap_or(false);
        if current {
            reviewed.push(review.key());
        }
    }
}

fn prompt_close_decision(hwnd: HWND, title: &str) -> CloseDecision {
    let message = wide_null(&format!("Save changes to {title} before closing?"));
    let caption = wide_null("FastPad");
    match unsafe {
        MessageBoxW(
            hwnd,
            message.as_ptr(),
            caption.as_ptr(),
            MB_YESNOCANCEL | MB_ICONWARNING,
        )
    } {
        IDYES => CloseDecision::Save,
        IDNO => CloseDecision::Discard,
        IDCANCEL => CloseDecision::Cancel,
        _ => CloseDecision::Cancel,
    }
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
    if notification.code == crate::editor::scintilla_constants::SCN_MODIFIED {
        let modification = unsafe { &*(lparam as *const TextModificationNotification) };
        let text_changes = crate::editor::scintilla_constants::SC_MOD_INSERTTEXT
            | crate::editor::scintilla_constants::SC_MOD_DELETETEXT;
        if modification.modification_type & text_changes as i32 != 0
            && let Some(mut app) = unsafe { app_ptr(hwnd) }
        {
            unsafe { app.as_mut() }.tabs.note_active_text_change();
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

unsafe fn app_ptr(hwnd: HWND) -> Option<NonNull<App>> {
    // SAFETY: `GWLP_USERDATA` is written exactly once from `WM_NCCREATE` with a `Box<App>` owned
    // by the window and cleared in `WM_NCDESTROY`. Callers must not keep references alive across
    // reentrant Win32 calls; they may only copy values or perform immediate mutation.
    NonNull::new(unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App })
}

unsafe fn window_identity(hwnd: HWND) -> Option<WindowIdentity> {
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
        MainWindowClass, WindowCreateContext, dark_mode_from_apps_use_light_theme, execute_command,
        handle_paint_with, mark_first_paint_complete, take_deferred_start_pending,
        take_json_issues, take_json_valid_count, take_language_errors,
    };
    use crate::app::App;
    use crate::document::Language;
    use crate::languages::LanguageManager;
    use crate::launch::LaunchOptions;
    use crate::perf::StartupMetrics;
    use crate::window::commands::CommandId;
    use std::cell::RefCell;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use windows_sys::Win32::Foundation::{HWND, LRESULT};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DestroyWindow, GWLP_USERDATA, GetWindowLongPtrW, IsWindow, WM_PAINT,
    };

    #[test]
    fn dark_mode_is_read_from_the_apps_use_light_theme_registry_value() {
        // Break caught: inverting the 0/1 sense of AppsUseLightTheme would style every JSON/
        // Markdown document with the wrong theme's colors.
        assert!(!dark_mode_from_apps_use_light_theme(Some(1)));
        assert!(dark_mode_from_apps_use_light_theme(Some(0)));
        assert!(!dark_mode_from_apps_use_light_theme(None));
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
            .language;
        assert_eq!(language_before, Language::PlainText);

        execute_command(window.hwnd, CommandId::LanguageJson);

        assert_eq!(
            take_language_errors(),
            vec![
                "FastPad could not enable syntax highlighting for this file. It will remain in \
                 plain text."
                    .to_owned()
            ]
        );
        let language_after = unsafe { super::app_ptr(window.hwnd).unwrap().as_ref() }
            .tabs
            .active()
            .language;
        assert_eq!(language_after, Language::PlainText);
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
        assert!(take_json_issues().is_empty());
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
        let issues = take_json_issues();
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

        assert_eq!(take_json_valid_count(), 1);
        assert!(take_json_issues().is_empty());
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

        assert_eq!(take_json_valid_count(), 0);
        let issues = take_json_issues();
        assert_eq!(issues.len(), 1);
        assert!(
            issues[0].contains("line 2") && issues[0].contains("column 3"),
            "{}",
            issues[0]
        );
        assert_eq!(editor.text().unwrap(), "{\n  bad\n}");
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
