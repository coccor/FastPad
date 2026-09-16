use crate::Result;
use crate::platform::{last_error, wide_null};
use crate::window::commands::CommandId;
use crate::window::menu_band::{self, MENU_TITLES};
use crate::window::modal::ModalScope;
use std::cell::RefCell;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    VIRTUAL_KEY, VK_ADD, VK_ESCAPE, VK_LEFT, VK_NUMPAD0, VK_NUMPAD1, VK_NUMPAD2, VK_NUMPAD3,
    VK_NUMPAD4, VK_NUMPAD5, VK_NUMPAD6, VK_NUMPAD7, VK_NUMPAD8, VK_NUMPAD9, VK_OEM_MINUS,
    VK_OEM_PLUS, VK_RIGHT, VK_SUBTRACT, VK_TAB,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    ACCEL, AppendMenuW, CallNextHookEx, CreateAcceleratorTableW, CreateMenu, CreatePopupMenu,
    DestroyAcceleratorTable, DestroyMenu, EnableMenuItem, EndMenu, FALT, FCONTROL, FSHIFT,
    FVIRTKEY, GetSubMenu, HACCEL, HMENU, MF_BYCOMMAND, MF_ENABLED, MF_GRAYED, MF_POPUP,
    MF_SEPARATOR, MF_STRING, MSG, MSGF_MENU, SetWindowsHookExW, TPM_LEFTALIGN, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, TPM_TOPALIGN, TPM_VERTICAL, TPMPARAMS, TrackPopupMenuEx,
    TranslateAcceleratorW, UnhookWindowsHookEx, WH_MSGFILTER, WM_KEYDOWN, WM_LBUTTONDOWN,
    WM_MOUSEMOVE,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AcceleratorSpec {
    pub modifiers: u8,
    pub key: u16,
    pub command: CommandId,
}

pub const fn accelerator_specs() -> [AcceleratorSpec; 42] {
    [
        accelerator(FCONTROL, b'N', CommandId::New),
        accelerator(FCONTROL, b'T', CommandId::New),
        accelerator(FCONTROL, b'O', CommandId::Open),
        accelerator(FCONTROL, b'S', CommandId::Save),
        accelerator(FCONTROL | FSHIFT, b'S', CommandId::SaveAs),
        accelerator(FCONTROL, b'F', CommandId::Find),
        accelerator(FCONTROL, b'H', CommandId::Replace),
        accelerator(FCONTROL, b'Z', CommandId::Undo),
        accelerator(FCONTROL, b'Y', CommandId::Redo),
        accelerator(FCONTROL | FSHIFT, b'F', CommandId::FormatJson),
        virtual_key(FCONTROL, VK_TAB, CommandId::NextTab),
        virtual_key(FCONTROL | FSHIFT, VK_TAB, CommandId::PreviousTab),
        accelerator(FCONTROL, b'1', CommandId::SelectTab1),
        accelerator(FCONTROL, b'2', CommandId::SelectTab2),
        accelerator(FCONTROL, b'3', CommandId::SelectTab3),
        accelerator(FCONTROL, b'4', CommandId::SelectTab4),
        accelerator(FCONTROL, b'5', CommandId::SelectTab5),
        accelerator(FCONTROL, b'6', CommandId::SelectTab6),
        accelerator(FCONTROL, b'7', CommandId::SelectTab7),
        accelerator(FCONTROL, b'8', CommandId::SelectTab8),
        accelerator(FCONTROL, b'9', CommandId::SelectTab9),
        virtual_key(FCONTROL, VK_NUMPAD1, CommandId::SelectTab1),
        virtual_key(FCONTROL, VK_NUMPAD2, CommandId::SelectTab2),
        virtual_key(FCONTROL, VK_NUMPAD3, CommandId::SelectTab3),
        virtual_key(FCONTROL, VK_NUMPAD4, CommandId::SelectTab4),
        virtual_key(FCONTROL, VK_NUMPAD5, CommandId::SelectTab5),
        virtual_key(FCONTROL, VK_NUMPAD6, CommandId::SelectTab6),
        virtual_key(FCONTROL, VK_NUMPAD7, CommandId::SelectTab7),
        virtual_key(FCONTROL, VK_NUMPAD8, CommandId::SelectTab8),
        virtual_key(FCONTROL, VK_NUMPAD9, CommandId::SelectTab9),
        // "+" shares a key with "=" on most layouts, so Ctrl+Shift+= is Ctrl++ as typed.
        virtual_key(FCONTROL, VK_OEM_PLUS, CommandId::ZoomIn),
        virtual_key(FCONTROL | FSHIFT, VK_OEM_PLUS, CommandId::ZoomIn),
        virtual_key(FCONTROL, VK_ADD, CommandId::ZoomIn),
        virtual_key(FCONTROL, VK_OEM_MINUS, CommandId::ZoomOut),
        virtual_key(FCONTROL, VK_SUBTRACT, CommandId::ZoomOut),
        accelerator(FCONTROL, b'0', CommandId::ZoomReset),
        virtual_key(FCONTROL, VK_NUMPAD0, CommandId::ZoomReset),
        accelerator(FCONTROL, b'L', CommandId::TextLeftToRight),
        accelerator(FCONTROL, b'R', CommandId::TextRightToLeft),
        accelerator(FCONTROL | FSHIFT, b'P', CommandId::CommandPalette),
        accelerator(FCONTROL | FSHIFT, b'V', CommandId::MarkdownPreviewCycle),
        accelerator(FALT, b'Z', CommandId::ToggleWordWrap),
    ]
}

