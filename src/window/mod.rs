pub(crate) mod accessibility;
pub mod commands;
mod main_window;
pub(crate) mod menus;
mod messages;
pub mod tabs;
pub mod titlebar;

#[cfg(test)]
pub(crate) use main_window::with_test_input_queue_status;
pub(crate) use main_window::{
    INPUT_MESSAGE_FIRST, INPUT_MESSAGE_LAST, MainWindowClass, WindowCreateContext,
    clear_input_priority, initialize_editor_with, input_priority_requested,
    input_queue_status_mask, maybe_post_deferred_start, translate_accelerator,
};
pub use messages::{
    WM_FASTPAD_APPLY_LANGUAGE, WM_FASTPAD_BUILD_CHROME, WM_FASTPAD_LOAD_SETTINGS,
    WM_FASTPAD_OPEN_REQUEST, WM_FASTPAD_RECOVERY, WM_FASTPAD_START_IPC,
};
