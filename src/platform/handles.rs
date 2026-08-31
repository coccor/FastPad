use crate::Result;
use crate::platform::win32;
use windows_sys::Win32::Foundation::{HANDLE, HMODULE};

#[derive(Debug)]
pub struct OwnedModule(HMODULE);

impl OwnedModule {
    /// Takes ownership of a module handle that must be released exactly once with `FreeLibrary`.
    ///
    /// # Safety
    ///
    /// `raw` must be a valid, uniquely owned `HMODULE` obtained from a Win32 API that transfers
    /// ownership to the caller. Passing a borrowed handle, such as one returned by
    /// `GetModuleHandleW`, causes `Drop` to call `FreeLibrary` on a module this type does not own.
    pub unsafe fn from_raw_owned(raw: HMODULE) -> Result<Self> {
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
    /// Takes ownership of a kernel handle that must be released exactly once with `CloseHandle`.
    ///
    /// # Safety
    ///
    /// `raw` must be a valid, uniquely owned `HANDLE` obtained from a Win32 API that transfers
    /// ownership to the caller. The caller must not pass a borrowed handle, `NULL`, or
    /// `INVALID_HANDLE_VALUE`.
    pub unsafe fn from_raw_owned(raw: HANDLE) -> Result<Self> {
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

#[cfg(test)]
mod tests {
    use super::{OwnedHandle, OwnedModule};
    use crate::FastPadError;
    use windows_sys::Win32::Foundation::{HMODULE, INVALID_HANDLE_VALUE};

    #[test]
    fn owned_module_rejects_null_raw_module() {
        // Break caught: accepting a null module handle and then attempting to free it in Drop.
        let result = unsafe { OwnedModule::from_raw_owned(std::ptr::null_mut()) };
        assert!(matches!(result, Err(FastPadError::Win32(_))));
    }

    #[test]
    fn owned_handle_rejects_null_raw_handle() {
        // Break caught: accepting a null handle and then attempting to close it in Drop.
        let result = unsafe { OwnedHandle::from_raw_owned(std::ptr::null_mut()) };
        assert!(matches!(result, Err(FastPadError::Win32(_))));
    }

    #[test]
    fn owned_handle_rejects_invalid_handle_value() {
        // Break caught: treating INVALID_HANDLE_VALUE as owned and later passing a failed handle to CloseHandle.
        let result = unsafe { OwnedHandle::from_raw_owned(INVALID_HANDLE_VALUE) };
        assert!(matches!(result, Err(FastPadError::Win32(_))));
    }

    #[test]
    fn owned_module_preserves_raw_value() {
        // Break caught: altering a caller-owned module handle instead of storing the exact owned raw value.
        let raw = 1 as HMODULE;
        let module = unsafe { OwnedModule::from_raw_owned(raw) }.unwrap();
        assert_eq!(module.as_raw(), raw);
        std::mem::forget(module);
    }
}