const fn accelerator(modifiers: u8, key: u8, command: CommandId) -> AcceleratorSpec {
    virtual_key(modifiers, key as VIRTUAL_KEY, command)
}

const fn virtual_key(modifiers: u8, key: VIRTUAL_KEY, command: CommandId) -> AcceleratorSpec {
    AcceleratorSpec {
        modifiers,
        key,
        command,
    }
}

/// `ACCEL` alone aligns to 2 bytes, but `CreateAcceleratorTableW` rejects a buffer that is not on a
/// 4-byte boundary with `ERROR_NOACCESS`. Where a plain stack array lands differs between debug and
/// optimized builds, so the alignment is pinned rather than left to chance.
#[repr(C, align(4))]
struct AlignedAccelerators<const N: usize>([ACCEL; N]);

#[derive(Debug)]
pub(crate) struct AcceleratorTable(HACCEL);

impl AcceleratorTable {
    pub(crate) fn create() -> Result<Self> {
        let native = AlignedAccelerators(accelerator_specs().map(|spec| ACCEL {
            fVirt: FVIRTKEY | spec.modifiers,
            key: spec.key,
            cmd: spec.command as u16,
        }));
        let handle = unsafe { CreateAcceleratorTableW(native.0.as_ptr(), native.0.len() as i32) };
        if handle.is_null() {
            Err(last_error())
        } else {
            Ok(Self(handle))
        }
    }

    pub(crate) fn raw(&self) -> HACCEL {
        self.0
    }
}

impl Drop for AcceleratorTable {
    fn drop(&mut self) {
        unsafe {
            DestroyAcceleratorTable(self.0);
        }
    }
}

#[derive(Debug)]
pub(crate) struct MenuBar(HMENU);

