use crate::window::commands::CommandId;
use crate::window::titlebar::{Point, Size, TitleBarLayout};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, Ordering};
use windows_sys::Win32::Foundation::{
    DISP_E_MEMBERNOTFOUND, E_INVALIDARG, E_NOINTERFACE, E_NOTIMPL, HWND, LRESULT, S_FALSE, S_OK,
    SysAllocStringLen, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
use windows_sys::Win32::UI::Accessibility::{
    LresultFromObject, NAVDIR_FIRSTCHILD, NAVDIR_LASTCHILD, NAVDIR_NEXT, NAVDIR_PREVIOUS,
    ROLE_SYSTEM_PAGETAB, ROLE_SYSTEM_PAGETABLIST, ROLE_SYSTEM_PUSHBUTTON,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetWindowRect, PostMessageW, SC_CLOSE, SC_MAXIMIZE, SC_MINIMIZE,
    STATE_SYSTEM_SELECTABLE, STATE_SYSTEM_SELECTED, WM_COMMAND, WM_SYSCOMMAND,
};
use windows_sys::core::{BSTR, GUID, HRESULT};

const IID_IUNKNOWN: GUID = GUID::from_u128(0x00000000_0000_0000_c000_000000000046);
const IID_IDISPATCH: GUID = GUID::from_u128(0x00020400_0000_0000_c000_000000000046);
const IID_IACCESSIBLE: GUID = GUID::from_u128(0x618736e0_3c3d_11cf_810c_00aa00389b71);
const VT_EMPTY: u16 = 0;
const VT_I4: u16 = 3;
const STATE_SYSTEM_FOCUSABLE: u32 = 0x0010_0000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AccessibleChild {
    Tab(String),
    Button(&'static str),
}

impl AccessibleChild {
    #[cfg(test)]
    pub fn button_name(&self) -> Option<&str> {
        match self {
            Self::Tab(_) => None,
            Self::Button(name) => Some(name),
        }
    }
}

pub fn accessible_children(tab_titles: &[&str]) -> Vec<AccessibleChild> {
    let mut children = tab_titles
        .iter()
        .map(|title| AccessibleChild::Tab((*title).to_owned()))
        .collect::<Vec<_>>();
    children.extend([
        AccessibleChild::Button("New tab"),
        AccessibleChild::Button("Overflow"),
        AccessibleChild::Button("Minimize"),
        AccessibleChild::Button("Maximize"),
        AccessibleChild::Button("Close"),
    ]);
    children
}

#[derive(Debug, Default)]
pub(crate) struct AccessibilityState {
    provider: Option<NonNull<AccessibleProvider>>,
}

impl AccessibilityState {
    pub(crate) fn ensure(&mut self, hwnd: HWND, tab_titles: &[&str]) -> *mut c_void {
        let provider = *self.provider.get_or_insert_with(|| {
            let provider = Box::new(AccessibleProvider {
                vtable: &ACCESSIBLE_VTABLE,
                references: AtomicU32::new(1),
                hwnd,
                children: accessible_children(tab_titles),
            });
            NonNull::new(Box::into_raw(provider)).expect("Box never creates a null pointer")
        });
        provider.as_ptr().cast()
    }

    #[cfg(test)]
    fn ensure_for_test(&mut self) {
        let _ = self.ensure(std::ptr::null_mut(), &["Untitled"]);
    }

    #[cfg(test)]
    fn is_created(&self) -> bool {
        self.provider.is_some()
    }
}

impl Drop for AccessibilityState {
    fn drop(&mut self) {
        if let Some(provider) = self.provider.take() {
            unsafe {
                accessible_release(provider.as_ptr().cast());
            }
        }
    }
}

pub(crate) unsafe fn object_result(provider: *mut c_void, wparam: WPARAM) -> LRESULT {
    unsafe { LresultFromObject(&IID_IACCESSIBLE, wparam, provider) }
}

#[repr(C)]
struct AccessibleProvider {
    vtable: &'static AccessibleVtable,
    references: AtomicU32,
    hwnd: HWND,
    children: Vec<AccessibleChild>,
}

#[repr(C)]
struct AccessibleVtable {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    get_type_info_count: unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT,
    get_type_info: unsafe extern "system" fn(*mut c_void, u32, u32, *mut *mut c_void) -> HRESULT,
    get_ids_of_names: unsafe extern "system" fn(
        *mut c_void,
        *const GUID,
        *mut *mut u16,
        u32,
        u32,
        *mut i32,
    ) -> HRESULT,
    invoke: unsafe extern "system" fn(
        *mut c_void,
        i32,
        *const GUID,
        u32,
        u16,
        *mut c_void,
        *mut RawVariant,
        *mut c_void,
        *mut u32,
    ) -> HRESULT,
    get_acc_parent: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT,
    get_acc_child_count: unsafe extern "system" fn(*mut c_void, *mut i32) -> HRESULT,
    get_acc_child: unsafe extern "system" fn(*mut c_void, RawVariant, *mut *mut c_void) -> HRESULT,
    get_acc_name: unsafe extern "system" fn(*mut c_void, RawVariant, *mut BSTR) -> HRESULT,
    get_acc_value: unsafe extern "system" fn(*mut c_void, RawVariant, *mut BSTR) -> HRESULT,
    get_acc_description: unsafe extern "system" fn(*mut c_void, RawVariant, *mut BSTR) -> HRESULT,
    get_acc_role: unsafe extern "system" fn(*mut c_void, RawVariant, *mut RawVariant) -> HRESULT,
    get_acc_state: unsafe extern "system" fn(*mut c_void, RawVariant, *mut RawVariant) -> HRESULT,
    get_acc_help: unsafe extern "system" fn(*mut c_void, RawVariant, *mut BSTR) -> HRESULT,
    get_acc_help_topic:
        unsafe extern "system" fn(*mut c_void, *mut BSTR, RawVariant, *mut i32) -> HRESULT,
    get_acc_keyboard_shortcut:
        unsafe extern "system" fn(*mut c_void, RawVariant, *mut BSTR) -> HRESULT,
    get_acc_focus: unsafe extern "system" fn(*mut c_void, *mut RawVariant) -> HRESULT,
    get_acc_selection: unsafe extern "system" fn(*mut c_void, *mut RawVariant) -> HRESULT,
    get_acc_default_action:
        unsafe extern "system" fn(*mut c_void, RawVariant, *mut BSTR) -> HRESULT,
    acc_select: unsafe extern "system" fn(*mut c_void, i32, RawVariant) -> HRESULT,
    acc_location: unsafe extern "system" fn(
        *mut c_void,
        *mut i32,
        *mut i32,
        *mut i32,
        *mut i32,
        RawVariant,
    ) -> HRESULT,
    acc_navigate:
        unsafe extern "system" fn(*mut c_void, i32, RawVariant, *mut RawVariant) -> HRESULT,
    acc_hit_test: unsafe extern "system" fn(*mut c_void, i32, i32, *mut RawVariant) -> HRESULT,
    acc_do_default_action: unsafe extern "system" fn(*mut c_void, RawVariant) -> HRESULT,
    put_acc_name: unsafe extern "system" fn(*mut c_void, RawVariant, BSTR) -> HRESULT,
    put_acc_value: unsafe extern "system" fn(*mut c_void, RawVariant, BSTR) -> HRESULT,
}

#[repr(C)]
union VariantData {
    l_val: i32,
    pointer: *mut c_void,
    alignment: [u64; 2],
}

#[repr(C)]
struct RawVariant {
    vt: u16,
    reserved: [u16; 3],
    data: VariantData,
}

impl RawVariant {
    fn empty() -> Self {
        Self {
            vt: VT_EMPTY,
            reserved: [0; 3],
            data: VariantData { alignment: [0; 2] },
        }
    }

    fn integer(value: i32) -> Self {
        Self {
            vt: VT_I4,
            reserved: [0; 3],
            data: VariantData { l_val: value },
        }
    }

    fn child_id(&self) -> Option<i32> {
        (self.vt == VT_I4).then_some(unsafe { self.data.l_val })
    }
}

static ACCESSIBLE_VTABLE: AccessibleVtable = AccessibleVtable {
    query_interface: accessible_query_interface,
    add_ref: accessible_add_ref,
    release: accessible_release,
    get_type_info_count: accessible_get_type_info_count,
    get_type_info: accessible_get_type_info,
    get_ids_of_names: accessible_get_ids_of_names,
    invoke: accessible_invoke,
    get_acc_parent: accessible_get_parent,
    get_acc_child_count: accessible_get_child_count,
    get_acc_child: accessible_get_child,
    get_acc_name: accessible_get_name,
    get_acc_value: accessible_get_value,
    get_acc_description: accessible_get_description,
    get_acc_role: accessible_get_role,
    get_acc_state: accessible_get_state,
    get_acc_help: accessible_get_help,
    get_acc_help_topic: accessible_get_help_topic,
    get_acc_keyboard_shortcut: accessible_get_keyboard_shortcut,
    get_acc_focus: accessible_get_focus,
    get_acc_selection: accessible_get_selection,
    get_acc_default_action: accessible_get_default_action,
    acc_select: accessible_select,
    acc_location: accessible_location,
    acc_navigate: accessible_navigate,
    acc_hit_test: accessible_hit_test,
    acc_do_default_action: accessible_do_default_action,
    put_acc_name: accessible_put_name,
    put_acc_value: accessible_put_value,
};

unsafe fn provider<'a>(this: *mut c_void) -> &'a AccessibleProvider {
    unsafe { &*(this.cast::<AccessibleProvider>()) }
}

unsafe extern "system" fn accessible_query_interface(
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
            accessible_add_ref(this);
        }
        S_OK
    } else {
        unsafe {
            *output = std::ptr::null_mut();
        }
        E_NOINTERFACE
    }
}

