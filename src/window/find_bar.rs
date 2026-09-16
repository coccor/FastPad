use std::ops::Range;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchDirection {
    Forward,
    Backward,
}

/// Wrap-once-then-stop search progression: bookkeeping only, independent of what actually looks
/// for the query in a given range. `next_range` is a pure, dependency-free reference
/// implementation driven by a plain string, used both for unit testing this algorithm and by the
/// window layer for the (rare) case a plain string is already in hand; live document navigation
/// instead drives the same shape of search via `Editor::search_in_target`, never materializing
/// the full document text.
#[derive(Debug)]
pub struct SearchState {
    query: String,
    direction: SearchDirection,
    origin: usize,
    cursor: usize,
    wrapped: bool,
}

impl SearchState {
    pub fn new(query: &str, direction: SearchDirection, start: usize) -> Self {
        Self {
            query: query.to_owned(),
            direction,
            origin: start,
            cursor: start,
            wrapped: false,
        }
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn direction(&self) -> SearchDirection {
        self.direction
    }

    /// Bounds for a leftmost-match backend (`str::find`/`str::rfind` over a normally-ordered
    /// slice): always `start <= end`. Used only by the pure `next_range` reference below.
    fn str_bounds(&self, len: usize) -> Range<usize> {
        match self.direction {
            SearchDirection::Forward if !self.wrapped => self.cursor..len,
            SearchDirection::Forward => self.cursor..self.origin,
            SearchDirection::Backward if !self.wrapped => 0..self.cursor,
            SearchDirection::Backward => self.origin..self.cursor,
        }
    }

    /// Bounds for `Editor::search_in_target`: Scintilla treats a target range with `start > end`
    /// as a backward search from `start` toward `end`, so `Backward` deliberately builds a
    /// reversed range here (unlike `str_bounds` above, which normal-orders it for `rfind`).
    fn scintilla_bounds(&self, doc_len: usize) -> Range<usize> {
        match self.direction {
            SearchDirection::Forward if !self.wrapped => self.cursor..doc_len,
            SearchDirection::Forward => self.cursor..self.origin,
            SearchDirection::Backward if !self.wrapped => self.cursor..0,
            SearchDirection::Backward => self.cursor..self.origin,
        }
    }

    fn record_match(&mut self, found: Range<usize>) {
        match self.direction {
            SearchDirection::Forward => self.cursor = found.end,
            SearchDirection::Backward => self.cursor = found.start,
        }
    }

    fn record_miss(&mut self, len: usize) {
        if self.wrapped {
            return;
        }
        self.wrapped = true;
        self.cursor = match self.direction {
            SearchDirection::Forward => 0,
            SearchDirection::Backward => len,
        };
    }

    /// Pure reference implementation of the wrap progression over an in-memory string. Never used
    /// for live editor navigation (see the struct docs); exists for testing and for any caller
    /// that already holds the text (e.g. a prefilled query match against a short selection).
    pub fn next_range(&mut self, haystack: &str) -> Option<Range<usize>> {
        let query = self.query.clone();
        let direction = self.direction;
        let find = |range: Range<usize>| -> Option<Range<usize>> {
            let slice = haystack.get(range.clone())?;
            let relative = match direction {
                SearchDirection::Forward => slice.find(&query),
                SearchDirection::Backward => slice.rfind(&query),
            }?;
            let start = range.start + relative;
            Some(start..start + query.len())
        };

        let bounds = self.str_bounds(haystack.len());
        if let Some(found) = find(bounds) {
            self.record_match(found.clone());
            return Some(found);
        }
        if self.wrapped {
            return None;
        }
        self.record_miss(haystack.len());
        let bounds = self.str_bounds(haystack.len());
        let found = find(bounds)?;
        self.record_match(found.clone());
        Some(found)
    }

    /// Drives the same wrap-once progression against a live Scintilla document via
    /// `Editor::search_in_target`, never materializing the full document text.
    pub(crate) fn next_editor_match(
        &mut self,
        editor: &crate::editor::Editor,
        flags: u32,
        doc_len: usize,
    ) -> crate::Result<Option<Range<usize>>> {
        if self.query.is_empty() {
            return Ok(None);
        }
        let bounds = self.scintilla_bounds(doc_len);
        if let Some(found) = editor.search_in_target(&self.query, bounds, flags)? {
            self.record_match(found.clone());
            return Ok(Some(found));
        }
        if self.wrapped {
            return Ok(None);
        }
        self.record_miss(doc_len);
        let bounds = self.scintilla_bounds(doc_len);
        let found = editor.search_in_target(&self.query, bounds, flags)?;
        if let Some(found) = &found {
            self.record_match(found.clone());
        }
        Ok(found)
    }
}

// --- Window integration: native child controls hosting Find/Replace ---

use crate::platform::{last_error, wide_null};
use crate::window::palette::Palette;
use crate::window::panel::{create_child, create_panel, fill, inset, scale, text_height};
use std::cell::Cell;
use std::rc::Rc;
use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DT_CENTER, DT_END_ELLIPSIS, DT_NOPREFIX, DT_SINGLELINE,
    DT_VCENTER, DeleteObject, DrawTextW, EndPaint, HBRUSH, HDC, HFONT, InvalidateRect, PAINTSTRUCT,
    RDW_ALLCHILDREN, RDW_ERASE, RDW_INVALIDATE, RedrawWindow, SelectObject, SetBkColor, SetBkMode,
    SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::UI::Controls::{
    EM_GETMARGINS, EM_REPLACESEL, EM_SETSEL, EM_UNDO, WM_MOUSELEAVE,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetFocus, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_ESCAPE,
    VK_RETURN, VK_SHIFT,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, ES_AUTOHSCROLL, GetClientRect, GetParent, GetWindowTextLengthW, GetWindowTextW,
    HWND_TOP, MoveWindow, SW_HIDE, SW_SHOWNA, SWP_NOACTIVATE, SWP_SHOWWINDOW, SendMessageW,
    SetWindowPos, SetWindowTextW, ShowWindow, WM_CHAR, WM_CLEAR, WM_CUT, WM_GETFONT, WM_KEYDOWN,
    WM_KILLFOCUS, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCDESTROY, WM_PAINT, WM_PASTE, WM_SETFOCUS,
    WM_SETFONT, WM_SETTEXT, WM_UNDO, WS_CHILD, WS_TABSTOP, WS_VISIBLE,
};

const BAR_HEIGHT_AT_96_DPI: i32 = 36;
const FIELD_HEIGHT_AT_96_DPI: i32 = 28;
const PADDING_AT_96_DPI: i32 = 6;
const FIELD_TEXT_INSET_AT_96_DPI: i32 = 8;

/// Height of the bar, reserved above the editor whenever it's visible.
pub(crate) const fn find_bar_height(dpi: u32) -> i32 {
    scale(BAR_HEIGHT_AT_96_DPI, dpi)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FindBarMode {
    Find,
    Replace,
}

/// One painted field box and the borderless `Edit` centered inside it, in bar coordinates.
#[derive(Clone, Copy)]
struct FieldLayout {
    field: RECT,
    edit: RECT,
}

/// The bar's parts in bar coordinates: the query field, in Replace mode the replacement field
/// beside it (each half the width), and a square close button at the right end.
struct BarLayout {
    query: FieldLayout,
    replace: Option<FieldLayout>,
    close: RECT,
}

fn bar_layout(width: i32, dpi: u32, text_height: i32, mode: FindBarMode) -> BarLayout {
    let padding = scale(PADDING_AT_96_DPI, dpi);
    let field_height = scale(FIELD_HEIGHT_AT_96_DPI, dpi);
    let top = (find_bar_height(dpi) - 1 - field_height) / 2;
    let close = RECT {
        left: (width - padding - field_height).max(0),
        top,
        right: (width - padding).max(0),
        bottom: top + field_height,
    };
    let (query, replace) = field_layouts(close.left, dpi, text_height, mode, top);
    BarLayout {
        query,
        replace,
        close,
    }
}

/// The fields across `width` (the right padding included), starting `top` pixels down the bar.
fn field_layouts(
    width: i32,
    dpi: u32,
    text_height: i32,
    mode: FindBarMode,
    top: i32,
) -> (FieldLayout, Option<FieldLayout>) {
    let padding = scale(PADDING_AT_96_DPI, dpi);
    let field_height = scale(FIELD_HEIGHT_AT_96_DPI, dpi);
    let inset_x = scale(FIELD_TEXT_INSET_AT_96_DPI, dpi);
    let text_height = text_height.clamp(1, (field_height - 2).max(1));
    let field = |left: i32, right: i32| {
        let field = RECT {
            left,
            top,
            right: right.max(left),
            bottom: top + field_height,
        };
        let edit_top = top + (field_height - text_height) / 2;
        FieldLayout {
            field,
            edit: RECT {
                left: left + inset_x,
                top: edit_top,
                right: (field.right - inset_x).max(left + inset_x),
                bottom: edit_top + text_height,
            },
        }
    };
    match mode {
        FindBarMode::Find => (field(padding, width - padding), None),
        FindBarMode::Replace => {
            let half = (width - 3 * padding) / 2;
            (
                field(padding, padding + half),
                // An odd leftover pixel stays at the right edge so both fields match.
                Some(field(2 * padding + half, 2 * padding + 2 * half)),
            )
        }
    }
}

/// A painted band in the strip colors hosting two borderless `Edit` controls (query, replacement),
/// shown/hidden/positioned by the main window. Each field sits in a box in the editor's colors,
/// outlined with the accent while it has the focus.
#[derive(Debug)]
pub(crate) struct FindBar {
    panel: HWND,
    query_edit: HWND,
    replace_edit: HWND,
    mode: FindBarMode,
    visible: bool,
    colors: Palette,
    field_brush: HBRUSH,
    close_hovered: Cell<bool>,
}

impl FindBar {
    pub(crate) fn create(parent: HWND) -> crate::Result<Self> {
        let panel = create_panel(parent)?;
        let fields = (|| {
            let query_edit = create_edit_child(panel)?;
            let replace_edit = create_edit_child(panel)?;
            install_field_hook(query_edit, parent, FindField::Query)?;
            install_field_hook(replace_edit, parent, FindField::Replace)?;
            Ok((query_edit, replace_edit))
        })();
        let (query_edit, replace_edit) = match fields {
            Ok(fields) => fields,
            Err(error) => {
                unsafe {
                    DestroyWindow(panel);
                }
                return Err(error);
            }
        };
        let colors = Palette::neutral();
        Ok(Self {
            panel,
            query_edit,
            replace_edit,
            mode: FindBarMode::Find,
            visible: false,
            colors,
            field_brush: unsafe { CreateSolidBrush(colors.editor_background) },
            close_hovered: Cell::new(false),
        })
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.visible
    }

    pub(crate) fn owns(&self, hwnd: HWND) -> bool {
        !hwnd.is_null()
            && (hwnd == self.panel || hwnd == self.query_edit || hwnd == self.replace_edit)
    }

    pub(crate) fn query_text(&self) -> String {
        control_text(self.query_edit)
    }

    pub(crate) fn replace_text(&self) -> String {
        control_text(self.replace_edit)
    }

    pub(crate) fn show(&mut self, mode: FindBarMode, prefill: Option<&str>, colors: Palette) {
        self.set_colors(colors);
        self.mode = mode;
        self.visible = true;
        if let Some(text) = prefill {
            set_control_text(self.query_edit, text);
        }
        unsafe {
            ShowWindow(
                self.replace_edit,
                if mode == FindBarMode::Replace {
                    SW_SHOWNA
                } else {
                    SW_HIDE
                },
            );
        }
    }

    pub(crate) fn hide(&mut self) {
        self.visible = false;
        unsafe {
            ShowWindow(self.panel, SW_HIDE);
        }
    }

    /// Recolors for a theme change; the caller repaints with `invalidate`.
    pub(crate) fn set_colors(&mut self, colors: Palette) {
        if colors == self.colors {
            return;
        }
        unsafe {
            DeleteObject(self.field_brush);
            self.field_brush = CreateSolidBrush(colors.editor_background);
        }
        self.colors = colors;
    }

    pub(crate) fn invalidate(&self) {
        unsafe {
            RedrawWindow(
                self.panel,
                std::ptr::null(),
                std::ptr::null_mut(),
                RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
            );
        }
    }

    /// Places the bar across `width` at `top` and its fields inside it.
    pub(crate) fn layout(&self, width: i32, top: i32, dpi: u32, font: HFONT) {
        if !self.visible {
            return;
        }
        unsafe {
            if !font.is_null() {
                SendMessageW(self.query_edit, WM_SETFONT, font as WPARAM, 0);
                SendMessageW(self.replace_edit, WM_SETFONT, font as WPARAM, 0);
            }
        }
        let BarLayout { query, replace, .. } =
            bar_layout(width, dpi, text_height(self.query_edit, font), self.mode);
        let move_to = |hwnd, rect: RECT| unsafe {
            MoveWindow(
                hwnd,
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                1,
            );
        };
        move_to(self.query_edit, query.edit);
        if let Some(replace) = replace {
            move_to(self.replace_edit, replace.edit);
        }
        unsafe {
            SetWindowPos(
                self.panel,
                HWND_TOP,
                0,
                top,
                width.max(0),
                find_bar_height(dpi),
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            InvalidateRect(self.panel, std::ptr::null(), 0);
        }
    }

    pub(crate) fn focus_query(&self) {
        unsafe {
            SetFocus(self.query_edit);
            select_all(self.query_edit);
        }
    }

    /// `WM_CTLCOLOREDIT` for either field.
    pub(crate) fn control_color(&self, dc: HDC) -> HBRUSH {
        unsafe {
            SetTextColor(dc, self.colors.editor_foreground);
            SetBkColor(dc, self.colors.editor_background);
        }
        self.field_brush
    }

    /// `WM_MOUSEMOVE`, `WM_MOUSELEAVE` and `WM_LBUTTONUP` on the bar; returns true when the close
    /// button was clicked.
    pub(crate) fn pointer(&self, message: u32, lparam: LPARAM) -> bool {
        let layout = self.current_layout();
        let x = (lparam as u32 & 0xffff) as u16 as i16 as i32;
        let y = ((lparam as u32 >> 16) & 0xffff) as u16 as i16 as i32;
        let over_close = message != WM_MOUSELEAVE
            && x >= layout.close.left
            && x < layout.close.right
            && y >= layout.close.top
            && y < layout.close.bottom;
        if message == WM_MOUSEMOVE {
            let mut track = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: self.panel,
                dwHoverTime: 0,
            };
            unsafe {
                TrackMouseEvent(&mut track);
            }
        }
        if self.close_hovered.replace(over_close) != over_close {
            unsafe {
                InvalidateRect(self.panel, &layout.close, 0);
            }
        }
        message == WM_LBUTTONUP && over_close
    }

    fn current_layout(&self) -> BarLayout {
        let mut client = RECT::default();
        unsafe {
            GetClientRect(self.panel, &mut client);
        }
        let dpi = unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForWindow(self.panel) }.max(96);
        bar_layout(client.right, dpi, 0, self.mode)
    }

    /// `WM_PAINT` for an empty field: its placeholder in the muted color where typed text starts.
    /// Returns false for a field that is not this bar's, which then paints normally.
    pub(crate) fn paint_placeholder(&self, edit: HWND) -> bool {
        let Some(placeholder) = self.placeholder(edit) else {
            return false;
        };
        let mut paint = PAINTSTRUCT::default();
        let dc = unsafe { BeginPaint(edit, &mut paint) };
        if dc.is_null() {
            return true;
        }
        unsafe {
            let mut client = RECT::default();
            GetClientRect(edit, &mut client);
            fill(dc, client, self.colors.editor_background);
            let font = SendMessageW(edit, WM_GETFONT, 0, 0);
            let previous = (font != 0).then(|| SelectObject(dc, font as _));
            // Typed text starts after the Edit's left margin (the low word).
            client.left += (SendMessageW(edit, EM_GETMARGINS, 0, 0) & 0xffff) as i32;
            SetBkMode(dc, TRANSPARENT as i32);
            SetTextColor(dc, self.colors.muted_foreground);
            let mut text = placeholder.encode_utf16().collect::<Vec<_>>();
            DrawTextW(
                dc,
                text.as_mut_ptr(),
                text.len() as i32,
                &mut client,
                DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
            );
            if let Some(previous) = previous {
                SelectObject(dc, previous);
            }
            EndPaint(edit, &paint);
        }
        true
    }

    /// What an empty field shows, or `None` for a control that is not one of this bar's fields.
    pub(crate) fn placeholder(&self, edit: HWND) -> Option<&'static str> {
        if edit == self.query_edit {
            Some("Find")
        } else if edit == self.replace_edit {
            Some("Replace")
        } else {
            None
        }
    }

