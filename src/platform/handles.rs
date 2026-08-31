use crate::Result;
use crate::platform::win32;
use windows_sys::Win32::Foundation::{HANDLE, HMODULE};

#[derive(Debug)]
pub struct OwnedModule(HMODULE);

impl OwnedModule {
    pub fn new(raw: HMODULE) -> Result<Self> {
        win32::require_module(raw).map(Self)
    }

    pub fn as_raw(&self) -> HMODULE {
        self.0
    }
}

impl Drop for OwnedModule {
    fn drop(&mut self) {
        if win32::module_is_valid(self.0) {
            win32::free_library(self.0);
        }
    }
}

#[derive(Debug)]
pub struct OwnedHandle(HANDLE);

impl OwnedHandle {
    pub fn new(raw: HANDLE) -> Result<Self> {
        win32::require_handle(raw).map(Self)
    }

    pub fn as_raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if win32::handle_is_valid(self.0) {
            win32::close_handle(self.0);
        }
    }
}