fn guid_eq(left: &GUID, right: &GUID) -> bool {
    left.data1 == right.data1
        && left.data2 == right.data2
        && left.data3 == right.data3
        && left.data4 == right.data4
}

unsafe extern "system" fn accessible_add_ref(this: *mut c_void) -> u32 {
    unsafe { provider(this) }
        .references
        .fetch_add(1, Ordering::Relaxed)
        + 1
}

unsafe extern "system" fn accessible_release(this: *mut c_void) -> u32 {
    let remaining = unsafe { provider(this) }
        .references
        .fetch_sub(1, Ordering::Release)
        - 1;
    if remaining == 0 {
        std::sync::atomic::fence(Ordering::Acquire);
        unsafe {
            drop(Box::from_raw(this.cast::<AccessibleProvider>()));
        }
    }
    remaining
}

unsafe extern "system" fn accessible_get_type_info_count(
    _this: *mut c_void,
    count: *mut u32,
) -> HRESULT {
    if count.is_null() {
        return E_INVALIDARG;
    }
    unsafe { *count = 0 };
    S_OK
}

unsafe extern "system" fn accessible_get_type_info(
    _this: *mut c_void,
    _index: u32,
    _locale: u32,
    output: *mut *mut c_void,
) -> HRESULT {
    if !output.is_null() {
        unsafe { *output = std::ptr::null_mut() };
    }
    E_NOTIMPL
}