    /// `WM_PAINT` for the bar: strip background, a hairline above the editor, each visible
    /// field's box (outlined with the accent while it has the focus), and the close button.
    pub(crate) fn paint_panel(&self, panel: HWND, glyph_font: HFONT) {
        let mut paint = PAINTSTRUCT::default();
        let dc = unsafe { BeginPaint(panel, &mut paint) };
        if dc.is_null() {
            return;
        }
        let mut client = RECT::default();
        unsafe {
            GetClientRect(panel, &mut client);
        }
        let dpi = unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForWindow(panel) }.max(96);
        let colors = self.colors;
        let BarLayout {
            query,
            replace,
            close,
        } = bar_layout(client.right, dpi, 0, self.mode);
        let focus = unsafe { GetFocus() };
        unsafe {
            fill(dc, client, colors.strip_background);
            fill(
                dc,
                RECT {
                    top: client.bottom - 1,
                    ..client
                },
                colors.pressed_background,
            );
            for (layout, edit) in [(Some(query), self.query_edit), (replace, self.replace_edit)] {
                let Some(layout) = layout else {
                    continue;
                };
                let outline = if focus == edit {
                    colors.selection_background
                } else {
                    colors.pressed_background
                };
                fill(dc, layout.field, outline);
                fill(dc, inset(layout.field, 1), colors.editor_background);
            }
            let hovered = self.close_hovered.get();
            if hovered {
                fill(dc, close, colors.hover_background);
            }
            if !glyph_font.is_null() {
                let previous = SelectObject(dc, glyph_font as _);
                SetBkMode(dc, TRANSPARENT as i32);
                SetTextColor(
                    dc,
                    if hovered {
                        colors.hover_foreground
                    } else {
                        colors.muted_foreground
                    },
                );
                let mut glyph = crate::window::titlebar::GLYPH_CLOSE
                    .encode_utf16()
                    .collect::<Vec<_>>();
                let mut rect = close;
                DrawTextW(
                    dc,
                    glyph.as_mut_ptr(),
                    glyph.len() as i32,
                    &mut rect,
                    DT_SINGLELINE | DT_CENTER | DT_VCENTER | DT_NOPREFIX,
                );
                SelectObject(dc, previous);
            }
            EndPaint(panel, &paint);
        }
    }

    #[cfg(test)]
    #[allow(
        dead_code,
        reason = "consumed by the source-linked editing integration target"
    )]
    pub(crate) fn query_hwnd(&self) -> HWND {
        self.query_edit
    }

    #[cfg(test)]
    #[allow(
        dead_code,
        reason = "consumed by the source-linked editing integration target"
    )]
    pub(crate) fn replace_hwnd(&self) -> HWND {
        self.replace_edit
    }

    #[cfg(test)]
    pub(crate) fn panel_hwnd(&self) -> HWND {
        self.panel
    }
}

