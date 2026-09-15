pub(crate) mod accessibility;
pub mod commands;
pub mod find_bar;
mod main_window;
pub(crate) mod menus;
mod messages;
pub mod tabs;
pub mod titlebar;

pub(crate) use main_window::{
    INPUT_MESSAGE_FIRST, INPUT_MESSAGE_LAST, MainWindowClass, WindowCreateContext,
    clear_input_priority, initialize_editor_with, input_priority_requested,
    input_queue_status_mask, maybe_post_deferred_start, open_path, translate_accelerator,
};
#[cfg(test)]
#[allow(
    unused_imports,
    reason = "consumed by the source-linked save_file integration target"
)]
pub(crate) use main_window::{save_path_as, take_save_errors, with_test_input_queue_status};
#[cfg(test)]
#[allow(
    unused_imports,
    reason = "consumed by the source-linked json_commands integration target"
)]
pub(crate) use main_window::{take_json_issues, take_json_valid_count};
pub use messages::{
    WM_FASTPAD_APPLY_LANGUAGE, WM_FASTPAD_BUILD_CHROME, WM_FASTPAD_LOAD_SETTINGS,
    WM_FASTPAD_OPEN_REQUEST, WM_FASTPAD_RECOVERY, WM_FASTPAD_START_IPC,
};
