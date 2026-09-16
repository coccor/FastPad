use crate::editor::scintilla_constants::{SCE_JSON_DEFAULT, SCE_JSON_NUMBER, SCE_JSON_STRING};
use crate::languages::{LexerStyle, SyntaxColors, per_theme_styles};
use crate::platform::theme::Theme;

const fn table(colors: &SyntaxColors) -> [LexerStyle; 3] {
    [
        LexerStyle {
            style: SCE_JSON_DEFAULT,
            foreground: colors.text,
            background: colors.background,
            bold: false,
        },
        LexerStyle {
            style: SCE_JSON_STRING,
            foreground: colors.string,
            background: colors.background,
            bold: false,
        },
        LexerStyle {
            style: SCE_JSON_NUMBER,
            foreground: colors.number,
            background: colors.background,
            bold: false,
        },
    ]
}

static JSON_STYLES: [[LexerStyle; 3]; Theme::COUNT] = per_theme_styles!(table);

pub(crate) fn styles(theme: Theme) -> &'static [LexerStyle] {
    &JSON_STYLES[theme as usize]
}

#[cfg(test)]
mod tests {
    use super::styles;
    use crate::editor::scintilla_constants::{SCE_JSON_DEFAULT, SCE_JSON_STRING};
    use crate::languages::{rgb, syntax_colors};
    use crate::platform::theme::Theme;

    #[test]
    fn every_theme_table_styles_the_default_json_text() {
        // Break caught: a style table missing an entry for SCE_JSON_DEFAULT leaves ordinary JSON
        // text uncolored (whatever Scintilla's built-in default happens to be) after a theme
        // switch, instead of a deterministic FastPad color.
        for theme in Theme::ALL {
            let default = styles(theme)
                .iter()
                .find(|style| style.style == SCE_JSON_DEFAULT)
                .expect("every theme styles SCE_JSON_DEFAULT");
            assert_eq!(default.foreground, syntax_colors(theme).text);
            assert_eq!(default.background, syntax_colors(theme).background);
        }
    }

    #[test]
    fn light_and_dark_tables_keep_their_original_colors() {
        // Break caught: moving to per-theme tables must not repaint existing light/dark users.
        let string = |theme| {
            styles(theme)
                .iter()
                .find(|style| style.style == SCE_JSON_STRING)
                .map(|style| style.foreground)
        };
        assert_eq!(string(Theme::Light), Some(rgb(163, 21, 21)));
        assert_eq!(string(Theme::Dark), Some(rgb(206, 145, 120)));
    }
}
