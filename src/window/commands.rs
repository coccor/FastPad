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
    NextTab,
    PreviousTab,
    SelectTab1,
    SelectTab2,
    SelectTab3,
    SelectTab4,
    SelectTab5,
    SelectTab6,
    SelectTab7,
    SelectTab8,
    SelectTab9,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    TextLeftToRight,
    TextRightToLeft,
    CommandPalette,
    ThemeSystem,
    ThemeLight,
    ThemeDark,
    ToggleWordWrap,
    ToggleLineNumbers,
    FontSizeIncrease,
    FontSizeDecrease,
    FontSizeReset,
    TabWidth2,
    TabWidth4,
    TabWidth8,
    ThemeCatppuccin,
    ThemeCatppuccinLatte,
    ThemeCatppuccinFrappe,
    ThemeCatppuccinMacchiato,
    ThemeCatppuccinMocha,
}

impl CommandId {
    /// Commands that act on the active document, and so do nothing while no tab is open.
    pub const fn needs_document(self) -> bool {
        !matches!(
            self,
            Self::New
                | Self::Open
                | Self::Exit
                | Self::CloseAllTabs
                | Self::CommandPalette
                | Self::ThemeSystem
                | Self::ThemeLight
                | Self::ThemeDark
                | Self::ToggleWordWrap
                | Self::ToggleLineNumbers
                | Self::FontSizeIncrease
                | Self::FontSizeDecrease
                | Self::FontSizeReset
                | Self::TabWidth2
                | Self::TabWidth4
                | Self::TabWidth8
                | Self::ThemeCatppuccin
                | Self::ThemeCatppuccinLatte
                | Self::ThemeCatppuccinFrappe
                | Self::ThemeCatppuccinMacchiato
                | Self::ThemeCatppuccinMocha
        )
    }

    /// The zero-based tab a `SelectTabN` command activates.
    pub const fn tab_index(self) -> Option<usize> {
        let first = Self::SelectTab1 as u16;
        let value = self as u16;
        if value >= first && value <= Self::SelectTab9 as u16 {
            Some((value - first) as usize)
        } else {
            None
        }
    }
}

impl TryFrom<u16> for CommandId {
    type Error = ();

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        const COMMANDS: [CommandId; 52] = [
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
            CommandId::NextTab,
            CommandId::PreviousTab,
            CommandId::SelectTab1,
            CommandId::SelectTab2,
            CommandId::SelectTab3,
            CommandId::SelectTab4,
            CommandId::SelectTab5,
            CommandId::SelectTab6,
            CommandId::SelectTab7,
            CommandId::SelectTab8,
            CommandId::SelectTab9,
            CommandId::ZoomIn,
            CommandId::ZoomOut,
            CommandId::ZoomReset,
            CommandId::TextLeftToRight,
            CommandId::TextRightToLeft,
            CommandId::CommandPalette,
            CommandId::ThemeSystem,
            CommandId::ThemeLight,
            CommandId::ThemeDark,
            CommandId::ToggleWordWrap,
            CommandId::ToggleLineNumbers,
            CommandId::FontSizeIncrease,
            CommandId::FontSizeDecrease,
            CommandId::FontSizeReset,
            CommandId::TabWidth2,
            CommandId::TabWidth4,
            CommandId::TabWidth8,
            CommandId::ThemeCatppuccin,
            CommandId::ThemeCatppuccinLatte,
            CommandId::ThemeCatppuccinFrappe,
            CommandId::ThemeCatppuccinMacchiato,
            CommandId::ThemeCatppuccinMocha,
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
        assert_eq!(CommandId::try_from(134), Ok(CommandId::TextRightToLeft));
        assert_eq!(CommandId::try_from(135), Ok(CommandId::CommandPalette));
        assert!(!CommandId::CommandPalette.needs_document());
        assert_eq!(CommandId::try_from(146), Ok(CommandId::TabWidth8));
        assert_eq!(
            CommandId::try_from(151),
            Ok(CommandId::ThemeCatppuccinMocha)
        );
        assert!(!CommandId::ToggleWordWrap.needs_document());
    }

    #[test]
    fn select_tab_commands_map_to_zero_based_indices() {
        assert_eq!(CommandId::SelectTab1.tab_index(), Some(0));
        assert_eq!(CommandId::SelectTab9.tab_index(), Some(8));
        assert_eq!(CommandId::NextTab.tab_index(), None);
        assert_eq!(CommandId::ZoomIn.tab_index(), None);
    }
}
