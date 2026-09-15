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
    FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, FileOpenDialog, FileSaveDialog, SIGDN_FILESYSPATH,
};
use windows_sys::core::{GUID, HRESULT};

const IID_IFILE_OPEN_DIALOG: GUID = GUID::from_u128(0xd57c7288_d4ad_4768_be02_9d969532d960);
const IID_IFILE_SAVE_DIALOG: GUID = GUID::from_u128(0x84bccd23_5fde_4cdb_aea4_af64b83d78ab);
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
    // Callable unconditionally: production show_save_dialog always prefills a suggested file name,
    // and the #[cfg(test)] Open dialog test seam calls it only under `#[cfg(test)]` at the call site.
    set_file_name: unsafe extern "system" fn(*mut c_void, *const u16) -> HRESULT,
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
        #[cfg(test)]
        note_event(DialogEvent::ComInitialized);
        // S_FALSE is success too and also requires a balancing CoUninitialize.
        Ok(Self)
    }
}
impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
        #[cfg(test)]
        note_event(DialogEvent::ComUninitialized);
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
            #[cfg(test)]
            note_event(DialogEvent::InterfaceReleased);
        }
    }
}

struct TaskString(*mut u16);
impl Drop for TaskString {
    fn drop(&mut self) {
        unsafe {
            CoTaskMemFree(self.0.cast());
        }
        #[cfg(test)]
        note_event(DialogEvent::PathFreed);
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
    #[cfg(test)]
    if let Some(path) = NEXT_FILE_NAME.with(|value| value.borrow_mut().take()) {
        use std::os::windows::ffi::OsStrExt;
        let name = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        check(unsafe { (dialog.dialog().set_file_name)(dialog.0, name.as_ptr()) })?;
    }
    let status = dialog.show(owner);
    #[cfg(test)]
    note_event(DialogEvent::ShowReturned);
    if status == CANCELLED {
        return Ok(None);
    }
    check(status)?;
    let mut item = Interface(std::ptr::null_mut());
    check(unsafe { (dialog.dialog().get_result)(dialog.0, &mut item.0) })?;
    item.require()?;
    #[cfg(test)]
    note_event(DialogEvent::ResultRetrieved);
    let mut text = TaskString(std::ptr::null_mut());
    check(unsafe { (item.shell_item().get_display_name)(item.0, SIGDN_FILESYSPATH, &mut text.0) })?;
    #[cfg(test)]
    note_event(DialogEvent::DisplayNameRetrieved);
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

pub fn show_save_dialog(owner: HWND, suggested_name: &str) -> Result<Option<PathBuf>> {
    let _apartment = ComApartment::initialize()?;
    let mut dialog = Interface(std::ptr::null_mut());
    check(unsafe {
        CoCreateInstance(
            &FileSaveDialog,
            std::ptr::null_mut(),
            CLSCTX_INPROC_SERVER,
            &IID_IFILE_SAVE_DIALOG,
            &mut dialog.0,
        )
    })?;
    dialog.require()?;
    let mut options = 0;
    check(unsafe { (dialog.dialog().get_options)(dialog.0, &mut options) })?;
    // IFileSaveDialog defaults to FOS_OVERWRITEPROMPT (confirmed via GetOptions: 0x880a on this
    // host); that default is left alone deliberately. FastPad's own atomic replace makes an
    // overwrite safe from partial-content corruption, but that is a different property from user
    // *consent* to overwrite: a user picking an existing, unrelated file in Save As should still
    // see the shell's standard "this file already exists, replace it?" confirmation before FastPad
    // replaces it. (A colliding path already open in another FastPad tab is rejected separately by
    // `Tabs::set_active_path`, before that file is ever touched.)
    check(unsafe { (dialog.dialog().set_options)(dialog.0, options | FOS_FORCEFILESYSTEM) })?;
    let name = suggested_name
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    check(unsafe { (dialog.dialog().set_file_name)(dialog.0, name.as_ptr()) })?;
    let status = dialog.show(owner);
    #[cfg(test)]
    note_event(DialogEvent::ShowReturned);
    if status == CANCELLED {
        return Ok(None);
    }
    check(status)?;
    let mut item = Interface(std::ptr::null_mut());
    check(unsafe { (dialog.dialog().get_result)(dialog.0, &mut item.0) })?;
    item.require()?;
    #[cfg(test)]
    note_event(DialogEvent::ResultRetrieved);
    let mut text = TaskString(std::ptr::null_mut());
    check(unsafe { (item.shell_item().get_display_name)(item.0, SIGDN_FILESYSPATH, &mut text.0) })?;
    #[cfg(test)]
    note_event(DialogEvent::DisplayNameRetrieved);
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

#[cfg(test)]
thread_local! {
    static NEXT_FILE_NAME: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
    static EVENTS: std::cell::RefCell<Vec<DialogEvent>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum DialogEvent {
    ComInitialized,
    ShowReturned,
    ResultRetrieved,
    DisplayNameRetrieved,
    PathFreed,
    InterfaceReleased,
    ComUninitialized,
}

#[cfg(test)]
fn note_event(event: DialogEvent) {
    EVENTS.with(|events| events.borrow_mut().push(event));
}

#[cfg(test)]
#[allow(
    dead_code,
    reason = "consumed by the source-linked open_file integration target"
)]
pub(crate) fn set_next_open_dialog_filename(path: PathBuf) {
    NEXT_FILE_NAME.with(|value| *value.borrow_mut() = Some(path));
    EVENTS.with(|events| events.borrow_mut().clear());
}

#[cfg(test)]
#[allow(
    dead_code,
    reason = "consumed by the source-linked open_file and save_file integration targets"
)]
pub(crate) fn take_dialog_events() -> Vec<DialogEvent> {
    EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()))
}
