#[cfg(windows)]
mod win32;

#[cfg(windows)]
pub use win32::WindowHarness;
