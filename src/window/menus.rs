use crate::Result;
use crate::platform::{last_error, wide_null};
use crate::window::commands::CommandId;
use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
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

pub const fn accelerator_specs() -> [AcceleratorSpec; 9] {
    [
        accelerator(FCONTROL, b'N', CommandId::New),
        accelerator(FCONTROL, b'O', CommandId::Open),
        accelerator(FCONTROL, b'S', CommandId::Save),
        accelerator(FCONTROL | FSHIFT, b'S', CommandId::SaveAs),
        accelerator(FCONTROL, b'F', CommandId::Find),
        accelerator(FCONTROL, b'H', CommandId::Replace),
        accelerator(FCONTROL, b'Z', CommandId::Undo),
        accelerator(FCONTROL, b'Y', CommandId::Redo),
        accelerator(FCONTROL | FSHIFT, b'F', CommandId::FormatJson),
    ]
}

const fn accelerator(modifiers: u8, key: u8, command: CommandId) -> AcceleratorSpec {
    AcceleratorSpec {
        modifiers,
        key: key as u16,
        command,
    }
}

#[derive(Debug)]
pub(crate) struct AcceleratorTable(HACCEL);

impl AcceleratorTable {
    pub(crate) fn create() -> Result<Self> {
        let native = accelerator_specs().map(|spec| ACCEL {
            fVirt: FVIRTKEY | spec.modifiers,
            key: spec.key,
            cmd: spec.command as u16,
        });
        let handle = unsafe { CreateAcceleratorTableW(native.as_ptr(), native.len() as i32) };
        if handle.is_null() {
            Err(last_error())
        } else {
            Ok(Self(handle))
        }
    }

    pub(crate) fn translate(&self, hwnd: HWND, message: &MSG) -> bool {
        unsafe { TranslateAcceleratorW(hwnd, self.0, message) != 0 }
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

    pub(crate) fn attach(&self, hwnd: HWND) {
        unsafe {
            SetMenu(hwnd, self.0);
            DrawMenuBar(hwnd);
        }
    }

    pub(crate) fn detach(&self, hwnd: HWND) {
        unsafe {
            SetMenu(hwnd, std::ptr::null_mut());
            DrawMenuBar(hwnd);
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
            MenuEntry::Separator => unsafe {
                AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null())
            },
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
    let menu = create_popup(&[
        MenuEntry::command("New", CommandId::New),
        MenuEntry::command("Open...", CommandId::Open),
        MenuEntry::command("Save", CommandId::Save),
        MenuEntry::Separator,
        MenuEntry::command("Find", CommandId::Find),
        MenuEntry::command("Format JSON", CommandId::FormatJson),
        MenuEntry::Separator,
        MenuEntry::command("Exit", CommandId::Exit),
    ])
    .ok()?;
    let mut point = POINT { x, y };
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
mod tests {
    use super::accelerator_specs;
    use crate::window::commands::CommandId;

    #[test]
    fn shortcut_and_menu_commands_share_command_ids() {
        let specs = accelerator_specs();
        assert!(specs.iter().any(|item| item.command == CommandId::New));
        assert!(specs.iter().any(|item| item.command == CommandId::SaveAs));
        assert!(specs.iter().any(|item| item.command == CommandId::FormatJson));
        assert_eq!(specs.len(), 9);
    }
}
