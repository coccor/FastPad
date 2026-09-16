//! System light/dark and high-contrast theme detection.
//!
//! `system_uses_dark_mode` is a straight relocation of the single-shot detector Task 13 wrote
//! directly into `window::main_window` (deliberately minimal at the time: no caching, no live
//! updates — both explicitly deferred to whichever task actually needed them). Its behavior is
//! unchanged; only its location moved, so `apply_language`'s existing call site keeps working
//! exactly as before.
//!
//! `SystemTheme` is new: a combined dark/high-contrast snapshot used by the deferred chrome build
//! (`WM_FASTPAD_BUILD_CHROME`) and its live-update hooks (`WM_SETTINGCHANGE`, `WM_THEMECHANGED`,
//! `WM_DWMCOLORIZATIONCOLORCHANGED`) — the only places new theme queries are added by this task.
//!
//! This is FastPad's only high-contrast detector: painting reads the cached `App::theme` through
//! `window::palette` instead of querying `SPI_GETHIGHCONTRAST` itself.

use crate::config::ThemePreference;

/// One point-in-time snapshot of the system theme state FastPad's chrome cares about.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SystemTheme {
    pub dark: bool,
    pub high_contrast: bool,
}

impl SystemTheme {
    /// Queries the live system theme: `AppsUseLightTheme` for dark/light (same registry value
    /// `system_uses_dark_mode` reads) and `SystemParametersInfoW(SPI_GETHIGHCONTRAST)` for high
    /// contrast. Defaults to light/non-high-contrast when a query fails, matching
    /// `system_uses_dark_mode`'s existing light-on-failure default.
    #[cfg(windows)]
    pub fn detect() -> Self {
        Self {
            dark: system_uses_dark_mode(),
            high_contrast: read_high_contrast(),
        }
    }

    #[cfg(not(windows))]
    pub fn detect() -> Self {
        Self {
            dark: false,
            high_contrast: false,
        }
    }

    /// Resolves the *effective* theme the editor should render with, given the user's configured
    /// `preference`: system-following preferences use this snapshot's dark flag; fixed themes
    /// ignore it. High contrast is reported separately on `self` — callers decide whether and how
    /// to react to it, since it is orthogonal to the theme choice.
    pub const fn effective_theme(&self, preference: ThemePreference) -> Theme {
        Theme::resolve(preference, self.dark)
    }
}

/// One concrete color theme. Every theme's palette and lexer style tables are compiled-in static
/// data indexed by the discriminant, so switching themes never allocates or computes colors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Theme {
    Light,
    Dark,
    CatppuccinLatte,
    CatppuccinFrappe,
    CatppuccinMacchiato,
    CatppuccinMocha,
}

impl Theme {
    pub const COUNT: usize = Self::ALL.len();

    /// Every theme, in discriminant order (`Self::ALL[theme as usize] == theme`).
    pub const ALL: [Self; 6] = [
        Self::Light,
        Self::Dark,
        Self::CatppuccinLatte,
        Self::CatppuccinFrappe,
        Self::CatppuccinMacchiato,
        Self::CatppuccinMocha,
    ];

    /// `system_dark` is only consulted by preferences that follow the system.
    pub const fn resolve(preference: ThemePreference, system_dark: bool) -> Self {
        match preference {
            ThemePreference::System if system_dark => Self::Dark,
            ThemePreference::System | ThemePreference::Light => Self::Light,
            ThemePreference::Dark => Self::Dark,
            ThemePreference::Catppuccin if system_dark => Self::CatppuccinMocha,
            ThemePreference::Catppuccin | ThemePreference::CatppuccinLatte => Self::CatppuccinLatte,
            ThemePreference::CatppuccinFrappe => Self::CatppuccinFrappe,
            ThemePreference::CatppuccinMacchiato => Self::CatppuccinMacchiato,
            ThemePreference::CatppuccinMocha => Self::CatppuccinMocha,
        }
    }

    pub const fn is_dark(self) -> bool {
        !matches!(self, Self::Light | Self::CatppuccinLatte)
    }
}

/// Reads `HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize\AppsUseLightTheme` on
/// every call (no caching). Defaults to light when the value cannot be read.
#[cfg(windows)]
pub fn system_uses_dark_mode() -> bool {
    dark_mode_from_apps_use_light_theme(read_apps_use_light_theme())
}

