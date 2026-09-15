use crate::Result;
use crate::app::{App, WindowIdentity};
use crate::document::{CloseDecision, Document, DocumentId, RecoveryId};
use crate::editor::Editor;
use crate::perf::Milestone;
use crate::platform::{last_error, wide_null};
use crate::window::accessibility::{self, AccessibleSelectRequest, WM_FASTPAD_ACCESSIBLE_SELECT};
use crate::window::commands::CommandId;
use crate::window::menus::{self, MenuBar};
use crate::window::messages::{
    DeferredAction, classify_deferred_message, completed_milestone, deferred_start_message,
};
use crate::window::tabs::{CloseReviewKey, Tabs};
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
            if let Some(editor_hwnd) = unsafe { editor_hwnd(hwnd) } {
                let mut rect = Default::default();
                let title_height =
                    crate::window::titlebar::layout_for_window(hwnd, tab_count(hwnd)).height;
                unsafe {
                    GetClientRect(hwnd, &mut rect);
                    MoveWindow(
                        editor_hwnd,
                        0,
                        title_height,
                        rect.right - rect.left,
                        (rect.bottom - rect.top - title_height).max(0),
                        1,
                    );
                }
            }
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
    match command {
        CommandId::New => {
            let _ = create_new_document(hwnd);
        }
        CommandId::CloseTab => close_active_document(hwnd),
        _ => {
            if let Some(mut app) = unsafe { app_ptr(hwnd) } {
                unsafe { app.as_mut() }.execute(command);
            }
        }
    }
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
        view.tabs
            .get(index)
            .map(|tab| (tab.id, view.revision))
    });
    if let Some((id, revision)) = target {
        let _ = activate_document(hwnd, id, revision);
    }
}

fn activate_document(hwnd: HWND, id: DocumentId, revision: u64) -> bool {
    let Some(identity) = (unsafe { window_identity(hwnd) }) else {
        return false;
    };
    let target = unsafe { app_ptr(hwnd) }.and_then(|mut app| {
        let app = unsafe { app.as_mut() };
        if app.tabs.view().snapshot().revision != revision {
            return None;
        }
        app.tabs.activate(id).ok()?;
        Some((app.editor.clone()?, app.tabs.active_handle().clone()))
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
        let closed = app.tabs.close_reviewed(review, decision, replacement).ok()?;
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
    if lparam == 0 {
        return;
    }
    let notification = unsafe { &*(lparam as *const NMHDR) };
    if unsafe { editor_hwnd(hwnd) } != Some(notification.hwndFrom) {
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
            let changed = app.tabs.set_active_dirty(dirty);
            changed
        })
        .unwrap_or(false);
    if changed {
        invalidate_title_strip(hwnd);
    }
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
    app.tabs = Tabs::with_document(document);
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
        MainWindowClass, WindowCreateContext, handle_paint_with, mark_first_paint_complete,
        take_deferred_start_pending,
    };
    use crate::app::App;
    use crate::launch::LaunchOptions;
    use crate::perf::StartupMetrics;
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
