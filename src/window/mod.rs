mod main_window;
mod messages;

pub use main_window::{MainWindowClass, maybe_post_deferred_start};
pub use messages::{
    WM_FASTPAD_APPLY_LANGUAGE, WM_FASTPAD_BUILD_CHROME, WM_FASTPAD_LOAD_SETTINGS,
    WM_FASTPAD_OPEN_REQUEST, WM_FASTPAD_RECOVERY, WM_FASTPAD_START_IPC,
};
