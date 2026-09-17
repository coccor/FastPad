//! MSAA for the preview: a document object named "Markdown preview" whose children are the links
//! currently on screen. It reads the view's link snapshot (safe from any thread) and activates a
//! link by posting to the preview window, which follows it on the UI thread.

use crate::preview::view::VisibleLink;
use crate::window::WM_FASTPAD_PREVIEW_ACTIVATE;
use crate::window::accessibility::{
    AccessibleVtable, IID_IACCESSIBLE, IID_IDISPATCH, IID_IUNKNOWN, RawVariant, VariantValue,
    accessible_get_help_topic, accessible_get_ids_of_names, accessible_get_parent,
    accessible_get_type_info, accessible_get_type_info_count, accessible_invoke, allocate_bstr,
    guid_eq,
};
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, RwLock};
use windows_sys::Win32::Foundation::{
    E_FAIL, E_INVALIDARG, E_NOINTERFACE, E_NOTIMPL, HWND, LRESULT, POINT, RECT, S_FALSE, S_OK,
    WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{ClientToScreen, ScreenToClient};
use windows_sys::Win32::UI::Accessibility::{
    LresultFromObject, ROLE_SYSTEM_DOCUMENT, ROLE_SYSTEM_LINK,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GUITHREADINFO, GetClientRect, GetGUIThreadInfo, GetWindowRect, GetWindowThreadProcessId,
    PostMessageW,
};
use windows_sys::core::{BSTR, GUID, HRESULT};

const STATE_SYSTEM_FOCUSED: u32 = 0x0000_0004;
const STATE_SYSTEM_READONLY: u32 = 0x0000_0040;
const STATE_SYSTEM_FOCUSABLE: u32 = 0x0010_0000;
const STATE_SYSTEM_LINKED: u32 = 0x0040_0000;

const EMPTY_RECT: RECT = RECT {
    left: 0,
    top: 0,
    right: 0,
    bottom: 0,
};

#[repr(C)]
struct PreviewAccessible {
    vtable: &'static AccessibleVtable,
    references: AtomicU32,
    hwnd: HWND,
    links: Arc<RwLock<Vec<VisibleLink>>>,
}

pub(crate) static PREVIEW_VTABLE: AccessibleVtable = AccessibleVtable {
    query_interface,
    add_ref,
    release,
    get_type_info_count: accessible_get_type_info_count,
    get_type_info: accessible_get_type_info,
    get_ids_of_names: accessible_get_ids_of_names,
    invoke: accessible_invoke,
    get_acc_parent: accessible_get_parent,
    get_acc_child_count: child_count,
    get_acc_child: child,
    get_acc_name: name,
    get_acc_value: value,
    get_acc_description: empty_text,
    get_acc_role: role,
    get_acc_state: state,
    get_acc_help: empty_text,
    get_acc_help_topic: accessible_get_help_topic,
    get_acc_keyboard_shortcut: empty_text,
    get_acc_focus: focus,
    get_acc_selection: selection,
    get_acc_default_action: default_action,
    acc_select: select,
    acc_location: location,
    acc_navigate: navigate,
    acc_hit_test: hit_test,
    acc_do_default_action: do_default_action,
    put_acc_name: put_text,
    put_acc_value: put_text,
};

fn create_provider(hwnd: HWND, links: Arc<RwLock<Vec<VisibleLink>>>) -> *mut c_void {
    Box::into_raw(Box::new(PreviewAccessible {
        vtable: &PREVIEW_VTABLE,
        references: AtomicU32::new(1),
        hwnd,
        links,
    }))
    .cast()
}

/// Answers `WM_GETOBJECT(OBJID_CLIENT)`. Call it with no view state borrowed: a client may call
/// back into the preview window while `LresultFromObject` runs.
pub fn object_result(hwnd: HWND, links: Arc<RwLock<Vec<VisibleLink>>>, wparam: WPARAM) -> LRESULT {
    let provider = create_provider(hwnd, links);
    let result = unsafe { LresultFromObject(&IID_IACCESSIBLE, wparam, provider) };
    unsafe { release(provider) };
    result
}

unsafe fn item<'a>(this: *mut c_void) -> &'a PreviewAccessible {
    unsafe { &*this.cast::<PreviewAccessible>() }
}

/// `Some(None)` for the document itself, `Some(Some(link))` for a child, `None` for a bad id.
fn target(item: &PreviewAccessible, child: &RawVariant) -> Option<Option<VisibleLink>> {
    match child.child_id()? {
        0 => Some(None),
        id if id > 0 => item
            .links
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .get(id as usize - 1)
            .cloned()
            .map(Some),
        _ => None,
    }
}

/// Asks the preview window's own thread: MSAA clients call in on RPC threads, where `GetFocus`
/// reports that thread's (empty) focus.
fn has_focus(item: &PreviewAccessible) -> bool {
    if item.hwnd.is_null() {
        return false;
    }
    let thread = unsafe { GetWindowThreadProcessId(item.hwnd, std::ptr::null_mut()) };
    let mut info = GUITHREADINFO {
        cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    thread != 0
        && unsafe { GetGUIThreadInfo(thread, &mut info) } != 0
        && info.hwndFocus == item.hwnd
}

unsafe extern "system" fn query_interface(
    this: *mut c_void,
    iid: *const GUID,
    output: *mut *mut c_void,
) -> HRESULT {
    if iid.is_null() || output.is_null() {
        return E_INVALIDARG;
    }
    let requested = unsafe { *iid };
    if guid_eq(&requested, &IID_IUNKNOWN)
        || guid_eq(&requested, &IID_IDISPATCH)
        || guid_eq(&requested, &IID_IACCESSIBLE)
    {
        unsafe {
            *output = this;
            add_ref(this);
        }
        S_OK
    } else {
        unsafe { *output = std::ptr::null_mut() };
        E_NOINTERFACE
    }
}

unsafe extern "system" fn add_ref(this: *mut c_void) -> u32 {
    unsafe { item(this) }
        .references
        .fetch_add(1, Ordering::Relaxed)
        + 1
}

unsafe extern "system" fn release(this: *mut c_void) -> u32 {
    let remaining = unsafe { item(this) }
        .references
        .fetch_sub(1, Ordering::Release)
        - 1;
    if remaining == 0 {
        drop(unsafe { Box::from_raw(this.cast::<PreviewAccessible>()) });
    }
    remaining
}

unsafe extern "system" fn child_count(this: *mut c_void, count: *mut i32) -> HRESULT {
    if count.is_null() {
        return E_INVALIDARG;
    }
    let links = unsafe { item(this) }
        .links
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .len();
    unsafe { *count = i32::try_from(links).unwrap_or(i32::MAX) };
    S_OK
}

unsafe extern "system" fn child(
    this: *mut c_void,
    child: RawVariant,
    output: *mut *mut c_void,
) -> HRESULT {
    if output.is_null() {
        return E_INVALIDARG;
    }
    unsafe { *output = std::ptr::null_mut() };
    match target(unsafe { item(this) }, &child) {
        // Links are simple elements: clients address them through the document by child id.
        Some(Some(_)) => S_FALSE,
        _ => E_INVALIDARG,
    }
}

unsafe extern "system" fn name(this: *mut c_void, child: RawVariant, output: *mut BSTR) -> HRESULT {
    match target(unsafe { item(this) }, &child) {
        Some(None) => unsafe { allocate_bstr("Markdown preview", output) },
        Some(Some(link)) => unsafe { allocate_bstr(&link.text, output) },
        None => E_INVALIDARG,
    }
}

unsafe extern "system" fn value(
    this: *mut c_void,
    child: RawVariant,
    output: *mut BSTR,
) -> HRESULT {
    match target(unsafe { item(this) }, &child) {
        Some(None) => unsafe { allocate_bstr("", output) },
        Some(Some(link)) => unsafe { allocate_bstr(&link.dest, output) },
        None => E_INVALIDARG,
    }
}

unsafe extern "system" fn empty_text(
    this: *mut c_void,
    child: RawVariant,
    output: *mut BSTR,
) -> HRESULT {
    match target(unsafe { item(this) }, &child) {
        Some(_) => unsafe { allocate_bstr("", output) },
        None => E_INVALIDARG,
    }
}

unsafe extern "system" fn role(
    this: *mut c_void,
    child: RawVariant,
    output: *mut RawVariant,
) -> HRESULT {
    if output.is_null() {
        return E_INVALIDARG;
    }
    let role = match target(unsafe { item(this) }, &child) {
        Some(None) => ROLE_SYSTEM_DOCUMENT,
        Some(Some(_)) => ROLE_SYSTEM_LINK,
        None => return E_INVALIDARG,
    };
    unsafe { *output = RawVariant::integer(role as i32) };
    S_OK
}

unsafe extern "system" fn state(
    this: *mut c_void,
    child: RawVariant,
    output: *mut RawVariant,
) -> HRESULT {
    if output.is_null() {
        return E_INVALIDARG;
    }
    let item = unsafe { item(this) };
    let state = match target(item, &child) {
        Some(None) => {
            let focused = if has_focus(item) {
                STATE_SYSTEM_FOCUSED
            } else {
                0
            };
            STATE_SYSTEM_READONLY | STATE_SYSTEM_FOCUSABLE | focused
        }
        Some(Some(_)) => STATE_SYSTEM_LINKED | STATE_SYSTEM_FOCUSABLE,
        None => return E_INVALIDARG,
    };
    unsafe { *output = RawVariant::integer(state as i32) };
    S_OK
}

unsafe extern "system" fn focus(this: *mut c_void, output: *mut RawVariant) -> HRESULT {
    if output.is_null() {
        return E_INVALIDARG;
    }
    if has_focus(unsafe { item(this) }) {
        unsafe { *output = RawVariant::integer(0) };
        S_OK
    } else {
        unsafe { *output = RawVariant::empty() };
        S_FALSE
    }
}

unsafe extern "system" fn selection(_this: *mut c_void, output: *mut RawVariant) -> HRESULT {
    if output.is_null() {
        return E_INVALIDARG;
    }
    unsafe { *output = RawVariant::empty() };
    S_FALSE
}

unsafe extern "system" fn default_action(
    this: *mut c_void,
    child: RawVariant,
    output: *mut BSTR,
) -> HRESULT {
    match target(unsafe { item(this) }, &child) {
        Some(Some(_)) => unsafe { allocate_bstr("Jump", output) },
        Some(None) => unsafe { allocate_bstr("", output) },
        None => E_INVALIDARG,
    }
}

unsafe extern "system" fn select(_this: *mut c_void, _flags: i32, _child: RawVariant) -> HRESULT {
    E_NOTIMPL
}

unsafe extern "system" fn location(
    this: *mut c_void,
    left: *mut i32,
    top: *mut i32,
    width: *mut i32,
    height: *mut i32,
    child: RawVariant,
) -> HRESULT {
    if left.is_null() || top.is_null() || width.is_null() || height.is_null() {
        return E_INVALIDARG;
    }
    let item = unsafe { item(this) };
    let rect = match target(item, &child) {
        Some(None) => {
            let mut window = EMPTY_RECT;
            if item.hwnd.is_null() || unsafe { GetWindowRect(item.hwnd, &mut window) } == 0 {
                return S_FALSE;
            }
            window
        }
        Some(Some(link)) => {
            let mut origin = POINT { x: 0, y: 0 };
            if item.hwnd.is_null() || unsafe { ClientToScreen(item.hwnd, &mut origin) } == 0 {
                return S_FALSE;
            }
            RECT {
                left: link.rect.left + origin.x,
                top: link.rect.top + origin.y,
                right: link.rect.right + origin.x,
                bottom: link.rect.bottom + origin.y,
            }
        }
        None => return E_INVALIDARG,
    };
    unsafe {
        *left = rect.left;
        *top = rect.top;
        *width = rect.right - rect.left;
        *height = rect.bottom - rect.top;
    }
    S_OK
}

unsafe extern "system" fn navigate(
    _this: *mut c_void,
    _direction: i32,
    _start: RawVariant,
    output: *mut RawVariant,
) -> HRESULT {
    if !output.is_null() {
        unsafe { *output = RawVariant::empty() };
    }
    E_NOTIMPL
}

unsafe extern "system" fn hit_test(
    this: *mut c_void,
    x: i32,
    y: i32,
    output: *mut RawVariant,
) -> HRESULT {
    if output.is_null() {
        return E_INVALIDARG;
    }
    let item = unsafe { item(this) };
    let mut point = POINT { x, y };
    let mut client = EMPTY_RECT;
    let inside = |rect: &RECT, point: &POINT| {
        point.x >= rect.left && point.x < rect.right && point.y >= rect.top && point.y < rect.bottom
    };
    if item.hwnd.is_null()
        || unsafe { ScreenToClient(item.hwnd, &mut point) } == 0
        || unsafe { GetClientRect(item.hwnd, &mut client) } == 0
        || !inside(&client, &point)
    {
        unsafe { *output = RawVariant::empty() };
        return S_FALSE;
    }
    let id = item
        .links
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .iter()
        .position(|link| inside(&link.rect, &point))
        .map_or(0, |index| index as i32 + 1);
    unsafe { *output = RawVariant::integer(id) };
    S_OK
}

unsafe extern "system" fn do_default_action(this: *mut c_void, child: RawVariant) -> HRESULT {
    let item = unsafe { item(this) };
    match target(item, &child) {
        Some(Some(link)) => {
            // Post the destination itself: the window's snapshot may change before the message is
            // handled, and an index would then name another link.
            let payload = Box::into_raw(Box::new(link.dest));
            if unsafe { PostMessageW(item.hwnd, WM_FASTPAD_PREVIEW_ACTIVATE, 0, payload as isize) }
                == 0
            {
                drop(unsafe { Box::from_raw(payload) });
                return E_FAIL;
            }
            S_OK
        }
        _ => E_INVALIDARG,
    }
}

unsafe extern "system" fn put_text(
    _this: *mut c_void,
    _child: RawVariant,
    _value: BSTR,
) -> HRESULT {
    E_NOTIMPL
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Foundation::{SysFreeString, SysStringLen};

    fn links() -> Arc<RwLock<Vec<VisibleLink>>> {
        let rect = RECT {
            left: 10,
            top: 10,
            right: 60,
            bottom: 30,
        };
        Arc::new(RwLock::new(vec![
            VisibleLink {
                text: "site".into(),
                dest: "https://x.dev".into(),
                rect,
                disclosure: None,
                focused: false,
            },
            VisibleLink {
                text: "notes".into(),
                dest: "notes.md".into(),
                rect: RECT {
                    top: 40,
                    bottom: 60,
                    ..rect
                },
                disclosure: None,
                focused: false,
            },
        ]))
    }

    fn read_bstr(value: BSTR) -> String {
        let text = unsafe { std::slice::from_raw_parts(value, SysStringLen(value) as usize) };
        let result = String::from_utf16_lossy(text);
        unsafe { SysFreeString(value) };
        result
    }

    #[test]
    fn default_action_posts_the_link_destination() {
        use crate::preview::render::TestWindow;
        use windows_sys::Win32::UI::WindowsAndMessaging::{MSG, PM_REMOVE, PeekMessageW};
        let window = TestWindow::new(100, 100);
        let provider = create_provider(window.0, links());
        let mut msg = MSG::default();
        unsafe {
            assert_eq!(
                (PREVIEW_VTABLE.acc_do_default_action)(provider, RawVariant::integer(2)),
                S_OK
            );
            assert_ne!(
                PeekMessageW(
                    &mut msg,
                    window.0,
                    WM_FASTPAD_PREVIEW_ACTIVATE,
                    WM_FASTPAD_PREVIEW_ACTIVATE,
                    PM_REMOVE,
                ),
                0
            );
            assert_eq!(*Box::from_raw(msg.lParam as *mut String), "notes.md");
            assert_eq!(
                (PREVIEW_VTABLE.acc_do_default_action)(provider, RawVariant::integer(3)),
                E_INVALIDARG
            );
            (PREVIEW_VTABLE.release)(provider);
        }
    }

    #[test]
    fn focus_is_reported_to_clients_on_other_threads() {
        // Break caught: `GetFocus` is per thread, so a screen reader calling in on an RPC thread
        // always saw the focused preview as unfocused.
        use crate::preview::render::TestWindow;
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus};
        use windows_sys::Win32::UI::WindowsAndMessaging::{SW_SHOW, ShowWindow};
        let window = TestWindow::new(100, 100);
        unsafe {
            ShowWindow(window.0, SW_SHOW);
            SetFocus(window.0);
        }
        let focused_here = unsafe { GetFocus() } == window.0;
        let hwnd = window.0 as isize;
        let seen_elsewhere = std::thread::spawn(move || {
            let provider = create_provider(hwnd as HWND, Arc::default());
            let focused = has_focus(unsafe { item(provider) });
            unsafe { release(provider) };
            focused
        })
        .join()
        .unwrap();
        assert!(focused_here, "the test window must take the focus");
        assert!(seen_elsewhere);
    }

    #[test]
    fn the_preview_is_a_document_whose_children_are_its_links() {
        let provider = create_provider(std::ptr::null_mut(), links());
        let table = &PREVIEW_VTABLE;
        unsafe {
            let mut count = 0;
            assert_eq!((table.get_acc_child_count)(provider, &mut count), S_OK);
            assert_eq!(count, 2);

            let mut name: BSTR = std::ptr::null();
            assert_eq!(
                (table.get_acc_name)(provider, RawVariant::integer(0), &mut name),
                S_OK
            );
            assert_eq!(read_bstr(name), "Markdown preview");
            assert_eq!(
                (table.get_acc_name)(provider, RawVariant::integer(2), &mut name),
                S_OK
            );
            assert_eq!(read_bstr(name), "notes");

            let mut value: BSTR = std::ptr::null();
            assert_eq!(
                (table.get_acc_value)(provider, RawVariant::integer(1), &mut value),
                S_OK
            );
            assert_eq!(read_bstr(value), "https://x.dev");

            let mut role = RawVariant::empty();
            assert_eq!(
                (table.get_acc_role)(provider, RawVariant::integer(0), &mut role),
                S_OK
            );
            assert_eq!(role.child_id(), Some(ROLE_SYSTEM_DOCUMENT as i32));
            assert_eq!(
                (table.get_acc_role)(provider, RawVariant::integer(1), &mut role),
                S_OK
            );
            assert_eq!(role.child_id(), Some(ROLE_SYSTEM_LINK as i32));

            let mut action: BSTR = std::ptr::null();
            assert_eq!(
                (table.get_acc_default_action)(provider, RawVariant::integer(1), &mut action),
                S_OK
            );
            assert_eq!(read_bstr(action), "Jump");

            assert_eq!(
                (table.get_acc_name)(provider, RawVariant::integer(3), &mut name),
                E_INVALIDARG
            );
            (table.release)(provider);
        }
    }
}