/// Pure: `1` (or missing/unreadable) means the light theme is in use; `0` means dark.
fn dark_mode_from_apps_use_light_theme(apps_use_light_theme: Option<u32>) -> bool {
    apps_use_light_theme == Some(0)
}

#[cfg(windows)]
fn read_apps_use_light_theme() -> Option<u32> {
    use std::ffi::c_void;
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};

    let subkey =
        crate::platform::wide_null(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    let value_name = crate::platform::wide_null("AppsUseLightTheme");
    let mut data: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            value_name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&raw mut data).cast::<c_void>(),
            &mut size,
        )
    };
    (status == 0).then_some(data)
}

/// Reads Windows' high-contrast accessibility mode via `SystemParametersInfoW(SPI_GETHIGHCONTRAST)`.
/// Defaults to `false` (not high contrast) when the query fails.
#[cfg(windows)]
fn read_high_contrast() -> bool {
    use windows_sys::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SPI_GETHIGHCONTRAST;

    let mut info = HIGHCONTRASTW {
        cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32,
        ..unsafe { std::mem::zeroed() }
    };
    let ok = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            std::mem::size_of::<HIGHCONTRASTW>() as u32,
            (&raw mut info).cast::<std::ffi::c_void>(),
            0,
        )
    };
    ok != 0 && info.dwFlags & HCF_HIGHCONTRASTON != 0
}

#[cfg(test)]
mod tests {
    use super::{SystemTheme, Theme, dark_mode_from_apps_use_light_theme};
    use crate::config::ThemePreference;

    #[test]
    fn light_theme_registry_value_of_one_or_missing_means_not_dark() {
        // Break caught: relocating this detector while flipping its 0/1 interpretation would report
        // dark mode when the system is actually light, and vice versa.
        assert!(!dark_mode_from_apps_use_light_theme(Some(1)));
        assert!(dark_mode_from_apps_use_light_theme(Some(0)));
        assert!(!dark_mode_from_apps_use_light_theme(None));
    }

    #[test]
    fn effective_theme_follows_system_only_for_system_following_preferences() {
        let dark_system = SystemTheme {
            dark: true,
            high_contrast: false,
        };
        let light_system = SystemTheme {
            dark: false,
            high_contrast: false,
        };
        assert_eq!(
            dark_system.effective_theme(ThemePreference::System),
            Theme::Dark
        );
        assert_eq!(
            light_system.effective_theme(ThemePreference::System),
            Theme::Light
        );
        assert_eq!(
            dark_system.effective_theme(ThemePreference::Light),
            Theme::Light
        );
        assert_eq!(
            light_system.effective_theme(ThemePreference::Dark),
            Theme::Dark
        );
        assert_eq!(
            dark_system.effective_theme(ThemePreference::Catppuccin),
            Theme::CatppuccinMocha
        );
        assert_eq!(
            light_system.effective_theme(ThemePreference::Catppuccin),
            Theme::CatppuccinLatte
        );
        for (preference, theme) in [
            (ThemePreference::CatppuccinLatte, Theme::CatppuccinLatte),
            (ThemePreference::CatppuccinFrappe, Theme::CatppuccinFrappe),
            (
                ThemePreference::CatppuccinMacchiato,
                Theme::CatppuccinMacchiato,
            ),
            (ThemePreference::CatppuccinMocha, Theme::CatppuccinMocha),
        ] {
            assert!(!preference.follows_system());
            assert_eq!(dark_system.effective_theme(preference), theme);
            assert_eq!(light_system.effective_theme(preference), theme);
        }
    }

    #[test]
    fn all_lists_every_theme_at_its_discriminant() {
        // Break caught: static per-theme tables are indexed by discriminant, so a reordered `ALL`
        // would paint one theme's chrome with another theme's colors.
        for (index, theme) in Theme::ALL.into_iter().enumerate() {
            assert_eq!(theme as usize, index);
        }
        assert!(!Theme::Light.is_dark());
        assert!(!Theme::CatppuccinLatte.is_dark());
        assert!(Theme::CatppuccinMocha.is_dark());
    }
}