unsafe extern "system" fn accessible_get_ids_of_names(
    _this: *mut c_void,
    _iid: *const GUID,
    _names: *mut *mut u16,
    _count: u32,
    _locale: u32,
    _ids: *mut i32,
) -> HRESULT {
    DISP_E_MEMBERNOTFOUND
}

unsafe extern "system" fn accessible_invoke(
    _this: *mut c_void,
    _id: i32,
    _iid: *const GUID,
    _locale: u32,
    _flags: u16,
    _params: *mut c_void,
    _result: *mut RawVariant,
    _exception: *mut c_void,
    _argument_error: *mut u32,
) -> HRESULT {
    DISP_E_MEMBERNOTFOUND
}

unsafe extern "system" fn accessible_get_parent(
    _this: *mut c_void,
    output: *mut *mut c_void,
) -> HRESULT {
    if output.is_null() {
        return E_INVALIDARG;
    }
    unsafe { *output = std::ptr::null_mut() };
    S_FALSE
}

unsafe extern "system" fn accessible_get_child_count(
    this: *mut c_void,
    count: *mut i32,
) -> HRESULT {
    if count.is_null() {
        return E_INVALIDARG;
    }
    unsafe { *count = provider(this).children.len() as i32 };
    S_OK
}

unsafe extern "system" fn accessible_get_child(
    _this: *mut c_void,
    _child: RawVariant,
    output: *mut *mut c_void,
) -> HRESULT {
    if output.is_null() {
        return E_INVALIDARG;
    }
    unsafe { *output = std::ptr::null_mut() };
    S_FALSE
}

