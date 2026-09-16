use crate::config::Settings;
use crate::document::{DocumentId, RecoveryId};
use crate::editor::Editor;
use crate::languages::LanguageManager;
use crate::launch::LaunchOptions;
use crate::perf::{Milestone, StartupMetrics};
use crate::platform::theme::SystemTheme;
use crate::window::accessibility::AccessibilityState;
use crate::window::command_palette::CommandPalette;
use crate::window::commands::CommandId;
use crate::window::find_bar::FindBar;
use crate::window::menu_band::MenuMode;
use crate::window::menus::{AcceleratorTable, MenuBar};
use crate::window::notification::NotificationCenter;
use crate::window::status::StatusModel;
use crate::window::tabs::Tabs;
use crate::window::titlebar::{PointerState, TitleFonts};
use std::cell::Cell;
use std::ffi::c_void;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use windows_sys::Win32::Foundation::HWND;

#[derive(Clone, Debug)]
pub(crate) struct WindowIdentity {
    state: Rc<Cell<WindowIdentityState>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WindowIdentityState {
    Unbound,
    Live(HWND),
    Invalidated,
}

#[derive(Debug)]
pub struct App {
    pub hwnd: HWND,
    pub editor: Option<Editor>,
    pub launch: LaunchOptions,
    pub startup: StartupMetrics,
    pub(crate) tabs: Tabs,
    pub(crate) accessibility: AccessibilityState,
    pub(crate) accelerators: Option<AcceleratorTable>,
    pub(crate) menu_bar: Option<MenuBar>,
    /// Present while the Alt/F10 menu band is showing.
    pub(crate) menu_mode: Option<MenuMode>,
    /// Where focus returns when menu mode ends; the frame holds it meanwhile for the key handling.
    pub(crate) menu_return_focus: HWND,
    pub(crate) find_bar: Option<FindBar>,
    pub(crate) command_palette: Option<CommandPalette>,
    pub(crate) language_manager: Option<LanguageManager>,
    pub(crate) settings: Settings,
    pub(crate) theme: Option<SystemTheme>,
    pub(crate) status: Option<StatusModel>,
    pub(crate) title_fonts: Option<TitleFonts>,
    pub(crate) title_pointer: PointerState,
    /// While the tab scroll thumb is dragged: where along the thumb the pointer grabbed it.
    pub(crate) tab_thumb_grab: Option<i32>,
    pub(crate) dark_frame_applied: bool,
    pub(crate) notifications: NotificationCenter,
    pub(crate) launch_open_completed: bool,
    pub(crate) populating_file: bool,
    pub(crate) modal_depth: u32,
    pub(crate) held_messages: Vec<u32>,
    identity: WindowIdentity,
    first_paint_completed: bool,
    deferred_start_pending: bool,
    prioritize_input: bool,
    menu_alt_pending: bool,
    pub(crate) recovery_root: Option<std::path::PathBuf>,
    pub(crate) recovery_owner: Option<crate::platform::OwnedHandle>,
    // Declared before `instance_mutex` so an emergency drop closes the pipe before releasing the
    // mutex: a new primary must never claim the session while this server still exists.
    pub(crate) ipc: Option<crate::ipc::IpcServer>,
    pub(crate) instance_mutex: Option<crate::platform::OwnedHandle>,
    pub(crate) ipc_requests: Vec<crate::ipc::IpcRequest>,
    pub(crate) last_snapshot_duration: Option<std::time::Duration>,
    pub(crate) last_snapshot_attempt: Option<DocumentId>,
    next_document_id: u64,
    process_start: u64,
}

static NEXT_RECOVERY_COUNTER: AtomicU64 = AtomicU64::new(1);

impl App {
    pub fn new(launch: LaunchOptions, startup: StartupMetrics) -> Self {
        Self {
            hwnd: std::ptr::null_mut(),
            editor: None,
            launch,
            tabs: Tabs::new(),
            accessibility: AccessibilityState::default(),
            accelerators: AcceleratorTable::create().ok(),
            menu_bar: None,
            menu_mode: None,
            menu_return_focus: std::ptr::null_mut(),
            find_bar: None,
            command_palette: None,
            language_manager: None,
            settings: crate::config::default_settings(),
            theme: None,
            status: None,
            title_fonts: None,
            title_pointer: PointerState::default(),
            tab_thumb_grab: None,
            dark_frame_applied: false,
            notifications: NotificationCenter::new(),
            launch_open_completed: false,
            populating_file: false,
            modal_depth: 0,
            held_messages: Vec::new(),
            identity: WindowIdentity {
                state: Rc::new(Cell::new(WindowIdentityState::Unbound)),
            },
            first_paint_completed: false,
            deferred_start_pending: false,
            prioritize_input: false,
            menu_alt_pending: false,
            recovery_root: None,
            recovery_owner: None,
            ipc: None,
            instance_mutex: None,
            ipc_requests: Vec::new(),
            last_snapshot_duration: None,
            last_snapshot_attempt: None,
            next_document_id: 2,
            process_start: startup.start_tick() as u64,
            startup,
        }
    }

