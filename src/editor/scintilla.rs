use crate::editor::scintilla_constants::{
    SC_CP_UTF8, SCI_ADDREFDOCUMENT, SCI_BEGINUNDOACTION, SCI_CREATEDOCUMENT, SCI_ENDUNDOACTION,
    SCI_GETDIRECTFUNCTION, SCI_GETDIRECTPOINTER, SCI_GETDOCPOINTER, SCI_GETTEXT, SCI_GETTEXTLENGTH,
    SCI_RELEASEDOCUMENT, SCI_SEARCHINTARGET, SCI_SETCODEPAGE, SCI_SETDOCPOINTER, SCI_SETSAVEPOINT,
    SCI_SETSEARCHFLAGS, SCI_SETTARGETRANGE, SCI_SETTEXT,
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

    pub fn set_save_point(&self) {
        let _ = self.endpoint.send_direct_if_alive(SCI_SETSAVEPOINT, 0, 0);
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
    fn test_fixture(direct_fn: SciFnDirect, direct_ptr: isize) -> Self {
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
        SCI_ADDREFDOCUMENT, SCI_RELEASEDOCUMENT, SCI_SEARCHINTARGET, SCI_SETSEARCHFLAGS,
        SCI_SETTARGETRANGE,
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

    #[derive(Default)]
    struct TestDirectState {
        messages: Vec<u32>,
        responses: VecDeque<isize>,
        target_range: Option<(usize, isize)>,
        search_flags: Option<usize>,
        search_needle: Option<Vec<u8>>,
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
            _ => state.responses.pop_front().unwrap_or(0),
        }
    }
}
