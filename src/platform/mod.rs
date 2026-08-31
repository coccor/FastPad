pub mod handles;
pub mod win32;

pub use handles::{OwnedHandle, OwnedModule};
pub use win32::{last_error, wide_null};
