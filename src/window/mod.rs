mod main_window;
mod messages;

pub(crate) use main_window::{
    MainWindowClass, WindowCreateContext, app_mut, clear_input_priority, input_priority_requested,
    maybe_post_deferred_start,
};
pub use messages::{
    WM_FASTPAD_APPLY_LANGUAGE, WM_FASTPAD_BUILD_CHROME, WM_FASTPAD_LOAD_SETTINGS,
    WM_FASTPAD_OPEN_REQUEST, WM_FASTPAD_RECOVERY, WM_FASTPAD_START_IPC,
};
