use crate::{FastPadError, Result};
use windows_sys::Win32::Foundation::{HANDLE, HMODULE};

#[cfg(windows)]
use windows_sys::Win32::Foundation::{CloseHandle, FreeLibrary, GetLastError};

pub fn wide_null(value: &str) -> Vec<u16> {
    let mut units: Vec<u16> = value.encode_utf16().collect();
    while units.last().copied() == Some(0) {
        units.pop();
    }
    units.push(0);
    units
}

pub fn last_error() -> FastPadError {
    FastPadError::Win32(raw_last_error())
}

pub(crate) fn require_module(raw: HMODULE) -> Result<HMODULE> {
    if module_is_valid(raw) {
        Ok(raw)
    } else {
        Err(last_error())
    }
}

pub(crate) fn require_handle(raw: HANDLE) -> Result<HANDLE> {
    if handle_is_valid(raw) {
        Ok(raw)
    } else {
        Err(last_error())
    }
}

pub(crate) fn module_is_valid(raw: HMODULE) -> bool {
    !raw.is_null()
}

pub(crate) fn handle_is_valid(raw: HANDLE) -> bool {
    !raw.is_null()
}

#[cfg(windows)]
pub(crate) fn free_library(module: HMODULE) {
    unsafe {
        FreeLibrary(module);
    }
}

#[cfg(not(windows))]
pub(crate) fn free_library(_module: HMODULE) {}

#[cfg(windows)]
pub(crate) fn close_handle(handle: HANDLE) {
    unsafe {
        CloseHandle(handle);
    }
}

#[cfg(not(windows))]
pub(crate) fn close_handle(_handle: HANDLE) {}

#[cfg(windows)]
fn raw_last_error() -> u32 {
    unsafe { GetLastError() }
}

#[cfg(not(windows))]
fn raw_last_error() -> u32 {
    0
}

#[cfg(test)]
mod tests {
    use super::wide_null;

    #[test]
    fn wide_null_appends_exactly_one_terminator() {
        // Break caught: forgetting the trailing NUL or appending more than one terminator.
        assert_eq!(wide_null("FastPad"), vec![70, 97, 115, 116, 80, 97, 100, 0]);
    }
}