impl Drop for FindBar {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.field_brush);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FindField {
    Query,
    Replace,
}

struct FindFieldHook {
    parent: HWND,
    field: FindField,
}
const FIND_FIELD_HOOK_ID: usize = 0x4650_4644;

fn install_field_hook(field_hwnd: HWND, parent: HWND, field: FindField) -> crate::Result<()> {
    let data = Rc::into_raw(Rc::new(FindFieldHook { parent, field })) as usize;
    if unsafe { SetWindowSubclass(field_hwnd, Some(find_field_proc), FIND_FIELD_HOOK_ID, data) }
        == 0
    {
        unsafe {
            drop(Rc::from_raw(data as *const FindFieldHook));
        }
        return Err(last_error());
    }
    Ok(())
}

unsafe extern "system" fn find_field_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    ref_data: usize,
) -> isize {
    let raw = ref_data as *const FindFieldHook;
    unsafe {
        Rc::increment_strong_count(raw);
    }
    let hook = unsafe { Rc::from_raw(raw) };
    if message == WM_NCDESTROY {
        unsafe {
            RemoveWindowSubclass(hwnd, Some(find_field_proc), FIND_FIELD_HOOK_ID);
            Rc::decrement_strong_count(raw);
        }
    }
    // A single-line Edit beeps at Enter and Escape characters; both are handled on key down.
    if message == WM_CHAR && matches!(wparam as u16, 0x0d | 0x1b) {
        return 0;
    }
    if message == WM_PAINT
        && unsafe { GetWindowTextLengthW(hwnd) } == 0
        && super::main_window::paint_find_placeholder(hook.parent, hwnd)
    {
        return 0;
    }
    // The Edit repaints only the text it changes; the placeholder must go (or come back) whole.
    let edits_text = matches!(
        message,
        WM_CHAR
            | WM_KEYDOWN
            | WM_PASTE
            | WM_CUT
            | WM_CLEAR
            | WM_UNDO
            | WM_SETTEXT
            | EM_UNDO
            | EM_REPLACESEL
    );
    let was_empty = edits_text && unsafe { GetWindowTextLengthW(hwnd) } == 0;
    let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
    if edits_text && was_empty != (unsafe { GetWindowTextLengthW(hwnd) } == 0) {
        unsafe {
            InvalidateRect(hwnd, std::ptr::null(), 1);
        }
    }
    match message {
        WM_KEYDOWN => {
            let shift = unsafe { GetAsyncKeyState(VK_SHIFT as i32) } < 0;
            let key = wparam as u16;
            if key == VK_RETURN {
                match hook.field {
                    FindField::Query if shift => super::main_window::find_previous(hook.parent),
                    FindField::Query => super::main_window::find_next(hook.parent),
                    FindField::Replace if shift => {
                        super::main_window::replace_all_matches(hook.parent)
                    }
                    FindField::Replace => super::main_window::replace_current(hook.parent),
                }
            } else if key == VK_ESCAPE {
                super::main_window::close_find_bar(hook.parent);
            }
        }
        // The focused field carries the accent outline the bar paints.
        WM_SETFOCUS | WM_KILLFOCUS => unsafe {
            InvalidateRect(GetParent(hwnd), std::ptr::null(), 0);
        },
        _ => {}
    }
    result
}

