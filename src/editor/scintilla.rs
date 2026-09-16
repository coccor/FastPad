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
    SC_ELEMENT_CARET_LINE_BACK, SC_ELEMENT_SELECTION_BACK, SC_ELEMENT_SELECTION_INACTIVE_BACK,
    SC_ELEMENT_SELECTION_INACTIVE_TEXT, SC_ELEMENT_SELECTION_TEXT, SC_WRAP_NONE, SC_WRAP_WORD,
    SCI_RESETELEMENTCOLOUR, SCI_SETCARETFORE, SCI_SETELEMENTCOLOUR, SCI_SETMARGINLEFT,
    SCI_SETMARGINRIGHT, SCI_SETMARGINWIDTHN, SCI_SETSCROLLWIDTH, SCI_SETSCROLLWIDTHTRACKING,
    SCI_SETTABWIDTH, SCI_SETWRAPMODE, SCI_STYLESETSIZEFRACTIONAL, STYLE_DEFAULT,
};
#[cfg(windows)]
use crate::editor::scintilla_constants::{SC_MARGIN_NUMBER, SCI_SETMARGINTYPEN, SCI_STYLEGETBACK};
use crate::editor::scintilla_constants::{
    SCI_GETLINECOUNT, SCI_SETZOOM, SCI_TEXTWIDTH, SCI_ZOOMIN, SCI_ZOOMOUT, STYLE_LINENUMBER,
};
use crate::{FastPadError, Result};
use std::cell::Cell;
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
use windows_sys::Win32::Foundation::{GetLastError, LPARAM, RECT, SetLastError, WPARAM};
#[cfg(windows)]
use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
#[cfg(windows)]
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL};
#[cfg(windows)]
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, GWL_EXSTYLE, GetClientRect, GetWindowLongPtrW,
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SendMessageW,
    SetWindowLongPtrW, SetWindowPos, WM_CHAR, WM_DPICHANGED_AFTERPARENT, WM_NCDESTROY, WS_CHILD,
    WS_EX_LAYOUTRTL, WS_TABSTOP, WS_VISIBLE,
};

pub type SciFnDirect = unsafe extern "C" fn(isize, u32, usize, isize) -> isize;

const ENDPOINT_DESTROYED: &str = "Scintilla editor endpoint is no longer alive";
#[cfg(windows)]
const EDITOR_ENDPOINT_SUBCLASS_ID: usize = 0x4650_4544;

/// The reading order the editor lays text out in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextDirection {
    LeftToRight,
    RightToLeft,
}

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
    line_numbers: Cell<LineNumberMargin>,
    #[cfg(test)]
    release_counter: Option<std::sync::Arc<std::sync::atomic::AtomicUsize>>,
}

/// Whether margin 0 shows line numbers, and how many digits its current width was measured for
/// (0 means not measured yet).
#[derive(Clone, Copy, Debug, Default)]
struct LineNumberMargin {
    visible: bool,
    digits: usize,
}