    pub fn mark_first_paint_complete(&mut self) {
        if !self.first_paint_completed {
            let _ = self.startup.record_now(Milestone::FirstPaint);
            self.first_paint_completed = true;
            self.deferred_start_pending = true;
        }
    }

    pub fn take_deferred_start_pending(&mut self) -> bool {
        let pending = self.deferred_start_pending;
        self.deferred_start_pending = false;
        pending
    }

    pub fn request_input_priority(&mut self) {
        self.prioritize_input = true;
    }

    pub fn clear_input_priority(&mut self) {
        self.prioritize_input = false;
    }

    pub fn prioritizes_input(&self) -> bool {
        self.prioritize_input
    }

    pub(crate) fn set_menu_alt_pending(&mut self, pending: bool) {
        self.menu_alt_pending = pending;
    }

    pub(crate) fn take_menu_alt_pending(&mut self) -> bool {
        std::mem::take(&mut self.menu_alt_pending)
    }

    pub fn execute(&mut self, command: CommandId) {
        if command == CommandId::Exit && !self.hwnd.is_null() {
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                    self.hwnd,
                    windows_sys::Win32::UI::WindowsAndMessaging::WM_CLOSE,
                    0,
                    0,
                );
            }
        }
    }

    // HWND-based orchestration deliberately does not borrow App across native callbacks.
    pub(crate) fn open_path(hwnd: HWND, path: &std::path::Path) -> crate::Result<()> {
        crate::window::open_path(hwnd, path)
    }

    pub(crate) fn allocate_document_identity(&mut self) -> (DocumentId, RecoveryId) {
        let id = DocumentId(self.next_document_id);
        self.next_document_id = self.next_document_id.saturating_add(1);
        (id, self.allocate_recovery_id())
    }

    pub(crate) fn allocate_recovery_id(&self) -> RecoveryId {
        let counter = NEXT_RECOVERY_COUNTER.fetch_add(1, Ordering::Relaxed);
        RecoveryId::compose(self.process_start, std::process::id(), counter)
    }

    pub(crate) fn recovery_owner_id(&self) -> RecoveryId {
        RecoveryId::compose(self.process_start, std::process::id(), 0)
    }

    pub(crate) fn owns_recovery_id(&self, id: RecoveryId) -> bool {
        id.is_from_process(self.process_start, std::process::id())
    }

    pub(crate) fn ensure_accessibility(&mut self) -> *mut c_void {
        self.accessibility
            .ensure(self.hwnd, self.tabs.view(), self.tabs.selection())
    }

    pub(crate) fn window_identity(&self) -> WindowIdentity {
        self.identity.clone()
    }

    pub(crate) fn bind_window(&mut self, hwnd: HWND) -> bool {
        if self.identity.state.get() != WindowIdentityState::Unbound {
            return false;
        }
        self.hwnd = hwnd;
        self.identity.state.set(WindowIdentityState::Live(hwnd));
        true
    }

    pub(crate) fn invalidate_window(&self, hwnd: HWND) {
        if self.identity.state.get() == WindowIdentityState::Live(hwnd) {
            self.identity.state.set(WindowIdentityState::Invalidated);
        }
    }
}

impl WindowIdentity {
    pub(crate) fn is_live_for(&self, hwnd: HWND) -> bool {
        self.state.get() == WindowIdentityState::Live(hwnd)
    }

    #[cfg(test)]
    pub(crate) fn is_invalidated(&self) -> bool {
        self.state.get() == WindowIdentityState::Invalidated
    }
}

#[cfg(test)]
mod tests {
    use super::App;
    use crate::launch::LaunchOptions;
    use crate::perf::StartupMetrics;

    #[test]
    fn multiple_paints_schedule_deferred_start_only_once() {
        // Break caught: clearing the only first-paint latch lets later WM_PAINT messages restart
        // the deferred startup chain.
        let mut app = App::new(
            LaunchOptions::default(),
            StartupMetrics::with_frequency(1, 0),
        );

        app.mark_first_paint_complete();
        assert!(app.take_deferred_start_pending());

        app.mark_first_paint_complete();
        assert!(!app.take_deferred_start_pending());
    }
}
