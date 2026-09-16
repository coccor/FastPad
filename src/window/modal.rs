//! Nested modal loops (common file dialogs, `MessageBoxW`) re-enter the window procedure. While one
//! runs, deferred startup units, the IPC drain, and recovery snapshots are held back so they cannot
//! change which document the modal operation ends up acting on.

use super::main_window::{app_ptr, window_identity};
use crate::app::WindowIdentity;
use crate::document::CloseDecision;
use crate::platform::wide_null;
use std::path::PathBuf;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    IDCANCEL, IDNO, IDYES, MB_ICONWARNING, MB_YESNOCANCEL, MessageBoxW, PostMessageW,
};

/// Raises `App::modal_depth` for its lifetime; leaving the outermost scope re-posts held messages.
pub(super) struct ModalScope {
    hwnd: HWND,
    identity: Option<WindowIdentity>,
}

impl ModalScope {
    pub(super) fn enter(hwnd: HWND) -> Self {
        let identity = unsafe { window_identity(hwnd) };
        if let Some(mut app) = unsafe { app_ptr(hwnd) } {
            let app = unsafe { app.as_mut() };
            app.modal_depth = app.modal_depth.saturating_add(1);
        }
        Self { hwnd, identity }
    }
}

impl Drop for ModalScope {
    fn drop(&mut self) {
        if !self
            .identity
            .as_ref()
            .is_some_and(|identity| identity.is_live_for(self.hwnd))
        {
            return;
        }
        let held = unsafe { app_ptr(self.hwnd) }
            .map(|mut app| {
                let app = unsafe { app.as_mut() };
                app.modal_depth = app.modal_depth.saturating_sub(1);
                if app.modal_depth == 0 {
                    std::mem::take(&mut app.held_messages)
                } else {
                    Vec::new()
                }
            })
            .unwrap_or_default();
        for message in held {
            unsafe {
                PostMessageW(self.hwnd, message, 0, 0);
            }
        }
    }
}

pub(super) fn modal_active(hwnd: HWND) -> bool {
    unsafe { app_ptr(hwnd) }.is_some_and(|app| unsafe { app.as_ref() }.modal_depth > 0)
}

/// Returns true when `message` was held for re-posting after the outermost modal loop ends.
pub(super) fn hold_while_modal(hwnd: HWND, message: u32) -> bool {
    unsafe { app_ptr(hwnd) }.is_some_and(|mut app| {
        let app = unsafe { app.as_mut() };
        if app.modal_depth == 0 {
            return false;
        }
        if !app.held_messages.contains(&message) {
            app.held_messages.push(message);
        }
        true
    })
}

/// The only modal prompt FastPad shows outside startup-fatal errors.
pub(super) fn prompt_close_decision(hwnd: HWND, title: &str) -> CloseDecision {
    let _modal = ModalScope::enter(hwnd);
    #[cfg(test)]
    if let Some(answer) = CLOSE_ANSWERS.with(|answers| answers.borrow_mut().pop_front()) {
        return answer(hwnd);
    }
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

pub(super) fn choose_save_path(hwnd: HWND, suggested_name: &str) -> crate::Result<Option<PathBuf>> {
    let _modal = ModalScope::enter(hwnd);
    #[cfg(test)]
    if let Some(answer) = SAVE_ANSWERS.with(|answers| answers.borrow_mut().pop_front()) {
        return Ok(answer(hwnd));
    }
    crate::window::commands::choose_save_path(hwnd, suggested_name)
}

pub(super) fn choose_open_path(hwnd: HWND) -> crate::Result<Option<PathBuf>> {
    let _modal = ModalScope::enter(hwnd);
    crate::window::commands::choose_open_path(hwnd)
}

#[cfg(test)]
type Answer<T> = Box<dyn FnOnce(HWND) -> T>;

#[cfg(test)]
thread_local! {
    static CLOSE_ANSWERS: std::cell::RefCell<std::collections::VecDeque<Answer<CloseDecision>>> =
        const { std::cell::RefCell::new(std::collections::VecDeque::new()) };
    static SAVE_ANSWERS: std::cell::RefCell<std::collections::VecDeque<Answer<Option<PathBuf>>>> =
        const { std::cell::RefCell::new(std::collections::VecDeque::new()) };
}

/// Answers the next close prompt from inside its modal scope instead of showing `MessageBoxW`.
#[cfg(test)]
pub(crate) fn answer_next_close_prompt(answer: impl FnOnce(HWND) -> CloseDecision + 'static) {
    CLOSE_ANSWERS.with(|answers| answers.borrow_mut().push_back(Box::new(answer)));
}

/// Answers the next Save As dialog from inside its modal scope; `None` is Cancel.
#[cfg(test)]
pub(crate) fn answer_next_save_dialog(answer: impl FnOnce(HWND) -> Option<PathBuf> + 'static) {
    SAVE_ANSWERS.with(|answers| answers.borrow_mut().push_back(Box::new(answer)));
}