impl MenuBar {
    pub(crate) fn create() -> Result<Self> {
        let root = unsafe { CreateMenu() };
        if root.is_null() {
            return Err(last_error());
        }
        let result = (|| {
            let file = create_popup(&[
                MenuEntry::command("&New\tCtrl+N", CommandId::New),
                MenuEntry::command("&Open...\tCtrl+O", CommandId::Open),
                MenuEntry::command("&Save\tCtrl+S", CommandId::Save),
                MenuEntry::command("Save &As...\tCtrl+Shift+S", CommandId::SaveAs),
                MenuEntry::command("&Close tab", CommandId::CloseTab),
                MenuEntry::Separator,
                MenuEntry::command("E&xit", CommandId::Exit),
            ])?;
            append_popup(root, MENU_TITLES[0], file)?;
            let edit = create_popup(&[
                MenuEntry::command("&Undo\tCtrl+Z", CommandId::Undo),
                MenuEntry::command("&Redo\tCtrl+Y", CommandId::Redo),
                MenuEntry::Separator,
                MenuEntry::command("Cu&t", CommandId::Cut),
                MenuEntry::command("&Copy", CommandId::Copy),
                MenuEntry::command("&Paste", CommandId::Paste),
            ])?;
            append_popup(root, MENU_TITLES[1], edit)?;
            let search = create_popup(&[
                MenuEntry::command("&Find\tCtrl+F", CommandId::Find),
                MenuEntry::command("&Replace\tCtrl+H", CommandId::Replace),
            ])?;
            append_popup(root, MENU_TITLES[2], search)?;
            let view = create_popup(&[
                MenuEntry::command("Plain text", CommandId::LanguagePlainText),
                MenuEntry::command("JSON", CommandId::LanguageJson),
                MenuEntry::command("Markdown", CommandId::LanguageMarkdown),
                MenuEntry::Separator,
                MenuEntry::command("Zoom &in	Ctrl++", CommandId::ZoomIn),
                MenuEntry::command("Zoom &out	Ctrl+-", CommandId::ZoomOut),
                MenuEntry::command("Reset &zoom	Ctrl+0", CommandId::ZoomReset),
                MenuEntry::Separator,
                MenuEntry::command("&Left-to-right text	Ctrl+L", CommandId::TextLeftToRight),
                MenuEntry::command("&Right-to-left text	Ctrl+R", CommandId::TextRightToLeft),
                MenuEntry::Separator,
                MenuEntry::command("&Word wrap	Alt+Z", CommandId::ToggleWordWrap),
                MenuEntry::command("Line &numbers", CommandId::ToggleLineNumbers),
                MenuEntry::Separator,
                MenuEntry::command(
                    "Markdown preview &side by side",
                    CommandId::MarkdownPreviewSide,
                ),
                MenuEntry::command("Markdown preview f&ull", CommandId::MarkdownPreviewFull),
                MenuEntry::command("Close Markdown pre&view", CommandId::MarkdownPreviewClose),
                MenuEntry::Separator,
                MenuEntry::command(
                    "Command &palette...	Ctrl+Shift+P",
                    CommandId::CommandPalette,
                ),
            ])?;
            append_popup(root, MENU_TITLES[3], view)
        })();
        match result {
            Ok(()) => Ok(Self(root)),
            Err(error) => {
                unsafe {
                    DestroyMenu(root);
                }
                Err(error)
            }
        }
    }

    /// The dropdown under heading `index` of `MENU_TITLES`.
    pub(crate) fn dropdown(&self, index: usize) -> HMENU {
        unsafe { GetSubMenu(self.0, index as i32) }
    }
}

pub(crate) fn translate_accelerator(handle: HACCEL, hwnd: HWND, message: &MSG) -> bool {
    unsafe { TranslateAcceleratorW(hwnd, handle, message) != 0 }
}

/// Grays the View menu's preview entries while the active tab is not Markdown.
pub(crate) fn set_markdown_preview_enabled(menu: HMENU, enabled: bool) {
    let state = MF_BYCOMMAND | if enabled { MF_ENABLED } else { MF_GRAYED };
    for command in [
        CommandId::MarkdownPreviewSide,
        CommandId::MarkdownPreviewFull,
        CommandId::MarkdownPreviewClose,
    ] {
        unsafe { EnableMenuItem(menu, command as u32, state) };
    }
}

impl Drop for MenuBar {
    fn drop(&mut self) {
        unsafe {
            DestroyMenu(self.0);
        }
    }
}

enum MenuEntry {
    Command(&'static str, CommandId),
    Separator,
}

impl MenuEntry {
    const fn command(label: &'static str, command: CommandId) -> Self {
        Self::Command(label, command)
    }
}

fn create_popup(entries: &[MenuEntry]) -> Result<HMENU> {
    let menu = unsafe { CreatePopupMenu() };
    if menu.is_null() {
        return Err(last_error());
    }
    for entry in entries {
        let ok = match entry {
            MenuEntry::Command(label, command) => {
                let label = wide_null(label);
                unsafe { AppendMenuW(menu, MF_STRING, *command as usize, label.as_ptr()) }
            }
            MenuEntry::Separator => unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null()) },
        };
        if ok == 0 {
            unsafe {
                DestroyMenu(menu);
            }
            return Err(last_error());
        }
    }
    Ok(menu)
}

fn append_popup(root: HMENU, label: &str, popup: HMENU) -> Result<()> {
    let label = wide_null(label);
    if unsafe { AppendMenuW(root, MF_POPUP, popup as usize, label.as_ptr()) } == 0 {
        unsafe {
            DestroyMenu(popup);
        }
        Err(last_error())
    } else {
        Ok(())
    }
}