fn create_edit_child(panel: HWND) -> crate::Result<HWND> {
    create_child(
        panel,
        &wide_null("Edit"),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | (ES_AUTOHSCROLL as u32),
    )
}

fn control_text(hwnd: HWND) -> String {
    unsafe {
        let length = GetWindowTextLengthW(hwnd);
        if length <= 0 {
            return String::new();
        }
        let mut buffer = vec![0u16; length as usize + 1];
        let copied = GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
        buffer.truncate(copied.max(0) as usize);
        String::from_utf16_lossy(&buffer)
    }
}

fn set_control_text(hwnd: HWND, text: &str) {
    let wide = wide_null(text);
    unsafe {
        SetWindowTextW(hwnd, wide.as_ptr());
    }
}

fn select_all(hwnd: HWND) {
    unsafe {
        SendMessageW(hwnd, EM_SETSEL, 0, -1);
    }
}

#[cfg(test)]
mod tests {
    use super::{SearchDirection, SearchState};
    use crate::editor::Editor;
    use crate::editor::scintilla_constants::{
        SCI_SEARCHINTARGET, SCI_SETSEARCHFLAGS, SCI_SETTARGETRANGE,
    };
    use std::collections::VecDeque;
    use std::sync::Mutex;

    #[test]
    fn find_fields_center_their_text_and_replace_mode_splits_the_bar_without_overlap() {
        // Break caught: field text stuck to the top of its box, or a replacement field that
        // overlaps the query field or runs past the bar.
        use super::{FindBarMode, bar_layout, find_bar_height};
        for dpi in [96, 144] {
            let height = find_bar_height(dpi);
            let layout = bar_layout(800, dpi, 16, FindBarMode::Find);
            let (query, replace, close) = (layout.query, layout.replace, layout.close);
            assert!(replace.is_none());
            // Break caught: a close button overlapping the field or hanging off the bar.
            assert!(query.field.right < close.left && close.right < 800);
            assert_eq!(close.right - close.left, close.bottom - close.top);
            assert!(query.field.top > 0 && query.field.bottom < height - 1);
            let field_middle = (query.field.top + query.field.bottom) / 2;
            let edit_middle = (query.edit.top + query.edit.bottom) / 2;
            assert!((field_middle - edit_middle).abs() <= 1);

            let layout = bar_layout(800, dpi, 16, FindBarMode::Replace);
            let (query, replace) = (layout.query, layout.replace.unwrap());
            assert!(query.field.right < replace.field.left);
            assert!(replace.field.right < layout.close.left);
            assert_eq!(
                query.field.right - query.field.left,
                replace.field.right - replace.field.left
            );
        }
    }

