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
    first_paint_completed: bool,
    deferred_start_pending: bool,
    prioritize_input: bool,
}

impl App {
    pub fn new(launch: LaunchOptions, startup: StartupMetrics) -> Self {
        Self {
            hwnd: std::ptr::null_mut(),
            editor: None,
            launch,
            startup,
            first_paint_completed: false,
            deferred_start_pending: false,
            prioritize_input: false,
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