unsafe extern "system" fn accessible_get_name(
    this: *mut c_void,
    child: RawVariant,
    output: *mut BSTR,
) -> HRESULT {
    let Some(name) = (unsafe { child_name(provider(this), &child) }) else {
        return E_INVALIDARG;
    };
    unsafe { allocate_bstr(name, output) }
}

unsafe extern "system" fn accessible_get_value(
    _this: *mut c_void,
    _child: RawVariant,
    output: *mut BSTR,
) -> HRESULT {
    unsafe { allocate_bstr("", output) }
}

unsafe extern "system" fn accessible_get_description(
    this: *mut c_void,
    child: RawVariant,
    output: *mut BSTR,
) -> HRESULT {
    let Some(id) = child.child_id() else {
        return E_INVALIDARG;
    };
    let description = if id > 0
        && matches!(
            unsafe { provider(this) }.children.get((id - 1) as usize),
            Some(AccessibleChild::Tab(_))
        ) {
        "Selectable and closable tab"
    } else {
        "Title bar button"
    };
    unsafe { allocate_bstr(description, output) }
}

unsafe extern "system" fn accessible_get_role(
    this: *mut c_void,
    child: RawVariant,
    output: *mut RawVariant,
) -> HRESULT {
    if output.is_null() {
        return E_INVALIDARG;
    }
    let Some(id) = child.child_id() else {
        return E_INVALIDARG;
    };
    let role = if id == 0 {
        ROLE_SYSTEM_PAGETABLIST
    } else {
        match unsafe { provider(this) }.children.get((id - 1) as usize) {
            Some(AccessibleChild::Tab(_)) => ROLE_SYSTEM_PAGETAB,
            Some(AccessibleChild::Button(_)) => ROLE_SYSTEM_PUSHBUTTON,
            None => return E_INVALIDARG,
        }
    };
    unsafe { *output = RawVariant::integer(role as i32) };
    S_OK
}

unsafe extern "system" fn accessible_get_state(
    this: *mut c_void,
    child: RawVariant,
    output: *mut RawVariant,
) -> HRESULT {
    if output.is_null() {
        return E_INVALIDARG;
    }
    let Some(id) = child.child_id() else {
        return E_INVALIDARG;
    };
    let state = if id == 0 {
        0
    } else {
        match unsafe { provider(this) }.children.get((id - 1) as usize) {
            Some(AccessibleChild::Tab(_)) => {
                STATE_SYSTEM_SELECTABLE | STATE_SYSTEM_FOCUSABLE | STATE_SYSTEM_SELECTED
            }
            Some(AccessibleChild::Button(_)) => STATE_SYSTEM_FOCUSABLE,
            None => return E_INVALIDARG,
        }
    };
    unsafe { *output = RawVariant::integer(state as i32) };
    S_OK
}

unsafe extern "system" fn accessible_get_help(
    _this: *mut c_void,
    _child: RawVariant,
    output: *mut BSTR,
) -> HRESULT {
    unsafe { allocate_bstr("", output) }
}

unsafe extern "system" fn accessible_get_help_topic(
    _this: *mut c_void,
    output: *mut BSTR,
    _child: RawVariant,
    topic: *mut i32,
) -> HRESULT {
    if !output.is_null() {
        unsafe { *output = std::ptr::null_mut() };
    }
    if !topic.is_null() {
        unsafe { *topic = -1 };
    }
    S_FALSE
}

unsafe extern "system" fn accessible_get_keyboard_shortcut(
    _this: *mut c_void,
    _child: RawVariant,
    output: *mut BSTR,
) -> HRESULT {
    unsafe { allocate_bstr("", output) }
}

unsafe extern "system" fn accessible_get_focus(
    _this: *mut c_void,
    output: *mut RawVariant,
) -> HRESULT {
    if output.is_null() {
        return E_INVALIDARG;
    }
    unsafe { *output = RawVariant::integer(1) };
    S_OK
}

unsafe extern "system" fn accessible_get_selection(
    _this: *mut c_void,
    output: *mut RawVariant,
) -> HRESULT {
    if output.is_null() {
        return E_INVALIDARG;
    }
    unsafe { *output = RawVariant::integer(1) };
    S_OK
}