    #[test]
    fn next_match_wraps_once_then_stops() {
        let mut state = SearchState::new("one", SearchDirection::Forward, 8);
        assert_eq!(state.next_range("one two one"), Some(8..11));
        assert_eq!(state.next_range("one two one"), Some(0..3));
        assert_eq!(state.next_range("one two one"), None);
    }

    #[test]
    fn backward_search_wraps_once_then_stops() {
        // Break caught: reusing forward-only bounds for Backward would search the wrong half of
        // the string, or never terminate once wrapped.
        let mut state = SearchState::new("one", SearchDirection::Backward, 3);
        assert_eq!(state.next_range("one two one"), Some(0..3));
        assert_eq!(state.next_range("one two one"), Some(8..11));
        assert_eq!(state.next_range("one two one"), None);
    }

    #[test]
    fn absent_query_never_matches() {
        let mut state = SearchState::new("missing", SearchDirection::Forward, 0);
        assert_eq!(state.next_range("one two one"), None);
        assert_eq!(state.next_range("one two one"), None);
    }

    #[test]
    fn repeated_forward_searches_over_a_single_match_stop_after_the_first_repeat() {
        // Break caught: not tracking `wrapped` across calls lets a lone match be reported forever.
        let mut state = SearchState::new("two", SearchDirection::Forward, 0);
        assert_eq!(state.next_range("one two one"), Some(4..7));
        assert_eq!(state.next_range("one two one"), None);
    }