/// Digits needed for the largest line number, never fewer than two so the margin does not resize
/// at line 10.
fn line_number_digits(line_count: isize) -> usize {
    (line_count.max(1).ilog10() as usize + 1).max(2)
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
        let dpi = unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd) };
        editor.initialize_view(|editor| editor.apply_chrome_defaults(dpi))?;
        Ok(editor)
    }

    #[cfg(not(windows))]
    pub fn create(_parent: HWND) -> Result<Self> {
        Err(FastPadError::Invariant(
            "Scintilla editor is only supported on Windows",
        ))
    }

    /// Sets the code page, which the UTF-8 contract depends on, and then applies the purely
    /// cosmetic chrome defaults. Spec 228 keeps non-fatal failures non-fatal: a failed margin or
    /// scroll-width call must never stop FastPad from opening an editable window.
    fn initialize_view(&self, apply_chrome: impl FnOnce(&Self) -> Result<()>) -> Result<()> {
        self.endpoint
            .send_direct_checked(SCI_SETCODEPAGE, SC_CP_UTF8 as usize, 0)?;
        let _ = apply_chrome(self);
        Ok(())
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
        // File loads suppress the window's edit notifications, so size the gutter here.
        let _ = self.endpoint.size_line_number_margin(false);
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
        // Scroll width is per view and tracking only grows it; restart from the new document.
        self.endpoint
            .send_direct_checked(SCI_SETSCROLLWIDTH, 1, 0)?;
        let _ = self.endpoint.size_line_number_margin(false);
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

    /// Applies user view settings to every style up to `STYLE_LINENUMBER` without touching text,
    /// selection, or colors, then re-sizes the line-number margin for the new font.
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
        for style in 0..=STYLE_LINENUMBER as usize {
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
        self.endpoint.size_line_number_margin(true)
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

    /// Keeps margin 0 as the only (line-number) margin, on the text background rather than
    /// Scintilla's grey band, pads the text area, lets the horizontal scrollbar follow the widest
    /// line instead of the default 2000 px scroll width, and shows line numbers until settings load.
    #[cfg(windows)]
    pub fn apply_chrome_defaults(&self, dpi: u32) -> Result<()> {
        self.endpoint
            .send_direct_checked(SCI_SETMARGINTYPEN, 0, SC_MARGIN_NUMBER as isize)?;
        for margin in 1..=2 {
            self.endpoint
                .send_direct_checked(SCI_SETMARGINWIDTHN, margin, 0)?;
        }
        let background =
            self.endpoint
                .send_direct_checked(SCI_STYLEGETBACK, STYLE_DEFAULT as usize, 0)?;
        self.endpoint.send_direct_checked(
            SCI_STYLESETBACK,
            STYLE_LINENUMBER as usize,
            background,
        )?;
        self.set_text_padding(dpi)?;
        self.endpoint
            .send_direct_checked(SCI_SETSCROLLWIDTH, 1, 0)?;
        self.endpoint
            .send_direct_checked(SCI_SETSCROLLWIDTHTRACKING, 1, 0)?;
        self.set_line_numbers(true)
    }

    #[cfg(not(windows))]
    pub fn apply_chrome_defaults(&self, _dpi: u32) -> Result<()> {
        Err(FastPadError::Invariant(
            "Scintilla editor is only supported on Windows",
        ))
    }

    /// Shows margin 0 sized for the current line count, or collapses it to zero width. Repeating
    /// the current, already-applied state sends nothing.
    pub fn set_line_numbers(&self, visible: bool) -> Result<()> {
        let current = self.endpoint.line_numbers.get();
        if current.visible == visible && (!visible || current.digits != 0) {
            return Ok(());
        }
        self.endpoint
            .line_numbers
            .set(LineNumberMargin { visible, digits: 0 });
        if visible {
            self.endpoint.size_line_number_margin(true)
        } else {
            self.endpoint
                .send_direct_checked(SCI_SETMARGINWIDTHN, 0, 0)?;
            Ok(())
        }
    }

    /// Re-sizes visible line numbers only if the line count gained or lost a digit, which keeps it
    /// cheap enough to run after every line-changing edit.
    pub fn refresh_line_numbers(&self) -> Result<()> {
        self.endpoint.size_line_number_margin(false)
    }

    /// Re-measures visible line numbers unconditionally, for font or DPI changes.
    pub fn remeasure_line_numbers(&self) -> Result<()> {
        self.endpoint.size_line_number_margin(true)
    }

    /// Colors the line-number margin. `SCI_STYLECLEARALL` resets it along with every other style,
    /// so callers reapply this after a lexer change.
    pub fn set_line_number_colors(&self, foreground: u32, background: u32) -> Result<()> {
        self.endpoint.send_direct_checked(
            SCI_STYLESETFORE,
            STYLE_LINENUMBER as usize,
            foreground as isize,
        )?;
        self.endpoint.send_direct_checked(
            SCI_STYLESETBACK,
            STYLE_LINENUMBER as usize,
            background as isize,
        )?;
        Ok(())
    }

    /// Left/right text padding in physical pixels for `dpi` (8 px at 96 DPI).
    #[cfg(windows)]
    pub fn set_text_padding(&self, dpi: u32) -> Result<()> {
        let padding = ((8 * i64::from(dpi.max(96)) + 48) / 96) as isize;
        self.endpoint
            .send_direct_checked(SCI_SETMARGINLEFT, 0, padding)?;
        self.endpoint
            .send_direct_checked(SCI_SETMARGINRIGHT, 0, padding)?;
        Ok(())
    }

    #[cfg(not(windows))]
    pub fn set_text_padding(&self, _dpi: u32) -> Result<()> {
        Err(FastPadError::Invariant(
            "Scintilla editor is only supported on Windows",
        ))
    }

    /// Magnifies every style by one point. The view notifies `SCN_ZOOM`, which is where the owner
    /// re-measures the line-number margin, so Ctrl+wheel zooming keeps the gutter sized too.
    pub fn zoom_in(&self) -> Result<()> {
        self.endpoint.send_direct_checked(SCI_ZOOMIN, 0, 0)?;
        Ok(())
    }

    pub fn zoom_out(&self) -> Result<()> {
        self.endpoint.send_direct_checked(SCI_ZOOMOUT, 0, 0)?;
        Ok(())
    }

    /// Returns to the configured font size.
    pub fn reset_zoom(&self) -> Result<()> {
        self.endpoint.send_direct_checked(SCI_SETZOOM, 0, 0)?;
        Ok(())
    }

    /// Mirrors the editor window so lines start at the right edge and the vertical scrollbar sits
    /// on the left. Scintilla's own bidirectional mode needs DirectWrite, and FastPad draws with
    /// GDI, which already reorders right-to-left runs inside a mirrored window.
    #[cfg(windows)]
    pub fn set_text_direction(&self, direction: TextDirection) -> Result<()> {
        let hwnd = self.endpoint.hwnd;
        let style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
        let updated = match direction {
            TextDirection::LeftToRight => style & !(WS_EX_LAYOUTRTL as isize),
            TextDirection::RightToLeft => style | WS_EX_LAYOUTRTL as isize,
        };
        if updated == style {
            return Ok(());
        }
        unsafe {
            SetLastError(0);
            if SetWindowLongPtrW(hwnd, GWL_EXSTYLE, updated) == 0 && GetLastError() != 0 {
                return Err(last_error());
            }
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
            InvalidateRect(hwnd, std::ptr::null(), 1);
        }
        Ok(())
    }

    #[cfg(not(windows))]
    pub fn set_text_direction(&self, _direction: TextDirection) -> Result<()> {
        Err(FastPadError::Invariant(
            "Scintilla editor is only supported on Windows",
        ))
    }

    /// Sets the selection (focused and unfocused) and caret-line backgrounds as opaque Scintilla 5
    /// element colours.
    #[cfg(windows)]
    pub fn set_chrome_colors(
        &self,
        selection: u32,
        inactive_selection: u32,
        caret_line: u32,
    ) -> Result<()> {
        for (element, colour) in [
            (SC_ELEMENT_SELECTION_BACK, selection),
            (SC_ELEMENT_SELECTION_INACTIVE_BACK, inactive_selection),
            (SC_ELEMENT_CARET_LINE_BACK, caret_line),
        ] {
            self.endpoint.send_direct_checked(
                SCI_SETELEMENTCOLOUR,
                element as usize,
                ((colour & 0x00FF_FFFF) | 0xFF00_0000) as isize,
            )?;
        }
        Ok(())
    }

    #[cfg(not(windows))]
    pub fn set_chrome_colors(
        &self,
        _selection: u32,
        _inactive_selection: u32,
        _caret_line: u32,
    ) -> Result<()> {
        Err(FastPadError::Invariant(
            "Scintilla editor is only supported on Windows",
        ))
    }

    /// Forces the selected-text foreground (focused and unfocused), or resets both elements so the
    /// lexer's own style colors show through again. Only high contrast sets a color, where selected
    /// text must keep the system highlight pair to stay legible.
    #[cfg(windows)]
    pub fn set_selection_text_colors(&self, foreground: Option<u32>) -> Result<()> {
        for element in [
            SC_ELEMENT_SELECTION_TEXT,
            SC_ELEMENT_SELECTION_INACTIVE_TEXT,
        ] {
            match foreground {
                Some(colour) => self.endpoint.send_direct_checked(
                    SCI_SETELEMENTCOLOUR,
                    element as usize,
                    ((colour & 0x00FF_FFFF) | 0xFF00_0000) as isize,
                )?,
                None => self.endpoint.send_direct_checked(
                    SCI_RESETELEMENTCOLOUR,
                    element as usize,
                    0,
                )?,
            };
        }
        Ok(())
    }

    #[cfg(not(windows))]
    pub fn set_selection_text_colors(&self, _foreground: Option<u32>) -> Result<()> {
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
                line_numbers: Cell::new(LineNumberMargin::default()),
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
            line_numbers: Cell::new(LineNumberMargin::default()),
            #[cfg(test)]
            release_counter: None,
        }
    }

    /// Sizes margin 0 for the current line count plus one digit of breathing room. Without `force`
    /// the font measurement is skipped while the digit count is unchanged.
    fn size_line_number_margin(&self, force: bool) -> Result<()> {
        let mut margin = self.line_numbers.get();
        if !margin.visible {
            return Ok(());
        }
        let line_count = self.send_direct_checked(SCI_GETLINECOUNT, 0, 0)?;
        let digits = line_number_digits(line_count);
        if !force && digits == margin.digits {
            return Ok(());
        }
        let sample = CString::new("9".repeat(digits + 1)).expect("digits contain no NUL bytes");
        let width = self.send_direct_checked(
            SCI_TEXTWIDTH,
            STYLE_LINENUMBER as usize,
            sample.as_ptr() as isize,
        )?;
        self.send_direct_checked(SCI_SETMARGINWIDTHN, 0, width)?;
        margin.digits = digits;
        self.line_numbers.set(margin);
        Ok(())
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
    if message == WM_CHAR {
        let ctrl_down = unsafe { GetKeyState(VK_CONTROL as i32) } < 0;
        if crate::editor::input_filter::should_ignore_char(wparam as u16, ctrl_down) {
            return 0;
        }
    }
    if message == WM_DPICHANGED_AFTERPARENT {
        // Scintilla adopts the new DPI inside its own handler; measure digits only after that.
        let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
        let _ = endpoint.size_line_number_margin(true);
        return result;
    }
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
        SC_ELEMENT_CARET_LINE_BACK, SC_ELEMENT_SELECTION_BACK, SC_ELEMENT_SELECTION_INACTIVE_BACK,
        SCI_ADDREFDOCUMENT, SCI_BEGINUNDOACTION, SCI_CANREDO, SCI_CANUNDO, SCI_COPY, SCI_CUT,
        SCI_ENDUNDOACTION, SCI_GETSELECTIONEND, SCI_GETSELECTIONSTART, SCI_GETSELTEXT, SCI_PASTE,
        SCI_REDO, SCI_RELEASEDOCUMENT, SCI_REPLACETARGET, SCI_SEARCHINTARGET, SCI_SETDOCPOINTER,
        SCI_SETELEMENTCOLOUR, SCI_SETILEXER, SCI_SETMARGINLEFT, SCI_SETMARGINRIGHT,
        SCI_SETMARGINWIDTHN, SCI_SETSCROLLWIDTH, SCI_SETSCROLLWIDTHTRACKING, SCI_SETSEARCHFLAGS,
        SCI_SETSEL, SCI_SETTARGETRANGE, SCI_STYLECLEARALL, SCI_STYLESETBACK, SCI_STYLESETBOLD,
        SCI_STYLESETFONT, SCI_STYLESETFORE, SCI_UNDO,
    };
    use crate::editor::scintilla_constants::{
        SC_MARGIN_NUMBER, SCI_GETLINECOUNT, SCI_SETMARGINTYPEN, SCI_SETZOOM, SCI_STYLEGETBACK,
        SCI_TEXTWIDTH, SCI_ZOOMIN, SCI_ZOOMOUT, STYLE_DEFAULT, STYLE_LINENUMBER,
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
    fn a_failed_cosmetic_chrome_default_still_yields_a_usable_editor() {
        // Break caught: a cosmetic margin or scroll-width failure aborts Editor::create, so FastPad
        // exits with a startup-fatal code instead of opening an editable window (spec 228).
        let harness = TestDirectHarness::new();
        let editor = Editor::test_fixture(test_direct, harness.direct_ptr());

        let result = editor
            .initialize_view(|_| Err(crate::FastPadError::Invariant("cosmetic chrome failure")));

        assert!(result.is_ok());
        assert_eq!(
            harness.messages(),
            vec![crate::editor::scintilla_constants::SCI_SETCODEPAGE]
        );
    }

    #[test]
    fn chrome_defaults_show_only_a_line_number_margin_and_track_scroll_width() {
        // Break caught: Scintilla's default 16 px symbol margin and 2000 px scroll width show an
        // unthemed grey gutter and a permanent horizontal scrollbar, and a number margin left at
        // its default grey background or zero width hides the line numbers shown by default.
        let harness = TestDirectHarness::new();
        harness.set_line_count(1);
        let editor = Editor::test_fixture(test_direct, harness.direct_ptr());

        editor.apply_chrome_defaults(144).unwrap();

        assert_eq!(
            harness.calls(),
            vec![
                (SCI_SETMARGINTYPEN, 0, SC_MARGIN_NUMBER as isize),
                (SCI_SETMARGINWIDTHN, 1, 0),
                (SCI_SETMARGINWIDTHN, 2, 0),
                (SCI_STYLEGETBACK, STYLE_DEFAULT as usize, 0),
                (
                    SCI_STYLESETBACK,
                    STYLE_LINENUMBER as usize,
                    TEST_DEFAULT_BACKGROUND
                ),
                (SCI_SETMARGINLEFT, 0, 12),
                (SCI_SETMARGINRIGHT, 0, 12),
                (SCI_SETSCROLLWIDTH, 1, 0),
                (SCI_SETSCROLLWIDTHTRACKING, 1, 0),
                (SCI_GETLINECOUNT, 0, 0),
                (SCI_TEXTWIDTH, STYLE_LINENUMBER as usize, 0),
                (SCI_SETMARGINWIDTHN, 0, 30),
            ]
        );
        // Two digits plus one digit of breathing room, so short files do not resize at line 10.
        assert_eq!(harness.text_width_texts(), vec![b"999".to_vec()]);
    }

    #[test]
    fn line_number_margin_resizes_only_when_the_line_count_gains_a_digit() {
        // Break caught: re-measuring on every edit costs a font measurement per keystroke, while
        // never re-measuring clips line 100 in a margin sized for two digits.
        let harness = TestDirectHarness::new();
        harness.set_line_count(9);
        let editor = Editor::test_fixture(test_direct, harness.direct_ptr());
        editor.set_line_numbers(true).unwrap();

        harness.set_line_count(99);
        editor.refresh_line_numbers().unwrap();
        harness.set_line_count(100);
        editor.refresh_line_numbers().unwrap();

        let widths: Vec<_> = harness
            .calls()
            .into_iter()
            .filter(|call| call.0 == SCI_SETMARGINWIDTHN)
            .collect();
        assert_eq!(
            widths,
            vec![(SCI_SETMARGINWIDTHN, 0, 30), (SCI_SETMARGINWIDTHN, 0, 40)]
        );
        assert_eq!(
            harness.text_width_texts(),
            vec![b"999".to_vec(), b"9999".to_vec()]
        );
    }

    #[test]
    fn hidden_line_numbers_collapse_the_margin_and_ignore_refreshes() {
        // Break caught: line_numbers=false still showing a gutter, or a later edit re-opening it.
        let harness = TestDirectHarness::new();
        harness.set_line_count(500);
        let editor = Editor::test_fixture(test_direct, harness.direct_ptr());
        editor.set_line_numbers(true).unwrap();

        editor.set_line_numbers(false).unwrap();
        let hidden_at = harness.calls().len();
        editor.refresh_line_numbers().unwrap();
        editor.remeasure_line_numbers().unwrap();

        assert_eq!(harness.calls()[hidden_at - 1], (SCI_SETMARGINWIDTHN, 0, 0));
        assert_eq!(harness.calls().len(), hidden_at);
    }

    #[test]
    fn remeasuring_line_numbers_resizes_even_when_the_digit_count_is_unchanged() {
        // Break caught: a font-size or DPI change keeping the old pixel width, clipping the numbers.
        let harness = TestDirectHarness::new();
        harness.set_line_count(9);
        let editor = Editor::test_fixture(test_direct, harness.direct_ptr());
        editor.set_line_numbers(true).unwrap();

        editor.remeasure_line_numbers().unwrap();

        assert_eq!(harness.text_width_texts().len(), 2);
    }

    #[test]
    fn view_settings_also_restyle_the_line_number_font() {
        // Break caught: STYLE_LINENUMBER sits just past STYLE_DEFAULT, so a loop ending at
        // STYLE_DEFAULT leaves line numbers in Scintilla's default font and size.
        let harness = TestDirectHarness::new();
        let editor = Editor::test_fixture(test_direct, harness.direct_ptr());

        editor
            .apply_view_settings("Cascadia Code", 12, 4, false)
            .unwrap();

        assert!(
            harness
                .font_calls()
                .contains(&(STYLE_LINENUMBER as usize, b"Cascadia Code".to_vec()))
        );
    }

    #[test]
    fn line_number_colors_target_the_line_number_style() {
        // Break caught: STYLE_LINENUMBER sits outside the base-color loop, so the gutter keeps
        // Scintilla's grey band, or full-contrast text after a lexer's style reset.
        let harness = TestDirectHarness::new();
        let editor = Editor::test_fixture(test_direct, harness.direct_ptr());

        editor
            .set_line_number_colors(0x0060_6060, 0x00FF_FFFF)
            .unwrap();

        assert_eq!(
            harness.style_calls(),
            vec![
                (SCI_STYLESETFORE, STYLE_LINENUMBER as usize, 0x0060_6060),
                (SCI_STYLESETBACK, STYLE_LINENUMBER as usize, 0x00FF_FFFF),
            ]
        );
    }

    #[test]
    fn text_padding_is_eight_pixels_at_96_dpi() {
        // Break caught: zero margins leave the caret and first glyph touching the window frame.
        let harness = TestDirectHarness::new();
        let editor = Editor::test_fixture(test_direct, harness.direct_ptr());

        editor.set_text_padding(96).unwrap();

        assert_eq!(
            harness.calls(),
            vec![(SCI_SETMARGINLEFT, 0, 8), (SCI_SETMARGINRIGHT, 0, 8)]
        );
    }

    #[test]
    fn zoom_commands_step_the_view_and_reset_to_the_configured_size() {
        let harness = TestDirectHarness::new();
        let editor = Editor::test_fixture(test_direct, harness.direct_ptr());

        editor.zoom_in().unwrap();
        editor.zoom_out().unwrap();
        editor.reset_zoom().unwrap();

        assert_eq!(
            harness.calls(),
            vec![(SCI_ZOOMIN, 0, 0), (SCI_ZOOMOUT, 0, 0), (SCI_SETZOOM, 0, 0)]
        );
    }

    #[test]
    fn switching_documents_resets_the_scroll_width() {
        // Break caught: scroll-width tracking only grows, so a short tab after a wide one would
        // keep the wide tab's horizontal scrollbar.
        let harness = TestDirectHarness::new();
        let editor = Editor::test_fixture(test_direct, harness.direct_ptr());
        let document = EditorDocument::test_fixture_with_raw(41, &editor);

        editor.use_document(&document).unwrap();

        assert_eq!(
            harness.calls(),
            vec![(SCI_SETDOCPOINTER, 0, 41), (SCI_SETSCROLLWIDTH, 1, 0)]
        );
    }

    #[test]
    fn chrome_colors_are_sent_as_opaque_element_colours() {
        // Break caught: Scintilla 5 element colours carry alpha in the top byte; a bare COLORREF has
        // alpha 0 and would leave the selection and caret line invisible.
        let harness = TestDirectHarness::new();
        let editor = Editor::test_fixture(test_direct, harness.direct_ptr());

        editor
            .set_chrome_colors(0x0078_4F26, 0x0041_3D3A, 0x0028_2828)
            .unwrap();

        assert_eq!(
            harness.calls(),
            vec![
                (
                    SCI_SETELEMENTCOLOUR,
                    SC_ELEMENT_SELECTION_BACK as usize,
                    0xFF78_4F26_u32 as isize
                ),
                (
                    SCI_SETELEMENTCOLOUR,
                    SC_ELEMENT_SELECTION_INACTIVE_BACK as usize,
                    0xFF41_3D3A_u32 as isize
                ),
                (
                    SCI_SETELEMENTCOLOUR,
                    SC_ELEMENT_CARET_LINE_BACK as usize,
                    0xFF28_2828_u32 as isize
                ),
            ]
        );
    }

    #[test]
    fn high_contrast_selection_text_is_forced_and_otherwise_reset() {
        // Break caught: leaving the selected-text element unset paints lexer-colored text on the
        // system highlight background in high contrast, which is frequently unreadable; never
        // resetting it would then keep those system colors after leaving high contrast.
        use crate::editor::scintilla_constants::{
            SC_ELEMENT_SELECTION_INACTIVE_TEXT, SC_ELEMENT_SELECTION_TEXT, SCI_RESETELEMENTCOLOUR,
        };
        let harness = TestDirectHarness::new();
        let editor = Editor::test_fixture(test_direct, harness.direct_ptr());

        editor.set_selection_text_colors(Some(0x00FF_FFFF)).unwrap();
        editor.set_selection_text_colors(None).unwrap();

        assert_eq!(
            harness.calls(),
            vec![
                (
                    SCI_SETELEMENTCOLOUR,
                    SC_ELEMENT_SELECTION_TEXT as usize,
                    0xFFFF_FFFF_u32 as isize
                ),
                (
                    SCI_SETELEMENTCOLOUR,
                    SC_ELEMENT_SELECTION_INACTIVE_TEXT as usize,
                    0xFFFF_FFFF_u32 as isize
                ),
                (
                    SCI_RESETELEMENTCOLOUR,
                    SC_ELEMENT_SELECTION_TEXT as usize,
                    0
                ),
                (
                    SCI_RESETELEMENTCOLOUR,
                    SC_ELEMENT_SELECTION_INACTIVE_TEXT as usize,
                    0
                ),
            ]
        );
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
        line_count: isize,
        text_width_texts: Vec<Vec<u8>>,
        calls: Vec<(u32, usize, isize)>,
    }

    const TEST_DEFAULT_BACKGROUND: isize = 0x00AB_CDEF;

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

        /// Every direct call, with `SCI_TEXTWIDTH`'s string pointer zeroed so it can be compared.
        fn calls(&self) -> Vec<(u32, usize, isize)> {
            self.state.lock().unwrap().calls.clone()
        }

        fn set_line_count(&self, line_count: isize) {
            self.state.lock().unwrap().line_count = line_count;
        }

        fn text_width_texts(&self) -> Vec<Vec<u8>> {
            self.state.lock().unwrap().text_width_texts.clone()
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
        let recorded_lparam = if message == SCI_TEXTWIDTH { 0 } else { lparam };
        state.calls.push((message, wparam, recorded_lparam));
        match message {
            SCI_GETLINECOUNT => state.line_count,
            SCI_STYLEGETBACK => TEST_DEFAULT_BACKGROUND,
            // Ten pixels per measured character keeps the expected widths readable.
            SCI_TEXTWIDTH => {
                let bytes = unsafe { std::ffi::CStr::from_ptr(lparam as *const std::ffi::c_char) }
                    .to_bytes()
                    .to_vec();
                let width = bytes.len() as isize * 10;
                state.text_width_texts.push(bytes);
                width
            }
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
