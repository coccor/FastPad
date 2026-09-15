use crate::editor::scintilla_constants::{SCE_JSON_DEFAULT, SCE_JSON_NUMBER, SCE_JSON_STRING};
use crate::languages::{LanguageStyles, LexerStyle, rgb};

const JSON_LIGHT: &[LexerStyle] = &[
    LexerStyle {
        style: SCE_JSON_DEFAULT,
        foreground: rgb(32, 32, 32),
        background: rgb(255, 255, 255),
        bold: false,
    },
    LexerStyle {
        style: SCE_JSON_STRING,
        foreground: rgb(163, 21, 21),
        background: rgb(255, 255, 255),
        bold: false,
    },
    LexerStyle {
        style: SCE_JSON_NUMBER,
        foreground: rgb(9, 134, 88),
        background: rgb(255, 255, 255),
        bold: false,
    },
];

const JSON_DARK: &[LexerStyle] = &[
    LexerStyle {
        style: SCE_JSON_DEFAULT,
        foreground: rgb(220, 220, 220),
        background: rgb(30, 30, 30),
        bold: false,
    },
    LexerStyle {
        style: SCE_JSON_STRING,
        foreground: rgb(206, 145, 120),
        background: rgb(30, 30, 30),
        bold: false,
    },
    LexerStyle {
        style: SCE_JSON_NUMBER,
        foreground: rgb(181, 206, 168),
        background: rgb(30, 30, 30),
        bold: false,
    },
];

pub static JSON_STYLES: LanguageStyles = LanguageStyles {
    light: JSON_LIGHT,
    dark: JSON_DARK,
};

#[cfg(test)]
mod tests {
    use super::JSON_STYLES;
    use crate::editor::scintilla_constants::SCE_JSON_DEFAULT;

    #[test]
    fn light_and_dark_tables_both_style_the_default_json_text() {
        // Break caught: a style table missing an entry for SCE_JSON_DEFAULT leaves ordinary JSON
        // text uncolored (whatever Scintilla's built-in default happens to be) after a theme
        // switch, instead of a deterministic FastPad color.
        assert!(
            JSON_STYLES
                .light
                .iter()
                .any(|style| style.style == SCE_JSON_DEFAULT)
        );
        assert!(
            JSON_STYLES
                .dark
                .iter()
                .any(|style| style.style == SCE_JSON_DEFAULT)
        );
    }
}
