use crate::editor::scintilla_constants::{
    SC_CP_UTF8, SCI_ADDREFDOCUMENT, SCI_BEGINUNDOACTION, SCI_CREATEDOCUMENT, SCI_ENDUNDOACTION,
    SCI_GETDIRECTFUNCTION, SCI_GETDIRECTPOINTER, SCI_GETDOCPOINTER, SCI_GETTEXT, SCI_GETTEXTLENGTH,
    SCI_RELEASEDOCUMENT, SCI_SETCODEPAGE, SCI_SETDOCPOINTER, SCI_SETSAVEPOINT, SCI_SETTEXT,
};
use crate::{FastPadError, Result};
use std::ffi::CString;
use windows_sys::Win32::Foundation::HWND;

#[cfg(windows)]
use crate::platform::{last_error, wide_null};
#[cfg(windows)]
use std::mem::transmute;
#[cfg(windows)]
use windows_sys::Win32::Foundation::{LPARAM, RECT, WPARAM};
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, GetClientRect, SendMessageW, WS_CHILD, WS_TABSTOP, WS_VISIBLE,
};

pub type SciFnDirect = unsafe extern "C" fn(isize, u32, usize, isize) -> isize;

#[derive(Debug)]
pub struct Editor {
    hwnd: HWND,
    direct_fn: SciFnDirect,
    direct_ptr: isize,
}

#[derive(Debug, Eq, PartialEq)]
pub struct EditorDocument {
    raw: isize,
    editor: HWND,
}