pub(crate) fn show_overflow(hwnd: HWND, x: i32, y: i32) -> Option<CommandId> {
    track_popup(
        hwnd,
        &[
            MenuEntry::command("New", CommandId::New),
            MenuEntry::command("Open...", CommandId::Open),
            MenuEntry::command("Save", CommandId::Save),
            MenuEntry::Separator,
            MenuEntry::command("Find", CommandId::Find),
            MenuEntry::command("Format JSON", CommandId::FormatJson),
            MenuEntry::command("Command palette...", CommandId::CommandPalette),
            MenuEntry::Separator,
            MenuEntry::command("Exit", CommandId::Exit),
        ],
        POINT { x, y },
    )
}

/// The context menu of the empty tab-strip space, at client coordinates `x`, `y`.
pub(crate) fn show_tab_strip_menu(hwnd: HWND, x: i32, y: i32, has_tabs: bool) -> Option<CommandId> {
    let mut entries = vec![
        MenuEntry::command("New tab	Ctrl+N", CommandId::New),
        MenuEntry::command("Open...	Ctrl+O", CommandId::Open),
    ];
    if has_tabs {
        entries.extend([
            MenuEntry::Separator,
            MenuEntry::command("Close all tabs", CommandId::CloseAllTabs),
        ]);
    }
    track_popup(hwnd, &entries, POINT { x, y })
}

fn track_popup(hwnd: HWND, entries: &[MenuEntry], client: POINT) -> Option<CommandId> {
    // TrackPopupMenuEx runs a nested modal loop that reenters the window procedure, exactly as the
    // file dialogs do. Hold deferred, IPC and snapshot work for its duration so the command the
    // user picks still acts on the document that was active when they opened the menu.
    let _modal = ModalScope::enter(hwnd);
    #[cfg(test)]
    if let Some(answer) = POPUP_ANSWERS.with(|answers| answers.borrow_mut().pop_front()) {
        return answer(hwnd);
    }
    let menu = create_popup(entries).ok()?;
    let mut point = client;
    unsafe {
        ClientToScreen(hwnd, &mut point);
    }
    let selected = unsafe {
        TrackPopupMenuEx(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            point.x,
            point.y,
            hwnd,
            std::ptr::null(),
        )
    };
    unsafe {
        DestroyMenu(menu);
    }
    u16::try_from(selected)
        .ok()
        .and_then(|value| CommandId::try_from(value).ok())
}

/// How a menu-band dropdown closed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DropdownExit {
    Command(CommandId),
    /// Left/Right, or the pointer moving onto another heading: open that heading's dropdown next.
    Switch(usize),
    /// Escape closes just the dropdown and leaves its heading highlighted.
    Escape,
    /// A click outside, or on the open heading itself, leaves menu mode.
    Dismissed,
}

struct DropdownTracking {
    current: usize,
    /// Heading rectangles in screen coordinates.
    headings: Vec<RECT>,
    exit: Option<DropdownExit>,
}

thread_local! {
    static DROPDOWN: RefCell<Option<DropdownTracking>> = const { RefCell::new(None) };
}

/// Opens `menu` below heading `current` (`headings` are client rectangles) and reports how it
/// closed. Moving between headings needs a message filter: a popup menu's modal loop otherwise
/// swallows Left/Right and pointer movement over the band.
pub(crate) fn track_dropdown(
    hwnd: HWND,
    menu: HMENU,
    current: usize,
    headings: &[RECT],
) -> DropdownExit {
    let _modal = ModalScope::enter(hwnd);
    #[cfg(test)]
    if let Some(answer) = DROPDOWN_ANSWERS.with(|answers| answers.borrow_mut().pop_front()) {
        return answer(hwnd, current);
    }
    let headings = headings
        .iter()
        .map(|rect| client_to_screen(hwnd, *rect))
        .collect::<Vec<_>>();
    let Some(anchor) = headings.get(current).copied() else {
        return DropdownExit::Dismissed;
    };
    DROPDOWN.with(|tracking| {
        *tracking.borrow_mut() = Some(DropdownTracking {
            current,
            headings,
            exit: None,
        });
    });
    let hook = unsafe {
        SetWindowsHookExW(
            WH_MSGFILTER,
            Some(dropdown_filter),
            std::ptr::null_mut(),
            GetCurrentThreadId(),
        )
    };
    let params = TPMPARAMS {
        cbSize: std::mem::size_of::<TPMPARAMS>() as u32,
        rcExclude: anchor,
    };
    let selected = unsafe {
        TrackPopupMenuEx(
            menu,
            TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN | TPM_VERTICAL,
            anchor.left,
            anchor.bottom,
            hwnd,
            &params,
        )
    };
    if !hook.is_null() {
        unsafe {
            UnhookWindowsHookEx(hook);
        }
    }
    let exit = DROPDOWN.with(|tracking| tracking.borrow_mut().take().and_then(|state| state.exit));
    u16::try_from(selected)
        .ok()
        .and_then(|value| CommandId::try_from(value).ok())
        .map(DropdownExit::Command)
        .or(exit)
        .unwrap_or(DropdownExit::Dismissed)
}