unsafe extern "system" fn accessible_get_default_action(
    this: *mut c_void,
    child: RawVariant,
    output: *mut BSTR,
) -> HRESULT {
    let Some(id) = child.child_id() else {
        return E_INVALIDARG;
    };
    let action = match unsafe { provider(this) }.children.get((id - 1) as usize) {
        Some(AccessibleChild::Tab(_)) => "Select",
        Some(AccessibleChild::Button(_)) => "Press",
        None => return E_INVALIDARG,
    };
    unsafe { allocate_bstr(action, output) }
}

unsafe extern "system" fn accessible_select(
    _this: *mut c_void,
    _flags: i32,
    child: RawVariant,
) -> HRESULT {
    if child.child_id().is_some_and(|id| id > 0) {
        S_OK
    } else {
        E_INVALIDARG
    }
}

unsafe extern "system" fn accessible_location(
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
    let Some(rect) = (unsafe { child_screen_rect(provider(this), &child) }) else {
        return E_INVALIDARG;
    };
    unsafe {
        *left = rect.left;
        *top = rect.top;
        *width = rect.right - rect.left;
        *height = rect.bottom - rect.top;
    }
    S_OK
}

unsafe extern "system" fn accessible_navigate(
    this: *mut c_void,
    direction: i32,
    start: RawVariant,
    output: *mut RawVariant,
) -> HRESULT {
    if output.is_null() {
        return E_INVALIDARG;
    }
    let Some(id) = start.child_id() else {
        return E_INVALIDARG;
    };
    let count = unsafe { provider(this) }.children.len() as i32;
    let target = match (direction as u32, id) {
        (NAVDIR_FIRSTCHILD, 0) => Some(1),
        (NAVDIR_LASTCHILD, 0) => Some(count),
        (NAVDIR_NEXT, value) if value > 0 && value < count => Some(value + 1),
        (NAVDIR_PREVIOUS, value) if value > 1 => Some(value - 1),
        _ => None,
    };
    unsafe { *output = target.map_or_else(RawVariant::empty, RawVariant::integer) };
    if target.is_some() { S_OK } else { S_FALSE }
}

unsafe extern "system" fn accessible_hit_test(
    this: *mut c_void,
    x: i32,
    y: i32,
    output: *mut RawVariant,
) -> HRESULT {
    if output.is_null() {
        return E_INVALIDARG;
    }
    let item = unsafe { provider(this) };
    let mut point = windows_sys::Win32::Foundation::POINT { x, y };
    if item.hwnd.is_null() || unsafe { ScreenToClient(item.hwnd, &mut point) } == 0 {
        unsafe { *output = RawVariant::empty() };
        return S_FALSE;
    }
    let target = native_layout(item).hit_test(Point::new(point.x, point.y));
    let tab_count = item
        .children
        .iter()
        .take_while(|child| matches!(child, AccessibleChild::Tab(_)))
        .count();
    let id = match target {
        crate::window::titlebar::HitTarget::Tab(index)
        | crate::window::titlebar::HitTarget::CloseTab(index) => Some(index as i32 + 1),
        crate::window::titlebar::HitTarget::NewTab => Some(tab_count as i32 + 1),
        crate::window::titlebar::HitTarget::Overflow => Some(tab_count as i32 + 2),
        crate::window::titlebar::HitTarget::Minimize => Some(tab_count as i32 + 3),
        crate::window::titlebar::HitTarget::Maximize => Some(tab_count as i32 + 4),
        crate::window::titlebar::HitTarget::Close => Some(tab_count as i32 + 5),
        _ => None,
    };
    unsafe { *output = id.map_or_else(RawVariant::empty, RawVariant::integer) };
    if id.is_some() { S_OK } else { S_FALSE }
}

unsafe extern "system" fn accessible_do_default_action(
    this: *mut c_void,
    child: RawVariant,
) -> HRESULT {
    let item = unsafe { provider(this) };
    let Some(id) = child.child_id() else {
        return E_INVALIDARG;
    };
    let tab_count = item
        .children
        .iter()
        .take_while(|entry| matches!(entry, AccessibleChild::Tab(_)))
        .count() as i32;
    match id - tab_count {
        1 => unsafe { PostMessageW(item.hwnd, WM_COMMAND, CommandId::New as usize, 0) },
        2 => unsafe { PostMessageW(item.hwnd, WM_COMMAND, 0, 0) },
        3 => unsafe { PostMessageW(item.hwnd, WM_SYSCOMMAND, SC_MINIMIZE as usize, 0) },
        4 => unsafe { PostMessageW(item.hwnd, WM_SYSCOMMAND, SC_MAXIMIZE as usize, 0) },
        5 => unsafe { PostMessageW(item.hwnd, WM_SYSCOMMAND, SC_CLOSE as usize, 0) },
        value if value <= 0 && id > 0 => return S_OK,
        _ => return E_INVALIDARG,
    };
    S_OK
}

