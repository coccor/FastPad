use crate::Result;
use crate::platform::{last_error, wide_null};
use crate::window::commands::CommandId;
use crate::window::modal::ModalScope;
use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    VIRTUAL_KEY, VK_ADD, VK_NUMPAD0, VK_NUMPAD1, VK_NUMPAD2, VK_NUMPAD3, VK_NUMPAD4, VK_NUMPAD5,
    VK_NUMPAD6, VK_NUMPAD7, VK_NUMPAD8, VK_NUMPAD9, VK_OEM_MINUS, VK_OEM_PLUS, VK_SUBTRACT, VK_TAB,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    ACCEL, AppendMenuW, CreateAcceleratorTableW, CreateMenu, CreatePopupMenu,
    DestroyAcceleratorTable, DestroyMenu, DrawMenuBar, FCONTROL, FSHIFT, FVIRTKEY, HACCEL, HMENU,
    MF_POPUP, MF_SEPARATOR, MF_STRING, MSG, SetMenu, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    TrackPopupMenuEx, TranslateAcceleratorW,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AcceleratorSpec {
    pub modifiers: u8,
    pub key: u16,
    pub command: CommandId,
}

pub const fn accelerator_specs() -> [AcceleratorSpec; 39] {
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
            append_popup(root, "&File", file)?;
            let edit = create_popup(&[
                MenuEntry::command("&Undo\tCtrl+Z", CommandId::Undo),
                MenuEntry::command("&Redo\tCtrl+Y", CommandId::Redo),
                MenuEntry::Separator,
                MenuEntry::command("Cu&t", CommandId::Cut),
                MenuEntry::command("&Copy", CommandId::Copy),
                MenuEntry::command("&Paste", CommandId::Paste),
            ])?;
            append_popup(root, "&Edit", edit)?;
            let search = create_popup(&[
                MenuEntry::command("&Find\tCtrl+F", CommandId::Find),
                MenuEntry::command("&Replace\tCtrl+H", CommandId::Replace),
            ])?;
            append_popup(root, "&Search", search)?;
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
            ])?;
            append_popup(root, "&View", view)?;
            let help = create_popup(&[])?;
            append_popup(root, "&Help", help)
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

    pub(crate) fn raw(&self) -> HMENU {
        self.0
    }
}

pub(crate) fn translate_accelerator(handle: HACCEL, hwnd: HWND, message: &MSG) -> bool {
    unsafe { TranslateAcceleratorW(hwnd, handle, message) != 0 }
}

pub(crate) fn attach_menu(hwnd: HWND, menu: HMENU) {
    unsafe {
        SetMenu(hwnd, menu);
        DrawMenuBar(hwnd);
    }
}

pub(crate) fn detach_menu(hwnd: HWND) {
    unsafe {
        SetMenu(hwnd, std::ptr::null_mut());
        DrawMenuBar(hwnd);
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

#[cfg(test)]
type PopupAnswer = Box<dyn FnOnce(HWND) -> Option<CommandId>>;

#[cfg(test)]
thread_local! {
    static POPUP_ANSWERS: std::cell::RefCell<std::collections::VecDeque<PopupAnswer>> =
        const { std::cell::RefCell::new(std::collections::VecDeque::new()) };
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
        assert_eq!(specs.len(), 39);
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
    }
}
