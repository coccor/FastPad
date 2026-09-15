//! Native file picking. COM and the dialog exist only for the duration of an Open command.

use crate::{FastPadError, Result};
use std::ffi::{OsString, c_void};
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoTaskMemFree, CoUninitialize,
};
use windows_sys::Win32::UI::Shell::{
    FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, FileOpenDialog, SIGDN_FILESYSPATH,
};
use windows_sys::core::{GUID, HRESULT};

const IID_IFILE_OPEN_DIALOG: GUID = GUID::from_u128(0xd57c7288_d4ad_4768_be02_9d969532d960);
const CANCELLED: HRESULT = 0x800704c7u32 as i32;

#[repr(C)]
struct UnknownVtable {
    query_interface: usize,
    add_ref: usize,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
}

// Prefixes follow the SDK's IUnknown -> IModalWindow -> IFileDialog and IShellItem ABI.
#[repr(C)]
struct FileDialogVtable {
    unknown: UnknownVtable,
    show: unsafe extern "system" fn(*mut c_void, HWND) -> HRESULT,
    set_file_types: usize,
    set_file_type_index: usize,
    get_file_type_index: usize,
    advise: usize,
    unadvise: usize,
    set_options: unsafe extern "system" fn(*mut c_void, u32) -> HRESULT,
    get_options: unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT,
    set_default_folder: usize,
    set_folder: usize,
    get_folder: usize,
    get_current_selection: usize,
    set_file_name: usize,
    get_file_name: usize,
    set_title: usize,
    set_ok_button_label: usize,
    set_file_name_label: usize,
    get_result: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT,
}

#[repr(C)]
struct ShellItemVtable {
    unknown: UnknownVtable,
    bind_to_handler: usize,
    get_parent: usize,
    get_display_name: unsafe extern "system" fn(*mut c_void, i32, *mut *mut u16) -> HRESULT,
}

struct ComApartment;
impl ComApartment {
    fn initialize() -> Result<Self> {
        check(unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) })?;
        // S_FALSE is success too and also requires a balancing CoUninitialize.
        Ok(Self)
    }
}
impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}

struct Interface(*mut c_void);
impl Interface {
    fn require(&self) -> Result<()> {
        if self.0.is_null() {
            Err(FastPadError::Invariant("COM returned a null interface"))
        } else {
            Ok(())
        }
    }
    fn dialog(&self) -> &FileDialogVtable {
        unsafe { &**(self.0 as *const *const FileDialogVtable) }
    }
    fn shell_item(&self) -> &ShellItemVtable {
        unsafe { &**(self.0 as *const *const ShellItemVtable) }
    }
    fn show(&self, owner: HWND) -> HRESULT {
        unsafe { (self.dialog().show)(self.0, owner) }
    }
}
impl Drop for Interface {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let table = &**(self.0 as *const *const UnknownVtable);
                (table.release)(self.0);
            }
        }
    }
}

struct TaskString(*mut u16);
impl Drop for TaskString {
    fn drop(&mut self) {
        unsafe {
            CoTaskMemFree(self.0.cast());
        }
    }
}

fn check(status: HRESULT) -> Result<()> {
    if status < 0 {
        Err(FastPadError::Win32(status as u32))
    } else {
        Ok(())
    }
}

pub fn show_open_dialog(owner: HWND) -> Result<Option<PathBuf>> {
    let _apartment = ComApartment::initialize()?;
    let mut dialog = Interface(std::ptr::null_mut());
    check(unsafe {
        CoCreateInstance(
            &FileOpenDialog,
            std::ptr::null_mut(),
            CLSCTX_INPROC_SERVER,
            &IID_IFILE_OPEN_DIALOG,
            &mut dialog.0,
        )
    })?;
    dialog.require()?;
    let mut options = 0;
    check(unsafe { (dialog.dialog().get_options)(dialog.0, &mut options) })?;
    check(unsafe {
        (dialog.dialog().set_options)(dialog.0, options | FOS_FORCEFILESYSTEM | FOS_FILEMUSTEXIST)
    })?;
    let status = dialog.show(owner);
    if status == CANCELLED {
        return Ok(None);
    }
    check(status)?;
    let mut item = Interface(std::ptr::null_mut());
    check(unsafe { (dialog.dialog().get_result)(dialog.0, &mut item.0) })?;
    item.require()?;
    let mut text = TaskString(std::ptr::null_mut());
    check(unsafe { (item.shell_item().get_display_name)(item.0, SIGDN_FILESYSPATH, &mut text.0) })?;
    if text.0.is_null() {
        return Err(FastPadError::Invariant("shell item returned a null path"));
    }
    let mut len = 0;
    // The shell owns this terminated UTF-16 allocation until TaskString frees it.
    unsafe {
        while *text.0.add(len) != 0 {
            len += 1;
        }
    }
    let path = PathBuf::from(OsString::from_wide(unsafe {
        std::slice::from_raw_parts(text.0, len)
    }));
    Ok(Some(path))
}