unsafe extern "system" fn accessible_put_name(
    _this: *mut c_void,
    _child: RawVariant,
    _value: BSTR,
) -> HRESULT {
    E_NOTIMPL
}

unsafe extern "system" fn accessible_put_value(
    _this: *mut c_void,
    _child: RawVariant,
    _value: BSTR,
) -> HRESULT {
    E_NOTIMPL
}

unsafe fn child_name<'a>(item: &'a AccessibleProvider, child: &RawVariant) -> Option<&'a str> {
    match child.child_id()? {
        0 => Some("FastPad title tabs"),
        id if id > 0 => match item.children.get((id - 1) as usize)? {
            AccessibleChild::Tab(name) => Some(name),
            AccessibleChild::Button(name) => Some(name),
        },
        _ => None,
    }
}

unsafe fn allocate_bstr(value: &str, output: *mut BSTR) -> HRESULT {
    if output.is_null() {
        return E_INVALIDARG;
    }
    let wide = value.encode_utf16().collect::<Vec<_>>();
    let bstr = unsafe { SysAllocStringLen(wide.as_ptr(), wide.len() as u32) };
    unsafe { *output = bstr };
    if bstr.is_null() && !wide.is_empty() {
        windows_sys::Win32::Foundation::E_OUTOFMEMORY
    } else {
        S_OK
    }
}

unsafe fn child_screen_rect(
    item: &AccessibleProvider,
    child: &RawVariant,
) -> Option<windows_sys::Win32::Foundation::RECT> {
    let id = child.child_id()?;
    let mut window = windows_sys::Win32::Foundation::RECT::default();
    if item.hwnd.is_null() || unsafe { GetWindowRect(item.hwnd, &mut window) } == 0 {
        return None;
    }
    if id == 0 {
        return Some(window);
    }
    let layout = native_layout(item);
    let tab_count = item
        .children
        .iter()
        .take_while(|entry| matches!(entry, AccessibleChild::Tab(_)))
        .count();
    let rect = if id as usize <= tab_count {
        layout.tab(id as usize - 1)
    } else {
        match id as usize - tab_count {
            1 => layout.new_tab,
            2 => layout.overflow,
            3 => layout.minimize,
            4 => layout.maximize,
            5 => layout.close,
            _ => return None,
        }
    };
    let mut origin = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
    unsafe {
        windows_sys::Win32::Graphics::Gdi::ClientToScreen(item.hwnd, &mut origin);
    }
    Some(windows_sys::Win32::Foundation::RECT {
        left: origin.x + rect.left,
        top: origin.y + rect.top,
        right: origin.x + rect.right,
        bottom: origin.y + rect.bottom,
    })
}

fn native_layout(item: &AccessibleProvider) -> TitleBarLayout {
    let mut client = windows_sys::Win32::Foundation::RECT::default();
    unsafe {
        GetClientRect(item.hwnd, &mut client);
    }
    let tabs = item
        .children
        .iter()
        .take_while(|entry| matches!(entry, AccessibleChild::Tab(_)))
        .count();
    TitleBarLayout::calculate(
        Size::new(client.right - client.left, client.bottom - client.top),
        unsafe { GetDpiForWindow(item.hwnd) }.max(96),
        tabs,
    )
}

#[cfg(test)]
mod tests {
    use super::{AccessibilityState, AccessibleChild, accessible_children};

    #[test]
    fn title_strip_accessibility_contains_tab_and_five_named_buttons() {
        let children = accessible_children(&["Untitled"]);
        assert_eq!(children[0], AccessibleChild::Tab("Untitled".into()));
        let names = children
            .iter()
            .filter_map(AccessibleChild::button_name)
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec!["New tab", "Overflow", "Minimize", "Maximize", "Close"]
        );
    }

    #[test]
    fn provider_is_not_created_until_requested() {
        let mut state = AccessibilityState::default();
        assert!(!state.is_created());
        state.ensure_for_test();
        assert!(state.is_created());
    }
}
