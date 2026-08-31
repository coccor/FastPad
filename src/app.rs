use crate::editor::Editor;
use crate::launch::LaunchOptions;
use crate::perf::{Milestone, StartupMetrics};
use windows_sys::Win32::Foundation::HWND;

#[derive(Debug)]
pub struct App {
    pub hwnd: HWND,
    pub editor: Option<Editor>,
    pub launch: LaunchOptions,
    pub startup: StartupMetrics,
    deferred_start_pending: bool,
}

impl App {
    pub fn new(launch: LaunchOptions, startup: StartupMetrics) -> Self {
        Self {
            hwnd: std::ptr::null_mut(),
            editor: None,
            launch,
            startup,
            deferred_start_pending: false,
        }
    }

    pub fn mark_first_paint_complete(&mut self) {
        if !self.deferred_start_pending {
            let _ = self.startup.record_now(Milestone::FirstPaint);
            self.deferred_start_pending = true;
        }
    }

    pub fn take_deferred_start_pending(&mut self) -> bool {
        let pending = self.deferred_start_pending;
        self.deferred_start_pending = false;
        pending
    }
}
