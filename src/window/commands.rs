#[repr(u16)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandId {
    New = 100,
    Open,
    Save,
    SaveAs,
    CloseTab,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    Find,
    Replace,
    ValidateJson,
    FormatJson,
    LanguagePlainText,
    LanguageJson,
    LanguageMarkdown,
    Exit,
    CloseAllTabs,
}

impl CommandId {
    /// Commands that act on the active document, and so do nothing while no tab is open.
    pub const fn needs_document(self) -> bool {
        !matches!(
            self,
            Self::New | Self::Open | Self::Exit | Self::CloseAllTabs
        )
    }
}

impl TryFrom<u16> for CommandId {
    type Error = ();

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        const COMMANDS: [CommandId; 19] = [
            CommandId::New,
            CommandId::Open,
            CommandId::Save,
            CommandId::SaveAs,
            CommandId::CloseTab,
            CommandId::Undo,
            CommandId::Redo,
            CommandId::Cut,
            CommandId::Copy,
            CommandId::Paste,
            CommandId::Find,
            CommandId::Replace,
            CommandId::ValidateJson,
            CommandId::FormatJson,
            CommandId::LanguagePlainText,
            CommandId::LanguageJson,
            CommandId::LanguageMarkdown,
            CommandId::Exit,
            CommandId::CloseAllTabs,
        ];
        COMMANDS
            .into_iter()
            .find(|command| *command as u16 == value)
            .ok_or(())
    }
}

pub(crate) fn choose_open_path(
    owner: windows_sys::Win32::Foundation::HWND,
) -> crate::Result<Option<std::path::PathBuf>> {
    crate::platform::dialogs::show_open_dialog(owner)
}

pub(crate) fn choose_save_path(
    owner: windows_sys::Win32::Foundation::HWND,
    suggested_name: &str,
) -> crate::Result<Option<std::path::PathBuf>> {
    crate::platform::dialogs::show_save_dialog(owner, suggested_name)
}

#[cfg(test)]
mod tests {
    use super::CommandId;

    #[test]
    fn native_command_values_are_stable_and_round_trip() {
        assert_eq!(CommandId::New as u16, 100);
        assert_eq!(CommandId::Exit as u16, 117);
        assert_eq!(CommandId::try_from(103), Ok(CommandId::SaveAs));
        assert!(CommandId::try_from(99).is_err());
        assert_eq!(CommandId::try_from(118), Ok(CommandId::CloseAllTabs));
        assert!(!CommandId::New.needs_document());
        assert!(CommandId::Paste.needs_document());
    }
}