fn client_to_screen(hwnd: HWND, rect: RECT) -> RECT {
    let mut top_left = POINT {
        x: rect.left,
        y: rect.top,
    };
    let mut bottom_right = POINT {
        x: rect.right,
        y: rect.bottom,
    };
    unsafe {
        ClientToScreen(hwnd, &mut top_left);
        ClientToScreen(hwnd, &mut bottom_right);
    }
    RECT {
        left: top_left.x,
        top: top_left.y,
        right: bottom_right.x,
        bottom: bottom_right.y,
    }
}

/// The `WH_MSGFILTER` hook installed for one dropdown: decides from each message of the popup's
/// modal loop whether to close it in favor of a neighboring heading.
unsafe extern "system" fn dropdown_filter(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == MSGF_MENU as i32 && lparam != 0 {
        let message = unsafe { &*(lparam as *const MSG) };
        let exit = DROPDOWN.with(|tracking| {
            let mut tracking = tracking.borrow_mut();
            let state = tracking.as_mut()?;
            let under_pointer = menu_band::heading_at(&state.headings, message.pt.x, message.pt.y);
            let exit = match message.message {
                WM_KEYDOWN if message.wParam == VK_LEFT as usize => {
                    DropdownExit::Switch(menu_band::neighbor(state.current, false))
                }
                WM_KEYDOWN if message.wParam == VK_RIGHT as usize => {
                    DropdownExit::Switch(menu_band::neighbor(state.current, true))
                }
                WM_KEYDOWN if message.wParam == VK_ESCAPE as usize => DropdownExit::Escape,
                WM_MOUSEMOVE => match under_pointer {
                    Some(index) if index != state.current => DropdownExit::Switch(index),
                    _ => return None,
                },
                WM_LBUTTONDOWN if under_pointer == Some(state.current) => DropdownExit::Dismissed,
                _ => return None,
            };
            state.exit = Some(exit);
            Some(exit)
        });
        match exit {
            // The popup's own Escape handling closes it; only the reason is recorded.
            Some(DropdownExit::Escape) | None => {}
            Some(_) => {
                unsafe {
                    EndMenu();
                }
                return 1;
            }
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

#[cfg(test)]
type PopupAnswer = Box<dyn FnOnce(HWND) -> Option<CommandId>>;

#[cfg(test)]
thread_local! {
    static POPUP_ANSWERS: std::cell::RefCell<std::collections::VecDeque<PopupAnswer>> =
        const { std::cell::RefCell::new(std::collections::VecDeque::new()) };
}

#[cfg(test)]
type DropdownAnswer = Box<dyn FnOnce(HWND, usize) -> DropdownExit>;

#[cfg(test)]
thread_local! {
    static DROPDOWN_ANSWERS: std::cell::RefCell<std::collections::VecDeque<DropdownAnswer>> =
        const { std::cell::RefCell::new(std::collections::VecDeque::new()) };
}

/// Answers the next menu-band dropdown (given its heading) instead of tracking a real popup.
#[cfg(test)]
pub(crate) fn answer_next_dropdown(answer: impl FnOnce(HWND, usize) -> DropdownExit + 'static) {
    DROPDOWN_ANSWERS.with(|answers| answers.borrow_mut().push_back(Box::new(answer)));
}

/// Answers the next popup menu from inside its modal scope instead of tracking a real popup.
#[cfg(test)]
pub(crate) fn answer_next_popup_menu(answer: impl FnOnce(HWND) -> Option<CommandId> + 'static) {
    POPUP_ANSWERS.with(|answers| answers.borrow_mut().push_back(Box::new(answer)));
}

#[cfg(test)]
mod tests {
    use super::accelerator_specs;
    use crate::window::commands::CommandId;

    #[test]
    fn shortcut_and_menu_commands_share_command_ids() {
        let specs = accelerator_specs();
        assert!(specs.iter().any(|item| item.command == CommandId::New));
        assert!(specs.iter().any(|item| item.command == CommandId::SaveAs));
        assert!(
            specs
                .iter()
                .any(|item| item.command == CommandId::FormatJson)
        );
        assert_eq!(specs.len(), 42);
    }

    #[test]
    fn the_native_accelerator_table_is_created_from_a_four_byte_aligned_buffer() {
        // Break caught: release builds where the table buffer landed off a 4-byte boundary, so
        // table creation failed with ERROR_NOACCESS and every keyboard shortcut was silently dead.
        assert_eq!(std::mem::align_of::<super::AlignedAccelerators<1>>() % 4, 0);
        super::AcceleratorTable::create().expect("accelerator table");
    }

    #[test]
    fn every_shortcut_chord_maps_to_exactly_one_command() {
        let specs = accelerator_specs();
        for (index, spec) in specs.iter().enumerate() {
            assert!(
                specs[index + 1..]
                    .iter()
                    .all(|other| (other.modifiers, other.key) != (spec.modifiers, spec.key)),
                "duplicate chord for {:?}",
                spec.command
            );
        }
    }

    #[test]
    fn tab_zoom_and_direction_shortcuts_are_bound() {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
            VK_NUMPAD9, VK_OEM_MINUS, VK_OEM_PLUS, VK_TAB,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::{FCONTROL, FSHIFT};
        let bound = |modifiers: u8, key: u16| {
            accelerator_specs()
                .into_iter()
                .find(|spec| spec.modifiers == modifiers && spec.key == key)
                .map(|spec| spec.command)
        };
        assert_eq!(bound(FCONTROL, u16::from(b'T')), Some(CommandId::New));
        assert_eq!(bound(FCONTROL, VK_TAB), Some(CommandId::NextTab));
        assert_eq!(
            bound(FCONTROL | FSHIFT, VK_TAB),
            Some(CommandId::PreviousTab)
        );
        assert_eq!(
            bound(FCONTROL, u16::from(b'1')),
            Some(CommandId::SelectTab1)
        );
        assert_eq!(bound(FCONTROL, VK_NUMPAD9), Some(CommandId::SelectTab9));
        assert_eq!(bound(FCONTROL, VK_OEM_PLUS), Some(CommandId::ZoomIn));
        assert_eq!(bound(FCONTROL, VK_OEM_MINUS), Some(CommandId::ZoomOut));
        assert_eq!(bound(FCONTROL, u16::from(b'0')), Some(CommandId::ZoomReset));
        assert_eq!(
            bound(FCONTROL, u16::from(b'L')),
            Some(CommandId::TextLeftToRight)
        );
        assert_eq!(
            bound(FCONTROL, u16::from(b'R')),
            Some(CommandId::TextRightToLeft)
        );
        assert_eq!(
            bound(FCONTROL | FSHIFT, u16::from(b'P')),
            Some(CommandId::CommandPalette)
        );
        assert_eq!(
            bound(
                windows_sys::Win32::UI::WindowsAndMessaging::FALT,
                u16::from(b'Z')
            ),
            Some(CommandId::ToggleWordWrap)
        );
    }

    #[test]
    fn preview_entries_gray_out_and_re_enable() {
        use super::{MenuBar, set_markdown_preview_enabled};
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetMenuState, MF_BYCOMMAND, MF_GRAYED};
        let bar = MenuBar::create().unwrap();
        let view = bar.dropdown(3);
        set_markdown_preview_enabled(view, false);
        let state =
            unsafe { GetMenuState(view, CommandId::MarkdownPreviewSide as u32, MF_BYCOMMAND) };
        assert_ne!(state & MF_GRAYED, 0);
        set_markdown_preview_enabled(view, true);
        let state =
            unsafe { GetMenuState(view, CommandId::MarkdownPreviewSide as u32, MF_BYCOMMAND) };
        assert_eq!(state & MF_GRAYED, 0);
    }
}