    #[derive(Default)]
    struct TargetLog {
        responses: VecDeque<isize>,
        ranges: Vec<(usize, isize)>,
    }

    unsafe extern "C" fn target_range_stub(
        direct_ptr: isize,
        message: u32,
        wparam: usize,
        lparam: isize,
    ) -> isize {
        let shared = unsafe { &*(direct_ptr as *const Mutex<TargetLog>) };
        let mut log = shared.lock().unwrap();
        match message {
            SCI_SETTARGETRANGE => {
                log.ranges.push((wparam, lparam));
                0
            }
            SCI_SETSEARCHFLAGS => 0,
            SCI_SEARCHINTARGET => log.responses.pop_front().unwrap_or(-1),
            _ => 0,
        }
    }

    #[test]
    fn next_editor_match_drives_search_in_target_with_evolving_bounds_and_wraps_once() {
        // Break caught: reusing `str_bounds`' forward-ordered shape (instead of `scintilla_bounds`'
        // reversed one for Backward), or not retrying once after the first miss, would search the
        // wrong range or never find the wrapped match.
        let log = Mutex::new(TargetLog {
            responses: VecDeque::from([8_isize, -1, 0, -1]),
            ranges: Vec::new(),
        });
        let editor =
            Editor::test_fixture(target_range_stub, &log as *const Mutex<TargetLog> as isize);
        let mut state = SearchState::new("one", SearchDirection::Forward, 8);

        assert_eq!(
            state.next_editor_match(&editor, 0, 11).unwrap(),
            Some(8..11)
        );
        assert_eq!(state.next_editor_match(&editor, 0, 11).unwrap(), Some(0..3));
        assert_eq!(state.next_editor_match(&editor, 0, 11).unwrap(), None);

        assert_eq!(
            log.lock().unwrap().ranges,
            vec![(8, 11), (11, 11), (0, 8), (3, 8)]
        );
    }

    #[test]
    fn next_editor_match_with_an_empty_query_never_calls_scintilla() {
        let log = Mutex::new(TargetLog::default());
        let editor =
            Editor::test_fixture(target_range_stub, &log as *const Mutex<TargetLog> as isize);
        let mut state = SearchState::new("", SearchDirection::Forward, 0);

        assert_eq!(state.next_editor_match(&editor, 0, 11).unwrap(), None);
        assert!(log.lock().unwrap().ranges.is_empty());
    }
}
