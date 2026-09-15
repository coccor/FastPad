use crate::editor::scintilla_constants::{
    SC_CP_UTF8, SCI_ADDREFDOCUMENT, SCI_BEGINUNDOACTION, SCI_CANREDO, SCI_CANUNDO, SCI_COPY,
    SCI_CREATEDOCUMENT, SCI_CUT, SCI_EMPTYUNDOBUFFER, SCI_ENDUNDOACTION, SCI_GETDIRECTFUNCTION,
    SCI_GETDIRECTPOINTER, SCI_GETDOCPOINTER, SCI_GETLENGTH, SCI_GETSELECTIONEND,
    SCI_GETSELECTIONSTART, SCI_GETSELTEXT, SCI_GETTEXT, SCI_GETTEXTLENGTH, SCI_PASTE, SCI_REDO,
    SCI_RELEASEDOCUMENT, SCI_REPLACETARGET, SCI_SCROLLCARET, SCI_SEARCHINTARGET, SCI_SETCODEPAGE,
    SCI_SETDOCPOINTER, SCI_SETILEXER, SCI_SETSAVEPOINT, SCI_SETSEARCHFLAGS, SCI_SETSEL,
    SCI_SETTARGETRANGE, SCI_SETTEXT, SCI_SETUNDOCOLLECTION, SCI_STYLECLEARALL, SCI_STYLESETBACK,
    SCI_STYLESETBOLD, SCI_STYLESETFONT, SCI_STYLESETFORE, SCI_UNDO,
};
#[cfg(windows)]
use crate::editor::scintilla_constants::{
    SC_WRAP_NONE, SC_WRAP_WORD, SCI_SETCARETFORE, SCI_SETTABWIDTH, SCI_SETWRAPMODE,
    SCI_STYLESETSIZEFRACTIONAL, STYLE_DEFAULT,
};
use crate::{FastPadError, Result};
use std::ffi::CString;
use std::ops::Range;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use windows_sys::Win32::Foundation::HWND;

#[cfg(windows)]
use crate::platform::{last_error, wide_null};
#[cfg(windows)]
use std::mem::transmute;
#[cfg(windows)]
use windows_sys::Win32::Foundation::{LPARAM, RECT, WPARAM};
#[cfg(windows)]
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, GetClientRect, SendMessageW, WM_NCDESTROY, WS_CHILD,
    WS_TABSTOP, WS_VISIBLE,
};

pub type SciFnDirect = unsafe extern "C" fn(isize, u32, usize, isize) -> isize;

const ENDPOINT_DESTROYED: &str = "Scintilla editor endpoint is no longer alive";
#[cfg(windows)]
const EDITOR_ENDPOINT_SUBCLASS_ID: usize = 0x4650_4544;

#[derive(Debug)]
pub struct Editor {
    endpoint: Rc<EditorEndpoint>,
}

impl Clone for Editor {
    fn clone(&self) -> Self {
        Self {
            endpoint: Rc::clone(&self.endpoint),
        }
    }
}

#[derive(Debug)]
pub struct EditorDocument {
    raw: isize,
    endpoint: Rc<EditorEndpoint>,
}

#[derive(Debug)]
struct EditorEndpoint {
    hwnd: HWND,
    direct_fn: SciFnDirect,
    direct_ptr: isize,
    destroyed: AtomicBool,
    destroy_window_on_drop: bool,
    #[cfg(test)]
    release_counter: Option<std::sync::Arc<std::sync::atomic::AtomicUsize>>,
}

impl Editor {
    #[cfg(windows)]
    pub fn create(parent: HWND) -> Result<Self> {
        let hwnd = create_scintilla_child(parent)?;
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

        let endpoint = Rc::new(EditorEndpoint::new(
            hwnd,
            unsafe { transmute::<isize, SciFnDirect>(direct_fn_raw) },
            direct_ptr,
            true,
        ));
        endpoint.install_lifecycle_guard()?;

        let editor = Self { endpoint };
        editor
            .endpoint
            .send_direct_checked(SCI_SETCODEPAGE, SC_CP_UTF8 as usize, 0)?;
        Ok(editor)
    }

    #[cfg(not(windows))]
    pub fn create(_parent: HWND) -> Result<Self> {
        Err(FastPadError::Invariant(
            "Scintilla editor is only supported on Windows",
        ))
    }

    pub fn hwnd(&self) -> HWND {
        self.endpoint.hwnd
    }

