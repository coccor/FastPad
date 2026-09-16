pub(crate) mod accessibility;
pub(crate) mod command_palette;
pub mod commands;
pub mod find_bar;
mod main_window;
pub(crate) mod menus;
mod messages;
mod modal;
pub mod notification;
pub mod palette;
pub mod status;
pub mod tabs;
pub mod titlebar;

pub(crate) use main_window::{
    INPUT_MESSAGE_FIRST, INPUT_MESSAGE_LAST, MainWindowClass, WindowCreateContext,
    clear_input_priority, initialize_editor_with, input_priority_requested,
    input_queue_status_mask, ipc_wait_handle, maybe_post_deferred_start, open_path, service_ipc,
    translate_accelerator,
};
#[cfg(test)]
#[allow(
    unused_imports,
    reason = "consumed by the source-linked save_file integration target"
)]
pub(crate) use main_window::{save_path_as, with_test_input_queue_status};
pub use messages::{
    WM_FASTPAD_APPLY_LANGUAGE, WM_FASTPAD_BUILD_CHROME, WM_FASTPAD_DIAGNOSTIC_JSON_COUNT,
    WM_FASTPAD_IPC_REQUEST, WM_FASTPAD_LOAD_SETTINGS, WM_FASTPAD_OPEN_REQUEST, WM_FASTPAD_RECOVERY,
    WM_FASTPAD_START_IPC,
};
#[cfg(test)]
#[allow(
    unused_imports,
    reason = "consumed by the source-linked save_file integration target"
)]
pub(crate) use modal::{answer_next_close_prompt, answer_next_save_dialog};