impl Editor {
    #[cfg(windows)]
    pub fn create(parent: HWND) -> Result<Self> {
        let hwnd = unsafe {
            let mut rect = RECT::default();
            if GetClientRect(parent, &mut rect) == 0 {
                return Err(last_error());
            }

            let class_name = wide_null("Scintilla");
            let width = (rect.right - rect.left).max(1);
            let height = (rect.bottom - rect.top).max(1);
            CreateWindowExW(
                0,
                class_name.as_ptr(),
                std::ptr::null(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                0,
                0,
                width,
                height,
                parent,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        require_hwnd(hwnd)?;

        let direct_fn_raw = unsafe { SendMessageW(hwnd, SCI_GETDIRECTFUNCTION, 0, 0) };
        if direct_fn_raw == 0 {
            return Err(FastPadError::Invariant(
                "Scintilla did not provide a direct function",
            ));
        }

        let direct_ptr = unsafe { SendMessageW(hwnd, SCI_GETDIRECTPOINTER, 0, 0) };
        if direct_ptr == 0 {
            return Err(FastPadError::Invariant(
                "Scintilla did not provide a direct pointer",
            ));
        }

        let editor = Self {
            hwnd,
            direct_fn: unsafe { transmute::<isize, SciFnDirect>(direct_fn_raw) },
            direct_ptr,
        };
        editor.send_direct(SCI_SETCODEPAGE, SC_CP_UTF8 as usize, 0);
        Ok(editor)
    }

    #[cfg(not(windows))]
    pub fn create(_parent: HWND) -> Result<Self> {
        Err(FastPadError::Invariant(
            "Scintilla editor is only supported on Windows",
        ))
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    #[cfg(windows)]
    pub fn set_text(&self, text: &str) -> Result<()> {
        let text = CString::new(text)
            .map_err(|_| FastPadError::Invariant("Scintilla text may not contain NUL bytes"))?;
        self.send_direct(SCI_SETTEXT, 0, text.as_ptr() as isize);
        Ok(())
    }

    #[cfg(not(windows))]
    pub fn set_text(&self, _text: &str) -> Result<()> {
        Err(FastPadError::Invariant(
            "Scintilla editor is only supported on Windows",
        ))
    }

    #[cfg(windows)]
    pub fn text(&self) -> Result<String> {
        let length = self.send_direct(SCI_GETTEXTLENGTH, 0, 0);
        if length < 0 {
            return Err(FastPadError::Invariant(
                "Scintilla returned a negative text length",
            ));
        }

        let mut bytes = vec![0_u8; length as usize + 1];
        self.send_direct(SCI_GETTEXT, bytes.len(), bytes.as_mut_ptr() as isize);
        bytes.truncate(length as usize);
        String::from_utf8(bytes)
            .map_err(|_| FastPadError::Invariant("Scintilla returned invalid UTF-8"))
    }

    #[cfg(not(windows))]
    pub fn text(&self) -> Result<String> {
        Err(FastPadError::Invariant(
            "Scintilla editor is only supported on Windows",
        ))
    }

    #[cfg(windows)]
    pub fn create_document(&self) -> Result<EditorDocument> {
        let raw = self.send_direct(SCI_CREATEDOCUMENT, 0, 0);
        if raw == 0 {
            return Err(FastPadError::Invariant(
                "Scintilla did not create a document",
            ));
        }
        Ok(EditorDocument {
            raw,
            editor: self.hwnd,
        })
    }

    #[cfg(not(windows))]
    pub fn create_document(&self) -> Result<EditorDocument> {
        Err(FastPadError::Invariant(
            "Scintilla editor is only supported on Windows",
        ))
    }

    #[cfg(windows)]
    pub fn current_document(&self) -> Result<EditorDocument> {
        let raw = self.send_direct(SCI_GETDOCPOINTER, 0, 0);
        if raw == 0 {
            return Err(FastPadError::Invariant(
                "Scintilla did not return the current document",
            ));
        }
        self.send_direct(SCI_ADDREFDOCUMENT, 0, raw);
        Ok(EditorDocument {
            raw,
            editor: self.hwnd,
        })
    }

    #[cfg(not(windows))]
    pub fn current_document(&self) -> Result<EditorDocument> {
        Err(FastPadError::Invariant(
            "Scintilla editor is only supported on Windows",
        ))
    }

    #[cfg(windows)]
    pub fn use_document(&self, document: &EditorDocument) -> Result<()> {
        if document.editor != self.hwnd {
            return Err(FastPadError::Invariant(
                "Scintilla document belongs to a different editor",
            ));
        }
        self.send_direct(SCI_SETDOCPOINTER, 0, document.raw);
        Ok(())
    }

    #[cfg(not(windows))]
    pub fn use_document(&self, _document: &EditorDocument) -> Result<()> {
        Err(FastPadError::Invariant(
            "Scintilla editor is only supported on Windows",
        ))
    }

    pub fn set_save_point(&self) {
        #[cfg(windows)]
        self.send_direct(SCI_SETSAVEPOINT, 0, 0);
    }

    pub fn begin_undo_action(&self) {
        #[cfg(windows)]
        self.send_direct(SCI_BEGINUNDOACTION, 0, 0);
    }

    pub fn end_undo_action(&self) {
        #[cfg(windows)]
        self.send_direct(SCI_ENDUNDOACTION, 0, 0);
    }

    #[cfg(windows)]
    fn send_direct(&self, message: u32, wparam: usize, lparam: isize) -> isize {
        unsafe { (self.direct_fn)(self.direct_ptr, message, wparam, lparam) }
    }
}

impl Clone for EditorDocument {
    #[cfg(windows)]
    fn clone(&self) -> Self {
        if should_skip_test_fixture_refcount(self.raw) {
            return Self {
                raw: self.raw,
                editor: self.editor,
            };
        }

        unsafe {
            SendMessageW(
                self.editor,
                SCI_ADDREFDOCUMENT,
                0 as WPARAM,
                self.raw as LPARAM,
            );
        }
        Self {
            raw: self.raw,
            editor: self.editor,
        }
    }

    #[cfg(not(windows))]
    fn clone(&self) -> Self {
        Self {
            raw: self.raw,
            editor: self.editor,
        }
    }
}

impl Drop for EditorDocument {
    #[cfg(windows)]
    fn drop(&mut self) {
        if should_skip_test_fixture_refcount(self.raw) {
            return;
        }

        unsafe {
            SendMessageW(
                self.editor,
                SCI_RELEASEDOCUMENT,
                0 as WPARAM,
                self.raw as LPARAM,
            );
        }
    }

    #[cfg(not(windows))]
    fn drop(&mut self) {}
}

impl EditorDocument {
    #[cfg(test)]
    pub fn test_fixture() -> Self {
        Self {
            raw: 0,
            editor: std::ptr::null_mut(),
        }
    }
}

#[cfg(test)]
fn should_skip_test_fixture_refcount(raw: isize) -> bool {
    raw == 0
}

#[cfg(not(test))]
fn should_skip_test_fixture_refcount(_raw: isize) -> bool {
    false
}

#[cfg(windows)]
fn require_hwnd(raw: HWND) -> Result<HWND> {
    if raw.is_null() {
        Err(last_error())
    } else {
        Ok(raw)
    }
}

#[cfg(test)]
mod tests {
    use super::EditorDocument;

    #[test]
    fn test_fixture_clone_and_drop_are_inert() {
        // Break caught: test-only fake document handles trying to refcount through a null HWND.
        let fixture = EditorDocument::test_fixture();
        let clone = fixture.clone();
        assert_eq!(clone, EditorDocument::test_fixture());
        drop(clone);
        drop(fixture);
    }
}