    #[cfg(windows)]
    pub fn set_text(&self, text: &str) -> Result<()> {
        let text = CString::new(text)
            .map_err(|_| FastPadError::Invariant("Scintilla text may not contain NUL bytes"))?;
        self.endpoint
            .send_direct_checked(SCI_SETTEXT, 0, text.as_ptr() as isize)?;
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
        let length = self.endpoint.send_direct_checked(SCI_GETTEXTLENGTH, 0, 0)?;
        if length < 0 {
            return Err(FastPadError::Invariant(
                "Scintilla returned a negative text length",
            ));
        }

        let mut bytes = vec![0_u8; length as usize + 1];
        self.endpoint
            .send_direct_checked(SCI_GETTEXT, bytes.len(), bytes.as_mut_ptr() as isize)?;
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
        let raw = self
            .endpoint
            .send_direct_checked(SCI_CREATEDOCUMENT, 0, 0)?;
        if raw == 0 {
            return Err(FastPadError::Invariant(
                "Scintilla did not create a document",
            ));
        }
        Ok(EditorDocument {
            raw,
            endpoint: Rc::clone(&self.endpoint),
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
        let raw = self.endpoint.send_direct_checked(SCI_GETDOCPOINTER, 0, 0)?;
        if raw == 0 {
            return Err(FastPadError::Invariant(
                "Scintilla did not return the current document",
            ));
        }
        self.endpoint.retain_document(raw);
        Ok(EditorDocument {
            raw,
            endpoint: Rc::clone(&self.endpoint),
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
        if !Rc::ptr_eq(&self.endpoint, &document.endpoint) {
            return Err(FastPadError::Invariant(
                "Scintilla document belongs to a different editor",
            ));
        }
        self.endpoint
            .send_direct_checked(SCI_SETDOCPOINTER, 0, document.raw)?;
        Ok(())
    }

    #[cfg(not(windows))]
    pub fn use_document(&self, _document: &EditorDocument) -> Result<()> {
        Err(FastPadError::Invariant(
            "Scintilla editor is only supported on Windows",
        ))
    }

    #[cfg(windows)]
    pub fn length(&self) -> Result<usize> {
        let length = self.endpoint.send_direct_checked(SCI_GETLENGTH, 0, 0)?;
        if length < 0 {
            return Err(FastPadError::Invariant(
                "Scintilla returned a negative document length",
            ));
        }
        Ok(length as usize)
    }

    #[cfg(not(windows))]
    pub fn length(&self) -> Result<usize> {
        Err(FastPadError::Invariant(
            "Scintilla editor is only supported on Windows",
        ))
    }

    /// Scrolls so the caret (the end of the current selection) is visible, without changing it.
    pub fn scroll_caret_into_view(&self) {
        let _ = self.endpoint.send_direct_if_alive(SCI_SCROLLCARET, 0, 0);
    }

    #[cfg(windows)]
    pub fn search_in_target(
        &self,
        needle: &str,
        range: Range<usize>,
        search_flags: u32,
    ) -> Result<Option<Range<usize>>> {
        let needle = CString::new(needle).map_err(|_| {
            FastPadError::Invariant("Scintilla search text may not contain NUL bytes")
        })?;
        self.endpoint
            .send_direct_checked(SCI_SETTARGETRANGE, range.start, range.end as isize)?;
        self.endpoint
            .send_direct_checked(SCI_SETSEARCHFLAGS, search_flags as usize, 0)?;
        let found = self.endpoint.send_direct_checked(
            SCI_SEARCHINTARGET,
            needle.as_bytes().len(),
            needle.as_ptr() as isize,
        )?;
        if found < 0 {
            return Ok(None);
        }

        let start = found as usize;
        Ok(Some(start..start + needle.as_bytes().len()))
    }

    #[cfg(not(windows))]
    pub fn search_in_target(
        &self,
        _needle: &str,
        _range: Range<usize>,
        _search_flags: u32,
    ) -> Result<Option<Range<usize>>> {
        Err(FastPadError::Invariant(
            "Scintilla editor is only supported on Windows",
        ))
    }

    #[cfg(windows)]
    pub fn replace_target(&self, range: Range<usize>, replacement: &str) -> Result<Range<usize>> {
        self.endpoint
            .send_direct_checked(SCI_SETTARGETRANGE, range.start, range.end as isize)?;
        let bytes = replacement.as_bytes();
        self.endpoint.send_direct_checked(
            SCI_REPLACETARGET,
            bytes.len(),
            bytes.as_ptr() as isize,
        )?;
        Ok(range.start..range.start + bytes.len())
    }

    #[cfg(not(windows))]
    pub fn replace_target(&self, _range: Range<usize>, _replacement: &str) -> Result<Range<usize>> {
        Err(FastPadError::Invariant(
            "Scintilla editor is only supported on Windows",
        ))
    }

    /// Replaces every occurrence of `query` with `replacement`, as one undo action. Searches
    /// incrementally via `search_in_target`/`replace_target`; never retrieves the full document.
    #[cfg(windows)]
    pub fn replace_all(&self, query: &str, replacement: &str, search_flags: u32) -> Result<usize> {
        if query.is_empty() {
            return Ok(0);
        }
        self.begin_undo_action();
        let result = (|| {
            let mut count = 0usize;
            let mut position = 0usize;
            loop {
                let length = self.length()?;
                if position > length {
                    break;
                }
                let Some(found) = self.search_in_target(query, position..length, search_flags)?
                else {
                    break;
                };
                let replaced = self.replace_target(found, replacement)?;
                count += 1;
                position = replaced.end;
            }
            Ok(count)
        })();
        self.end_undo_action();
        result
    }

    #[cfg(not(windows))]
    pub fn replace_all(
        &self,
        _query: &str,
        _replacement: &str,
        _search_flags: u32,
    ) -> Result<usize> {
        Err(FastPadError::Invariant(
            "Scintilla editor is only supported on Windows",
        ))
    }

    #[cfg(windows)]
    pub fn undo(&self) -> Result<()> {
        self.endpoint.send_direct_checked(SCI_UNDO, 0, 0)?;
        Ok(())
    }

    #[cfg(not(windows))]
    pub fn undo(&self) -> Result<()> {
        Err(FastPadError::Invariant(
            "Scintilla editor is only supported on Windows",
        ))
    }

    #[cfg(windows)]
    pub fn redo(&self) -> Result<()> {
        self.endpoint.send_direct_checked(SCI_REDO, 0, 0)?;
        Ok(())
    }

    #[cfg(not(windows))]
    pub fn redo(&self) -> Result<()> {
        Err(FastPadError::Invariant(
            "Scintilla editor is only supported on Windows",
        ))
    }

    #[cfg(windows)]
    pub fn can_undo(&self) -> Result<bool> {
        Ok(self.endpoint.send_direct_checked(SCI_CANUNDO, 0, 0)? != 0)
    }

    #[cfg(not(windows))]
    pub fn can_undo(&self) -> Result<bool> {
        Err(FastPadError::Invariant(
            "Scintilla editor is only supported on Windows",
        ))
    }

    #[cfg(windows)]
    pub fn can_redo(&self) -> Result<bool> {
        Ok(self.endpoint.send_direct_checked(SCI_CANREDO, 0, 0)? != 0)
    }

    #[cfg(not(windows))]
    pub fn can_redo(&self) -> Result<bool> {
        Err(FastPadError::Invariant(
            "Scintilla editor is only supported on Windows",
        ))
    }

    #[cfg(windows)]
    pub fn cut(&self) -> Result<()> {
        self.endpoint.send_direct_checked(SCI_CUT, 0, 0)?;
        Ok(())
    }

    #[cfg(not(windows))]
    pub fn cut(&self) -> Result<()> {
        Err(FastPadError::Invariant(
            "Scintilla editor is only supported on Windows",
        ))
    }

    #[cfg(windows)]
    pub fn copy(&self) -> Result<()> {
        self.endpoint.send_direct_checked(SCI_COPY, 0, 0)?;
        Ok(())
    }

    #[cfg(not(windows))]
    pub fn copy(&self) -> Result<()> {
        Err(FastPadError::Invariant(
            "Scintilla editor is only supported on Windows",
        ))
    }

    #[cfg(windows)]
    pub fn paste(&self) -> Result<()> {
        self.endpoint.send_direct_checked(SCI_PASTE, 0, 0)?;
        Ok(())
    }

    #[cfg(not(windows))]
    pub fn paste(&self) -> Result<()> {
        Err(FastPadError::Invariant(
            "Scintilla editor is only supported on Windows",
        ))
    }

    #[cfg(windows)]
    pub fn selection(&self) -> Result<Range<usize>> {
        let start = self
            .endpoint
            .send_direct_checked(SCI_GETSELECTIONSTART, 0, 0)?;
        let end = self
            .endpoint
            .send_direct_checked(SCI_GETSELECTIONEND, 0, 0)?;
        if start < 0 || end < start {
            return Err(FastPadError::Invariant(
                "Scintilla returned an invalid selection range",
            ));
        }
        Ok(start as usize..end as usize)
    }

    #[cfg(not(windows))]
    pub fn selection(&self) -> Result<Range<usize>> {
        Err(FastPadError::Invariant(
            "Scintilla editor is only supported on Windows",
        ))
    }

    #[cfg(windows)]
    pub fn set_selection(&self, range: Range<usize>) -> Result<()> {
        self.endpoint
            .send_direct_checked(SCI_SETSEL, range.start, range.end as isize)?;
        Ok(())
    }

    #[cfg(not(windows))]
    pub fn set_selection(&self, _range: Range<usize>) -> Result<()> {
        Err(FastPadError::Invariant(
            "Scintilla editor is only supported on Windows",
        ))
    }

    /// The text of the current selection, retrieved directly via `SCI_GETSELTEXT` rather than by
    /// slicing a full-document read.
    #[cfg(windows)]
    pub fn selected_text(&self) -> Result<String> {
        let length = self.endpoint.send_direct_checked(SCI_GETSELTEXT, 0, 0)?;
        if length < 0 {
            return Err(FastPadError::Invariant(
                "Scintilla returned a negative selection length",
            ));
        }
        let mut bytes = vec![0_u8; length as usize + 1];
        self.endpoint
            .send_direct_checked(SCI_GETSELTEXT, 0, bytes.as_mut_ptr() as isize)?;
        bytes.truncate(length as usize);
        String::from_utf8(bytes)
            .map_err(|_| FastPadError::Invariant("Scintilla returned invalid UTF-8"))
    }

    #[cfg(not(windows))]
    pub fn selected_text(&self) -> Result<String> {
        Err(FastPadError::Invariant(
            "Scintilla editor is only supported on Windows",
        ))
    }

    /// Installs `lexer` (an opaque `ILexer5*` from Lexilla's `CreateLexer`, or `0` for Scintilla's
    /// built-in null lexer) via `SCI_SETILEXER`. Scintilla takes ownership of a non-null pointer
    /// and releases it itself when replaced or the document is destroyed; this method never calls
    /// `Release()` and never interprets the pointer beyond forwarding it.
    #[cfg(windows)]
    pub fn set_lexer(&self, lexer: isize) -> Result<()> {
        self.endpoint.send_direct_checked(SCI_SETILEXER, 0, lexer)?;
        Ok(())
    }

    #[cfg(not(windows))]
    pub fn set_lexer(&self, _lexer: isize) -> Result<()> {
        Err(FastPadError::Invariant(
            "Scintilla editor is only supported on Windows",
        ))
    }

    /// Resets every style number to Scintilla's current default look via `SCI_STYLECLEARALL`, so a
    /// previous language's style overrides cannot bleed into the next one before `set_style`
    /// reapplies the styles relevant to the newly installed lexer.
    #[cfg(windows)]
    pub fn clear_all_styles(&self) -> Result<()> {
        self.endpoint.send_direct_checked(SCI_STYLECLEARALL, 0, 0)?;
        Ok(())
    }

    #[cfg(not(windows))]
    pub fn clear_all_styles(&self) -> Result<()> {
        Err(FastPadError::Invariant(
            "Scintilla editor is only supported on Windows",
        ))
    }

    /// Applies user view settings to every style up to `STYLE_DEFAULT` without touching text,
    /// selection, or colors.
    #[cfg(windows)]
    pub fn apply_view_settings(
        &self,
        face: &str,
        size_points: u16,
        tab_width: u8,
        word_wrap: bool,
    ) -> Result<()> {
        let face = CString::new(face).map_err(|_| {
            FastPadError::Invariant("Scintilla font face may not contain NUL bytes")
        })?;
        for style in 0..=STYLE_DEFAULT as usize {
            self.endpoint
                .send_direct_checked(SCI_STYLESETFONT, style, face.as_ptr() as isize)?;
            self.endpoint.send_direct_checked(
                SCI_STYLESETSIZEFRACTIONAL,
                style,
                size_points as isize * 100,
            )?;
        }
        self.endpoint
            .send_direct_checked(SCI_SETTABWIDTH, usize::from(tab_width), 0)?;
        let wrap = if word_wrap {
            SC_WRAP_WORD
        } else {
            SC_WRAP_NONE
        };
        self.endpoint
            .send_direct_checked(SCI_SETWRAPMODE, wrap as usize, 0)?;
        Ok(())
    }

    #[cfg(not(windows))]
    pub fn apply_view_settings(
        &self,
        _face: &str,
        _size_points: u16,
        _tab_width: u8,
        _word_wrap: bool,
    ) -> Result<()> {
        Err(FastPadError::Invariant(
            "Scintilla editor is only supported on Windows",
        ))
    }

    /// Sets plain-text foreground/background on every style up to `STYLE_DEFAULT`, plus the caret.
    #[cfg(windows)]
    pub fn set_base_colors(&self, foreground: u32, background: u32) -> Result<()> {
        for style in 0..=STYLE_DEFAULT as usize {
            self.endpoint
                .send_direct_checked(SCI_STYLESETFORE, style, foreground as isize)?;
            self.endpoint
                .send_direct_checked(SCI_STYLESETBACK, style, background as isize)?;
        }
        self.endpoint
            .send_direct_checked(SCI_SETCARETFORE, foreground as usize, 0)?;
        Ok(())
    }

    #[cfg(not(windows))]
    pub fn set_base_colors(&self, _foreground: u32, _background: u32) -> Result<()> {
        Err(FastPadError::Invariant(
            "Scintilla editor is only supported on Windows",
        ))
    }

    /// Sets one lexer style's foreground, background, bold flag, and font face. `bold` is always
    /// sent explicitly (both true and false) so a previous language's bold flag cannot leak
    /// through onto this style number.
    #[cfg(windows)]
    pub fn set_style(
        &self,
        style: u32,
        foreground: u32,
        background: u32,
        bold: bool,
        face: &str,
    ) -> Result<()> {
        let face = CString::new(face).map_err(|_| {
            FastPadError::Invariant("Scintilla font face may not contain NUL bytes")
        })?;
        self.endpoint
            .send_direct_checked(SCI_STYLESETFORE, style as usize, foreground as isize)?;
        self.endpoint
            .send_direct_checked(SCI_STYLESETBACK, style as usize, background as isize)?;
        self.endpoint
            .send_direct_checked(SCI_STYLESETBOLD, style as usize, isize::from(bold))?;
        self.endpoint.send_direct_checked(
            SCI_STYLESETFONT,
            style as usize,
            face.as_ptr() as isize,
        )?;
        Ok(())
    }

    #[cfg(not(windows))]
    pub fn set_style(
        &self,
        _style: u32,
        _foreground: u32,
        _background: u32,
        _bold: bool,
        _face: &str,
    ) -> Result<()> {
        Err(FastPadError::Invariant(
            "Scintilla editor is only supported on Windows",
        ))
    }

    pub fn set_save_point(&self) {
        let _ = self.endpoint.send_direct_if_alive(SCI_SETSAVEPOINT, 0, 0);
    }

    pub(crate) fn populate_clean(&self, text: &str) -> Result<()> {
        self.endpoint
            .send_direct_checked(SCI_SETUNDOCOLLECTION, 0, 0)?;
        let result = self.set_text(text);
        let clear = self.endpoint.send_direct_checked(SCI_EMPTYUNDOBUFFER, 0, 0);
        let enable = self
            .endpoint
            .send_direct_checked(SCI_SETUNDOCOLLECTION, 1, 0);
        result?;
        clear?;
        enable?;
        self.endpoint.send_direct_checked(SCI_SETSAVEPOINT, 0, 0)?;
        Ok(())
    }

    pub fn begin_undo_action(&self) {
        let _ = self
            .endpoint
            .send_direct_if_alive(SCI_BEGINUNDOACTION, 0, 0);
    }

    pub fn end_undo_action(&self) {
        let _ = self.endpoint.send_direct_if_alive(SCI_ENDUNDOACTION, 0, 0);
    }

    #[cfg(test)]
    pub(crate) fn test_fixture(direct_fn: SciFnDirect, direct_ptr: isize) -> Self {
        Self {
            endpoint: Rc::new(EditorEndpoint::new(
                std::ptr::null_mut(),
                direct_fn,
                direct_ptr,
                false,
            )),
        }
    }
}

impl Clone for EditorDocument {
    fn clone(&self) -> Self {
        self.endpoint.retain_document(self.raw);
        Self {
            raw: self.raw,
            endpoint: Rc::clone(&self.endpoint),
        }
    }
}

impl Drop for EditorDocument {
    fn drop(&mut self) {
        self.endpoint.release_document(self.raw);
    }
}

impl EditorDocument {
    #[cfg(test)]
    pub fn test_fixture() -> Self {
        Self {
            raw: 0,
            endpoint: Rc::new(EditorEndpoint::new(
                std::ptr::null_mut(),
                inert_direct_call,
                0,
                false,
            )),
        }
    }

    #[cfg(test)]
    pub fn test_fixture_with_release_counter(
        releases: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ) -> Self {
        Self {
            raw: 1,
            endpoint: Rc::new(EditorEndpoint {
                hwnd: std::ptr::null_mut(),
                direct_fn: inert_direct_call,
                direct_ptr: 0,
                destroyed: AtomicBool::new(false),
                destroy_window_on_drop: false,
                release_counter: Some(releases),
            }),
        }
    }

    #[cfg(test)]
    fn test_fixture_with_raw(raw: isize, editor: &Editor) -> Self {
        Self {
            raw,
            endpoint: Rc::clone(&editor.endpoint),
        }
    }

    #[cfg(test)]
    fn raw(&self) -> isize {
        self.raw
    }
}

impl EditorEndpoint {
    fn new(
        hwnd: HWND,
        direct_fn: SciFnDirect,
        direct_ptr: isize,
        destroy_window_on_drop: bool,
    ) -> Self {
        Self {
            hwnd,
            direct_fn,
            direct_ptr,
            destroyed: AtomicBool::new(false),
            destroy_window_on_drop,
            #[cfg(test)]
            release_counter: None,
        }
    }

    #[cfg(windows)]
    fn install_lifecycle_guard(self: &Rc<Self>) -> Result<()> {
        let installed = unsafe {
            SetWindowSubclass(
                self.hwnd,
                Some(editor_endpoint_subclass_proc),
                EDITOR_ENDPOINT_SUBCLASS_ID,
                Rc::as_ptr(self) as usize,
            )
        };
        if installed == 0 {
            return Err(last_error());
        }
        Ok(())
    }

    #[cfg(not(windows))]
    fn install_lifecycle_guard(self: &Rc<Self>) -> Result<()> {
        let _ = self;
        Ok(())
    }

    fn send_direct_checked(&self, message: u32, wparam: usize, lparam: isize) -> Result<isize> {
        self.send_direct_if_alive(message, wparam, lparam)
            .ok_or(FastPadError::Invariant(ENDPOINT_DESTROYED))
    }

    fn send_direct_if_alive(&self, message: u32, wparam: usize, lparam: isize) -> Option<isize> {
        if self.destroyed.load(Ordering::Acquire) {
            return None;
        }
        Some(unsafe { (self.direct_fn)(self.direct_ptr, message, wparam, lparam) })
    }

    fn retain_document(&self, raw: isize) {
        if raw == 0 {
            return;
        }
        let _ = self.send_direct_if_alive(SCI_ADDREFDOCUMENT, 0, raw);
    }

    fn release_document(&self, raw: isize) {
        if raw == 0 {
            return;
        }
        let _release_result = self.send_direct_if_alive(SCI_RELEASEDOCUMENT, 0, raw);
        #[cfg(test)]
        if _release_result.is_some() {
            if let Some(counter) = &self.release_counter {
                counter.fetch_add(1, Ordering::SeqCst);
            }
            release_observation::record(self.hwnd, raw);
        }
    }
}

#[cfg(test)]
pub(crate) mod release_observation {
    use std::cell::RefCell;
    use windows_sys::Win32::Foundation::HWND;

    #[derive(Debug)]
    pub(crate) struct Release {
        pub(crate) hwnd: HWND,
        pub(crate) document: isize,
        pub(crate) window_was_live: bool,
    }

    thread_local! {
        static RELEASES: RefCell<Option<Vec<Release>>> = const { RefCell::new(None) };
    }

    pub(super) fn record(hwnd: HWND, document: isize) {
        RELEASES.with(|releases| {
            if let Some(releases) = releases.borrow_mut().as_mut() {
                releases.push(Release {
                    hwnd,
                    document,
                    window_was_live: unsafe {
                        windows_sys::Win32::UI::WindowsAndMessaging::IsWindow(hwnd) != 0
                    },
                });
            }
        });
    }

    pub(crate) fn during<R>(run: impl FnOnce() -> R) -> (R, Vec<Release>) {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                RELEASES.with(|releases| {
                    releases.replace(None);
                });
            }
        }
        RELEASES.with(|releases| {
            assert!(releases.replace(Some(Vec::new())).is_none());
        });
        let _reset = Reset;
        let result = run();
        let releases = RELEASES.with(|releases| releases.take().unwrap());
        (result, releases)
    }
}

impl Drop for EditorEndpoint {
    fn drop(&mut self) {
        if !self.destroy_window_on_drop {
            return;
        }
        if self.destroyed.swap(true, Ordering::AcqRel) {
            return;
        }

        #[cfg(windows)]
        unsafe {
            if !self.hwnd.is_null() {
                DestroyWindow(self.hwnd);
            }
        }
    }
}

#[cfg(windows)]
fn require_hwnd(raw: HWND) -> Result<HWND> {
    if raw.is_null() {
        Err(last_error())
    } else {
        Ok(raw)
    }
}

#[cfg(windows)]
fn create_scintilla_child(parent: HWND) -> Result<HWND> {
    let rect = parent_client_rect(parent)?;
    let class_name = wide_null("Scintilla");
    let width = (rect.right - rect.left).max(1);
    let height = (rect.bottom - rect.top).max(1);
    let hwnd = unsafe {
        // SAFETY: `parent` is treated as an opaque host HWND supplied by the caller. This helper
        // contains the only raw-HWND FFI for `Editor::create`, constraining the unchecked Win32
        // boundary to one private function.
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
    Ok(hwnd)
}

#[cfg(windows)]
fn parent_client_rect(parent: HWND) -> Result<RECT> {
    let mut rect = RECT::default();
    let ok = unsafe {
        // SAFETY: `parent` is forwarded unchanged to Win32 so the FFI dereference stays inside
        // this private boundary instead of the public `Editor::create` API.
        GetClientRect(parent, &mut rect)
    };
    if ok == 0 { Err(last_error()) } else { Ok(rect) }
}

#[cfg(windows)]
unsafe extern "system" fn editor_endpoint_subclass_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    ref_data: usize,
) -> isize {
    let endpoint = unsafe { &*(ref_data as *const EditorEndpoint) };
    if message == WM_NCDESTROY {
        endpoint.destroyed.store(true, Ordering::Release);
        unsafe {
            RemoveWindowSubclass(
                hwnd,
                Some(editor_endpoint_subclass_proc),
                EDITOR_ENDPOINT_SUBCLASS_ID,
            );
        }
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

#[cfg(test)]
unsafe extern "C" fn inert_direct_call(
    _direct_ptr: isize,
    _message: u32,
    _wparam: usize,
    _lparam: isize,
) -> isize {
    0
}

#[cfg(test)]
mod tests {
    use super::{Editor, EditorDocument};
    use crate::editor::scintilla_constants::{
        SCI_ADDREFDOCUMENT, SCI_BEGINUNDOACTION, SCI_CANREDO, SCI_CANUNDO, SCI_COPY, SCI_CUT,
        SCI_ENDUNDOACTION, SCI_GETSELECTIONEND, SCI_GETSELECTIONSTART, SCI_GETSELTEXT, SCI_PASTE,
        SCI_REDO, SCI_RELEASEDOCUMENT, SCI_REPLACETARGET, SCI_SEARCHINTARGET, SCI_SETILEXER,
        SCI_SETSEARCHFLAGS, SCI_SETSEL, SCI_SETTARGETRANGE, SCI_STYLECLEARALL, SCI_STYLESETBACK,
        SCI_STYLESETBOLD, SCI_STYLESETFONT, SCI_STYLESETFORE, SCI_UNDO,
    };
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    #[test]
    fn test_fixture_clone_and_drop_are_inert() {
        // Break caught: test-only fake document handles trying to refcount through a null endpoint.
        let fixture = EditorDocument::test_fixture();
        let clone = fixture.clone();
        assert_eq!(clone.raw(), 0);
        drop(clone);
        drop(fixture);
    }

    #[test]
    fn release_counter_does_not_claim_a_call_after_endpoint_destruction() {
        // Break caught: counting Drop attempts before the liveness gate overstates native releases.
        let releases = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let document = EditorDocument::test_fixture_with_release_counter(Arc::clone(&releases));
        document
            .endpoint
            .destroyed
            .store(true, std::sync::atomic::Ordering::Release);
        drop(document);
        assert_eq!(releases.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn document_refcounts_keep_using_the_cached_endpoint_after_editor_drop() {
        // Break caught: storing only an HWND in EditorDocument makes clone/drop target a dead
        // editor endpoint after the Editor value is dropped.
        let harness = TestDirectHarness::new();
        let editor = Editor::test_fixture(test_direct, harness.direct_ptr());
        let document = EditorDocument::test_fixture_with_raw(41, &editor);
        let clone = document.clone();

        drop(editor);
        drop(clone);
        drop(document);

        assert_eq!(
            harness.messages(),
            vec![SCI_ADDREFDOCUMENT, SCI_RELEASEDOCUMENT, SCI_RELEASEDOCUMENT]
        );
    }

    #[test]
    fn search_in_target_sets_range_and_flags_before_searching() {
        // Break caught: omitting the requested target range or search flags can reuse stale
        // Scintilla target state and return the wrong match.
        let harness = TestDirectHarness::new();
        harness.push_response(7);
        let editor = Editor::test_fixture(test_direct, harness.direct_ptr());

        let found = editor.search_in_target("needle", 3..15, 99).unwrap();

        assert_eq!(found, Some(7..13));
        assert_eq!(
            harness.messages(),
            vec![SCI_SETTARGETRANGE, SCI_SETSEARCHFLAGS, SCI_SEARCHINTARGET]
        );
        assert_eq!(harness.target_range(), Some((3, 15)));
        assert_eq!(harness.search_flags(), Some(99));
        assert_eq!(harness.search_needle(), Some(b"needle".to_vec()));
    }

    #[test]
    fn search_in_target_returns_none_when_scintilla_reports_no_match() {
        // Break caught: turning Scintilla's not-found sentinel into a bogus byte range instead of
        // reporting the absence of a match.
        let harness = TestDirectHarness::new();
        harness.push_response(-1);
        let editor = Editor::test_fixture(test_direct, harness.direct_ptr());

        let found = editor.search_in_target("needle", 0..6, 0).unwrap();

        assert_eq!(found, None);
    }

    #[test]
    fn length_reads_the_document_length() {
        let harness = TestDirectHarness::new();
        harness.push_response(42);
        let editor = Editor::test_fixture(test_direct, harness.direct_ptr());

        assert_eq!(editor.length().unwrap(), 42);
    }

    #[test]
    fn undo_and_redo_send_the_matching_scintilla_messages() {
        let harness = TestDirectHarness::new();
        let editor = Editor::test_fixture(test_direct, harness.direct_ptr());

        editor.undo().unwrap();
        editor.redo().unwrap();

        assert_eq!(harness.messages(), vec![SCI_UNDO, SCI_REDO]);
    }

    #[test]
    fn can_undo_and_can_redo_report_scintillas_boolean_state() {
        // Break caught: treating any nonzero Scintilla response as `true` incorrectly, or
        // collapsing distinct CANUNDO/CANREDO answers into one shared flag.
        let harness = TestDirectHarness::new();
        harness.push_response(1);
        harness.push_response(0);
        let editor = Editor::test_fixture(test_direct, harness.direct_ptr());

        assert!(editor.can_undo().unwrap());
        assert!(!editor.can_redo().unwrap());
        assert_eq!(harness.messages(), vec![SCI_CANUNDO, SCI_CANREDO]);
    }

    #[test]
    fn cut_copy_paste_send_the_matching_scintilla_messages() {
        let harness = TestDirectHarness::new();
        let editor = Editor::test_fixture(test_direct, harness.direct_ptr());

        editor.cut().unwrap();
        editor.copy().unwrap();
        editor.paste().unwrap();

        assert_eq!(harness.messages(), vec![SCI_CUT, SCI_COPY, SCI_PASTE]);
    }

    #[test]
    fn selection_reads_start_and_end_from_scintilla() {
        let harness = TestDirectHarness::new();
        harness.push_response(3);
        harness.push_response(9);
        let editor = Editor::test_fixture(test_direct, harness.direct_ptr());

        let range = editor.selection().unwrap();

        assert_eq!(range, 3..9);
        assert_eq!(
            harness.messages(),
            vec![SCI_GETSELECTIONSTART, SCI_GETSELECTIONEND]
        );
    }

    #[test]
    fn selection_rejects_an_end_before_start() {
        // Break caught: trusting Scintilla's raw start/end without validating ordering can hand
        // callers a range that panics on use (e.g. slicing) instead of a clear error.
        let harness = TestDirectHarness::new();
        harness.push_response(9);
        harness.push_response(3);
        let editor = Editor::test_fixture(test_direct, harness.direct_ptr());

        assert!(editor.selection().is_err());
    }

    #[test]
    fn set_selection_sends_anchor_and_caret_as_start_and_end() {
        let harness = TestDirectHarness::new();
        let editor = Editor::test_fixture(test_direct, harness.direct_ptr());

        editor.set_selection(4..10).unwrap();

        assert_eq!(harness.messages(), vec![SCI_SETSEL]);
        assert_eq!(harness.set_sel_calls(), vec![(4, 10)]);
    }

    #[test]
    fn selected_text_reads_the_current_selection_without_a_full_document_fetch() {
        let harness = TestDirectHarness::new();
        harness.set_selected_text("needle");
        let editor = Editor::test_fixture(test_direct, harness.direct_ptr());

        let text = editor.selected_text().unwrap();

        assert_eq!(text, "needle");
        assert_eq!(harness.messages(), vec![SCI_GETSELTEXT, SCI_GETSELTEXT]);
    }

    #[test]
    fn replace_target_sets_the_range_then_replaces_and_returns_the_new_end() {
        let harness = TestDirectHarness::new();
        let editor = Editor::test_fixture(test_direct, harness.direct_ptr());

        let replaced = editor.replace_target(4..7, "longer").unwrap();

        assert_eq!(replaced, 4..10);
        assert_eq!(
            harness.messages(),
            vec![SCI_SETTARGETRANGE, SCI_REPLACETARGET]
        );
        assert_eq!(harness.target_range(), Some((4, 7)));
        assert_eq!(harness.replace_bytes(), vec![b"longer".to_vec()]);
    }

    #[test]
    fn replace_all_replaces_every_match_as_exactly_one_undo_action() {
        // Break caught: wrapping each individual replacement in its own undo action instead of one
        // action for the whole operation would require multiple Ctrl+Z presses to undo Replace All.
        let harness = TestDirectHarness::new();
        // Iteration 1: document length 11 ("one two one"), match "one" at 0.
        harness.push_response(0); // SCI_BEGINUNDOACTION (ignored)
        harness.push_response(11); // SCI_GETLENGTH
        harness.push_response(0); // SCI_SEARCHINTARGET finds "one" at 0
        harness.push_response(0); // SCI_REPLACETARGET (ignored)
        // Iteration 2: document length now 12 (replaced 3 bytes with 4), match "one" at 9.
        harness.push_response(12); // SCI_GETLENGTH
        harness.push_response(9); // SCI_SEARCHINTARGET finds "one" at 9
        harness.push_response(0); // SCI_REPLACETARGET (ignored)
        // Iteration 3: no more matches.
        harness.push_response(13); // SCI_GETLENGTH
        harness.push_response(-1); // SCI_SEARCHINTARGET finds nothing
        harness.push_response(0); // SCI_ENDUNDOACTION (ignored)
        let editor = Editor::test_fixture(test_direct, harness.direct_ptr());

        let count = editor.replace_all("one", "1111", 0).unwrap();

        assert_eq!(count, 2);
        assert_eq!(
            harness.replace_bytes(),
            vec![b"1111".to_vec(), b"1111".to_vec()]
        );
        assert_eq!(
            harness.event_log(),
            vec!["begin", "replace", "replace", "end"]
        );
    }

    #[test]
    fn set_lexer_sends_the_raw_pointer_via_sci_setilexer() {
        // Break caught: not forwarding the exact opaque ILexer5 pointer Lexilla returned (or
        // routing it through the wrong message) would hand Scintilla a value it cannot own.
        let harness = TestDirectHarness::new();
        let editor = Editor::test_fixture(test_direct, harness.direct_ptr());

        editor.set_lexer(0x1234).unwrap();

        assert_eq!(harness.messages(), vec![SCI_SETILEXER]);
        assert_eq!(harness.lexer_calls(), vec![0x1234]);
    }

    #[test]
    fn set_lexer_with_null_sends_the_null_lexer() {
        // Break caught: treating a null (plain text) lexer as a no-op instead of explicitly
        // clearing any previously installed lexer.
        let harness = TestDirectHarness::new();
        let editor = Editor::test_fixture(test_direct, harness.direct_ptr());

        editor.set_lexer(0).unwrap();

        assert_eq!(harness.lexer_calls(), vec![0]);
    }

    #[test]
    fn clear_all_styles_sends_sci_styleclearall() {
        let harness = TestDirectHarness::new();
        let editor = Editor::test_fixture(test_direct, harness.direct_ptr());

        editor.clear_all_styles().unwrap();

        assert_eq!(harness.messages(), vec![SCI_STYLECLEARALL]);
    }

    #[test]
    fn set_style_sends_foreground_background_bold_and_font_for_the_style_id() {
        // Break caught: dropping one of fore/back/bold/font, or sending them for the wrong style
        // id, leaves a lexer's styling stale or bleeding across style numbers.
        let harness = TestDirectHarness::new();
        let editor = Editor::test_fixture(test_direct, harness.direct_ptr());

        editor
            .set_style(2, 0xff0000, 0x00ff00, true, "Consolas")
            .unwrap();

        assert_eq!(
            harness.messages(),
            vec![
                SCI_STYLESETFORE,
                SCI_STYLESETBACK,
                SCI_STYLESETBOLD,
                SCI_STYLESETFONT
            ]
        );
        assert_eq!(
            harness.style_calls(),
            vec![
                (SCI_STYLESETFORE, 2, 0xff0000),
                (SCI_STYLESETBACK, 2, 0x00ff00),
                (SCI_STYLESETBOLD, 2, 1),
            ]
        );
        assert_eq!(harness.font_calls(), vec![(2, b"Consolas".to_vec())]);
    }

    #[test]
    fn set_style_sends_a_zero_bold_flag_when_not_bold() {
        let harness = TestDirectHarness::new();
        let editor = Editor::test_fixture(test_direct, harness.direct_ptr());

        editor.set_style(0, 0, 0, false, "Consolas").unwrap();

        assert_eq!(
            harness.style_calls(),
            vec![
                (SCI_STYLESETFORE, 0, 0),
                (SCI_STYLESETBACK, 0, 0),
                (SCI_STYLESETBOLD, 0, 0),
            ]
        );
    }

    #[test]
    fn set_style_rejects_a_font_face_containing_nul_bytes() {
        let harness = TestDirectHarness::new();
        let editor = Editor::test_fixture(test_direct, harness.direct_ptr());

        assert!(editor.set_style(0, 0, 0, false, "bad\0face").is_err());
    }

    #[test]
    fn replace_all_with_an_empty_query_does_nothing() {
        let harness = TestDirectHarness::new();
        let editor = Editor::test_fixture(test_direct, harness.direct_ptr());

        let count = editor.replace_all("", "x", 0).unwrap();

        assert_eq!(count, 0);
        assert!(harness.messages().is_empty());
    }

    #[derive(Default)]
    struct TestDirectState {
        messages: Vec<u32>,
        responses: VecDeque<isize>,
        target_range: Option<(usize, isize)>,
        search_flags: Option<usize>,
        search_needle: Option<Vec<u8>>,
        replace_bytes: Vec<Vec<u8>>,
        set_sel_calls: Vec<(usize, isize)>,
        selected_text: Option<Vec<u8>>,
        event_log: Vec<&'static str>,
        lexer_calls: Vec<isize>,
        style_calls: Vec<(u32, usize, isize)>,
        font_calls: Vec<(usize, Vec<u8>)>,
    }

    struct TestDirectHarness {
        state: Arc<Mutex<TestDirectState>>,
    }

    impl TestDirectHarness {
        fn new() -> Self {
            Self {
                state: Arc::new(Mutex::new(TestDirectState::default())),
            }
        }

        fn direct_ptr(&self) -> isize {
            Arc::as_ptr(&self.state) as isize
        }

        fn push_response(&self, response: isize) {
            self.state.lock().unwrap().responses.push_back(response);
        }

        fn messages(&self) -> Vec<u32> {
            self.state.lock().unwrap().messages.clone()
        }

        fn target_range(&self) -> Option<(usize, isize)> {
            self.state.lock().unwrap().target_range
        }

        fn search_flags(&self) -> Option<usize> {
            self.state.lock().unwrap().search_flags
        }

        fn search_needle(&self) -> Option<Vec<u8>> {
            self.state.lock().unwrap().search_needle.clone()
        }

        fn replace_bytes(&self) -> Vec<Vec<u8>> {
            self.state.lock().unwrap().replace_bytes.clone()
        }

        fn set_sel_calls(&self) -> Vec<(usize, isize)> {
            self.state.lock().unwrap().set_sel_calls.clone()
        }

        fn set_selected_text(&self, text: &str) {
            self.state.lock().unwrap().selected_text = Some(text.as_bytes().to_vec());
        }

        fn event_log(&self) -> Vec<&'static str> {
            self.state.lock().unwrap().event_log.clone()
        }

        fn lexer_calls(&self) -> Vec<isize> {
            self.state.lock().unwrap().lexer_calls.clone()
        }

        fn style_calls(&self) -> Vec<(u32, usize, isize)> {
            self.state.lock().unwrap().style_calls.clone()
        }

        fn font_calls(&self) -> Vec<(usize, Vec<u8>)> {
            self.state.lock().unwrap().font_calls.clone()
        }
    }

    unsafe extern "C" fn test_direct(
        direct_ptr: isize,
        message: u32,
        wparam: usize,
        lparam: isize,
    ) -> isize {
        let shared = unsafe { &*(direct_ptr as *const Mutex<TestDirectState>) };
        let mut state = shared.lock().unwrap();
        state.messages.push(message);
        match message {
            SCI_SETTARGETRANGE => {
                state.target_range = Some((wparam, lparam));
                0
            }
            SCI_SETSEARCHFLAGS => {
                state.search_flags = Some(wparam);
                0
            }
            SCI_SEARCHINTARGET => {
                let bytes = unsafe { std::slice::from_raw_parts(lparam as *const u8, wparam) };
                state.search_needle = Some(bytes.to_vec());
                state.responses.pop_front().unwrap_or(-1)
            }
            SCI_REPLACETARGET => {
                let bytes = unsafe { std::slice::from_raw_parts(lparam as *const u8, wparam) };
                state.replace_bytes.push(bytes.to_vec());
                state.event_log.push("replace");
                state.responses.pop_front().unwrap_or(0)
            }
            SCI_SETSEL => {
                state.set_sel_calls.push((wparam, lparam));
                0
            }
            SCI_BEGINUNDOACTION => {
                state.event_log.push("begin");
                state.responses.pop_front().unwrap_or(0)
            }
            SCI_ENDUNDOACTION => {
                state.event_log.push("end");
                state.responses.pop_front().unwrap_or(0)
            }
            SCI_GETSELTEXT => {
                let text = state.selected_text.clone().unwrap_or_default();
                if lparam != 0 {
                    let buffer = unsafe {
                        std::slice::from_raw_parts_mut(lparam as *mut u8, text.len() + 1)
                    };
                    buffer[..text.len()].copy_from_slice(&text);
                    buffer[text.len()] = 0;
                }
                text.len() as isize
            }
            SCI_SETILEXER => {
                state.lexer_calls.push(lparam);
                0
            }
            SCI_STYLESETFORE | SCI_STYLESETBACK | SCI_STYLESETBOLD => {
                state.style_calls.push((message, wparam, lparam));
                0
            }
            SCI_STYLESETFONT => {
                let bytes = unsafe { std::ffi::CStr::from_ptr(lparam as *const std::ffi::c_char) }
                    .to_bytes()
                    .to_vec();
                state.font_calls.push((wparam, bytes));
                0
            }
            _ => state.responses.pop_front().unwrap_or(0),
        }
    }
}
