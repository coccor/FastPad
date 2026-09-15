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

#[cfg(windows)]
use crate::platform::{last_error, wide_null};
#[cfg(windows)]
use std::rc::Rc;
use windows_sys::Win32::Foundation::HWND;
#[cfg(windows)]
use windows_sys::Win32::Foundation::{LPARAM, WPARAM};
#[cfg(windows)]
use windows_sys::Win32::UI::Controls::EM_SETSEL;
#[cfg(windows)]
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SetFocus, VK_ESCAPE, VK_RETURN, VK_SHIFT,
};
#[cfg(windows)]
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, ES_AUTOHSCROLL, GetWindowTextLengthW, GetWindowTextW, MoveWindow, SW_HIDE,
    SW_SHOWNA, SendMessageW, SetWindowTextW, ShowWindow, WM_KEYDOWN, WM_NCDESTROY, WS_BORDER,
    WS_CHILD, WS_TABSTOP,
};

/// Fixed height of the bar, reserved above the editor whenever it's visible.
pub(crate) const FIND_BAR_HEIGHT: i32 = 26;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FindBarMode {
    Find,
    Replace,
}

/// Two plain Win32 `Edit` child controls (query, replacement) parented directly to the main
/// window, shown/hidden/positioned by the caller. No custom window class: consistent with the
/// project's "no UI framework" constraint, and simple enough not to need one.
#[derive(Debug)]
pub(crate) struct FindBar {
    query_edit: HWND,
    replace_edit: HWND,
    mode: FindBarMode,
    visible: bool,
}

impl FindBar {
    #[cfg(windows)]
    pub(crate) fn create(parent: HWND) -> crate::Result<Self> {
        let query_edit = create_edit_child(parent)?;
        let replace_edit = create_edit_child(parent)?;
        install_field_hook(query_edit, parent, FindField::Query)?;
        install_field_hook(replace_edit, parent, FindField::Replace)?;
        Ok(Self {
            query_edit,
            replace_edit,
            mode: FindBarMode::Find,
            visible: false,
        })
    }

    #[cfg(not(windows))]
    pub(crate) fn create(_parent: HWND) -> crate::Result<Self> {
        Err(crate::FastPadError::Invariant(
            "Scintilla editor is only supported on Windows",
        ))
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.visible
    }

    #[cfg(windows)]
    pub(crate) fn query_text(&self) -> String {
        control_text(self.query_edit)
    }

    #[cfg(windows)]
    pub(crate) fn replace_text(&self) -> String {
        control_text(self.replace_edit)
    }

    #[cfg(windows)]
    pub(crate) fn show(&mut self, mode: FindBarMode, prefill: Option<&str>) {
        self.mode = mode;
        self.visible = true;
        if let Some(text) = prefill {
            set_control_text(self.query_edit, text);
        }
        unsafe {
            ShowWindow(self.query_edit, SW_SHOWNA);
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

    #[cfg(windows)]
    pub(crate) fn hide(&mut self) {
        self.visible = false;
        unsafe {
            ShowWindow(self.query_edit, SW_HIDE);
            ShowWindow(self.replace_edit, SW_HIDE);
        }
    }

    #[cfg(windows)]
    pub(crate) fn layout(&self, width: i32, top: i32) {
        if !self.visible {
            return;
        }
        let inner_top = top + 3;
        let inner_height = (FIND_BAR_HEIGHT - 6).max(0);
        let replace_visible = self.mode == FindBarMode::Replace;
        let field_width = if replace_visible {
            ((width - 12) / 2).max(0)
        } else {
            (width - 8).max(0)
        };
        unsafe {
            MoveWindow(self.query_edit, 4, inner_top, field_width, inner_height, 1);
            if replace_visible {
                MoveWindow(
                    self.replace_edit,
                    8 + field_width,
                    inner_top,
                    field_width,
                    inner_height,
                    1,
                );
            }
        }
    }

    #[cfg(windows)]
    pub(crate) fn focus_query(&self) {
        unsafe {
            SetFocus(self.query_edit);
            select_all(self.query_edit);
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

#[cfg(windows)]
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

#[cfg(windows)]
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
    let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
    if message == WM_KEYDOWN {
        let shift = unsafe { GetAsyncKeyState(VK_SHIFT as i32) } < 0;
        let key = wparam as u16;
        if key == VK_RETURN {
            match hook.field {
                FindField::Query if shift => super::main_window::find_previous(hook.parent),
                FindField::Query => super::main_window::find_next(hook.parent),
                FindField::Replace if shift => super::main_window::replace_all_matches(hook.parent),
                FindField::Replace => super::main_window::replace_current(hook.parent),
            }
        } else if key == VK_ESCAPE {
            super::main_window::close_find_bar(hook.parent);
        }
    }
    result
}

#[cfg(windows)]
fn create_edit_child(parent: HWND) -> crate::Result<HWND> {
    let class_name = wide_null("Edit");
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            std::ptr::null(),
            WS_CHILD | WS_TABSTOP | WS_BORDER | (ES_AUTOHSCROLL as u32),
            0,
            0,
            0,
            0,
            parent,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
        )
    };
    if hwnd.is_null() {
        Err(last_error())
    } else {
        Ok(hwnd)
    }
}

#[cfg(windows)]
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

#[cfg(windows)]
fn set_control_text(hwnd: HWND, text: &str) {
    let wide = wide_null(text);
    unsafe {
        SetWindowTextW(hwnd, wide.as_ptr());
    }
}

#[cfg(windows)]
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
