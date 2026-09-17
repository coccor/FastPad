//! The preview's color roles per theme. Backgrounds always equal the editor's so Split mode reads
//! as one surface; high contrast takes system colors.

use crate::catppuccin::{self, Flavor};
use crate::languages::rgb;
use crate::platform::theme::Theme;
use crate::window::palette::Palette;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ColorRole {
    Background,
    Text,
    Muted,
    Heading,
    Link,
    CodeBackground,
    Border,
    QuoteBar,
    TableStripe,
    Focus,
    Mark,
    KbdBorder,
}

impl ColorRole {
    pub const ALL: [ColorRole; 12] = [
        Self::Background,
        Self::Text,
        Self::Muted,
        Self::Heading,
        Self::Link,
        Self::CodeBackground,
        Self::Border,
        Self::QuoteBar,
        Self::TableStripe,
        Self::Focus,
        Self::Mark,
        Self::KbdBorder,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreviewColors {
    pub background: u32,
    pub text: u32,
    pub muted: u32,
    pub heading: u32,
    pub link: u32,
    pub code_background: u32,
    pub border: u32,
    pub quote_bar: u32,
    pub table_stripe: u32,
    pub focus: u32,
    pub mark: u32,
    pub kbd_border: u32,
}

impl PreviewColors {
    pub fn get(&self, role: ColorRole) -> u32 {
        match role {
            ColorRole::Background => self.background,
            ColorRole::Text => self.text,
            ColorRole::Muted => self.muted,
            ColorRole::Heading => self.heading,
            ColorRole::Link => self.link,
            ColorRole::CodeBackground => self.code_background,
            ColorRole::Border => self.border,
            ColorRole::QuoteBar => self.quote_bar,
            ColorRole::TableStripe => self.table_stripe,
            ColorRole::Focus => self.focus,
            ColorRole::Mark => self.mark,
            ColorRole::KbdBorder => self.kbd_border,
        }
    }

    /// Whether the preview background is dark, for `<picture>` sources that depend on the scheme.
    pub fn is_dark(&self) -> bool {
        let channel = |shift: u32| (self.background >> shift) & 0xFF;
        299 * channel(0) + 587 * channel(8) + 114 * channel(16) < 128_000
    }
}

const GITHUB_LIGHT: PreviewColors = PreviewColors {
    background: rgb(255, 255, 255),
    text: rgb(31, 35, 40),
    muted: rgb(89, 99, 110),
    heading: rgb(31, 35, 40),
    link: rgb(9, 105, 218),
    code_background: rgb(246, 248, 250),
    border: rgb(209, 217, 224),
    quote_bar: rgb(209, 217, 224),
    table_stripe: rgb(246, 248, 250),
    focus: rgb(9, 105, 218),
    mark: rgb(255, 248, 197),
    kbd_border: rgb(209, 217, 224),
};

const GITHUB_DARK: PreviewColors = PreviewColors {
    background: rgb(30, 30, 30),
    text: rgb(230, 237, 243),
    muted: rgb(145, 152, 161),
    heading: rgb(230, 237, 243),
    link: rgb(68, 147, 248),
    code_background: rgb(45, 45, 45),
    border: rgb(61, 68, 77),
    quote_bar: rgb(61, 68, 77),
    table_stripe: rgb(37, 37, 38),
    focus: rgb(68, 147, 248),
    mark: rgb(69, 56, 25),
    kbd_border: rgb(61, 68, 77),
};

const fn catppuccin_colors(flavor: &Flavor) -> PreviewColors {
    PreviewColors {
        background: flavor.base,
        text: flavor.text,
        muted: flavor.subtext0,
        heading: flavor.text,
        link: flavor.blue,
        code_background: flavor.mantle,
        border: flavor.surface1,
        quote_bar: flavor.surface1,
        table_stripe: catppuccin::blend(flavor.surface0, flavor.base, 96),
        focus: flavor.blue,
        mark: catppuccin::blend(flavor.yellow, flavor.base, 64),
        kbd_border: flavor.surface1,
    }
}

pub fn preview_colors(theme: Theme, high_contrast: bool) -> PreviewColors {
    let palette = Palette::for_theme(theme, high_contrast);
    if high_contrast {
        let link = unsafe {
            windows_sys::Win32::Graphics::Gdi::GetSysColor(
                windows_sys::Win32::Graphics::Gdi::COLOR_HOTLIGHT,
            )
        };
        let link = if link == palette.editor_background {
            palette.editor_foreground
        } else {
            link
        };
        return PreviewColors {
            background: palette.editor_background,
            text: palette.editor_foreground,
            muted: palette.editor_foreground,
            heading: palette.editor_foreground,
            link,
            code_background: palette.editor_background,
            border: palette.editor_foreground,
            quote_bar: palette.editor_foreground,
            table_stripe: palette.editor_background,
            focus: palette.selection_background,
            mark: palette.editor_background,
            kbd_border: palette.editor_foreground,
        };
    }
    let colors = match theme {
        Theme::Light => GITHUB_LIGHT,
        Theme::Dark => GITHUB_DARK,
        Theme::CatppuccinLatte => catppuccin_colors(&catppuccin::LATTE),
        Theme::CatppuccinFrappe => catppuccin_colors(&catppuccin::FRAPPE),
        Theme::CatppuccinMacchiato => catppuccin_colors(&catppuccin::MACCHIATO),
        Theme::CatppuccinMocha => catppuccin_colors(&catppuccin::MOCHA),
    };
    PreviewColors {
        background: palette.editor_background,
        ..colors
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window::palette::Palette;

    #[test]
    fn preview_background_matches_the_editor_for_every_theme() {
        for theme in Theme::ALL {
            for high_contrast in [false, true] {
                assert_eq!(
                    preview_colors(theme, high_contrast).background,
                    Palette::for_theme(theme, high_contrast).editor_background
                );
            }
        }
    }

    #[test]
    fn text_roles_are_distinguishable_from_the_background() {
        for theme in Theme::ALL {
            for high_contrast in [false, true] {
                let colors = preview_colors(theme, high_contrast);
                for role in [
                    ColorRole::Text,
                    ColorRole::Muted,
                    ColorRole::Heading,
                    ColorRole::Link,
                ] {
                    assert_ne!(colors.get(role), colors.background, "{theme:?} {role:?}");
                }
            }
        }
    }

    #[test]
    fn catppuccin_uses_the_style_guide_roles() {
        let mocha = preview_colors(Theme::CatppuccinMocha, false);
        assert_eq!(mocha.link, crate::catppuccin::MOCHA.blue);
        assert_eq!(mocha.border, crate::catppuccin::MOCHA.surface1);
        assert_eq!(mocha.muted, crate::catppuccin::MOCHA.subtext0);
        assert_eq!(mocha.code_background, crate::catppuccin::MOCHA.mantle);
    }

    #[test]
    fn roles_index_their_fields() {
        let colors = preview_colors(Theme::Light, false);
        assert_eq!(colors.get(ColorRole::Link), colors.link);
        assert_eq!(ColorRole::ALL[ColorRole::Focus as usize], ColorRole::Focus);
    }

    #[test]
    fn mark_and_keyboard_roles_stand_out_on_every_theme() {
        for theme in Theme::ALL {
            let colors = preview_colors(theme, false);
            assert_ne!(colors.mark, colors.background, "{theme:?}");
            assert_ne!(colors.kbd_border, colors.background, "{theme:?}");
        }
    }

    #[test]
    fn darkness_follows_the_background() {
        assert!(!preview_colors(Theme::Light, false).is_dark());
        assert!(preview_colors(Theme::Dark, false).is_dark());
        assert!(preview_colors(Theme::CatppuccinMocha, false).is_dark());
        assert!(!preview_colors(Theme::CatppuccinLatte, false).is_dark());
    }
}
