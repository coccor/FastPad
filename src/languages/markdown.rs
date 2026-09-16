use crate::editor::scintilla_constants::{
    SCE_MARKDOWN_CODE, SCE_MARKDOWN_DEFAULT, SCE_MARKDOWN_HEADER1,
};
use crate::languages::{LexerStyle, SyntaxColors, per_theme_styles};
use crate::platform::theme::Theme;

const fn table(colors: &SyntaxColors) -> [LexerStyle; 3] {
    [
        LexerStyle {
            style: SCE_MARKDOWN_DEFAULT,
            foreground: colors.text,
            background: colors.background,
            bold: false,
        },
        LexerStyle {
            style: SCE_MARKDOWN_HEADER1,
            foreground: colors.heading,
            background: colors.background,
            bold: true,
        },
        LexerStyle {
            style: SCE_MARKDOWN_CODE,
            foreground: colors.code,
            background: colors.code_background,
            bold: false,
        },
    ]
}

static MARKDOWN_STYLES: [[LexerStyle; 3]; Theme::COUNT] = per_theme_styles!(table);

pub(crate) fn styles(theme: Theme) -> &'static [LexerStyle] {
    &MARKDOWN_STYLES[theme as usize]
}

#[cfg(test)]
mod tests {
    use super::styles;
    use crate::editor::scintilla_constants::{SCE_MARKDOWN_CODE, SCE_MARKDOWN_DEFAULT};
    use crate::languages::{rgb, syntax_colors};
    use crate::platform::theme::Theme;

    #[test]
    fn every_theme_table_styles_the_default_markdown_text() {
        // Break caught: a style table missing an entry for SCE_MARKDOWN_DEFAULT leaves ordinary
        // Markdown text uncolored after a theme switch, instead of a deterministic FastPad color.
        for theme in Theme::ALL {
            let default = styles(theme)
                .iter()
                .find(|style| style.style == SCE_MARKDOWN_DEFAULT)
                .expect("every theme styles SCE_MARKDOWN_DEFAULT");
            assert_eq!(default.foreground, syntax_colors(theme).text);
        }
    }

    #[test]
    fn light_and_dark_tables_keep_their_original_colors() {
        // Break caught: moving to per-theme tables must not repaint existing light/dark users.
        let code = |theme| {
            styles(theme)
                .iter()
                .find(|style| style.style == SCE_MARKDOWN_CODE)
                .map(|style| (style.foreground, style.background))
        };
        assert_eq!(
            code(Theme::Light),
            Some((rgb(110, 65, 15), rgb(246, 248, 250)))
        );
        assert_eq!(
            code(Theme::Dark),
            Some((rgb(215, 186, 125), rgb(45, 45, 45)))
        );
    }
}
