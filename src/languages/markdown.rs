use crate::editor::scintilla_constants::{
    SCE_MARKDOWN_CODE, SCE_MARKDOWN_DEFAULT, SCE_MARKDOWN_HEADER1,
};
use crate::languages::{LanguageStyles, LexerStyle, rgb};

const MARKDOWN_LIGHT: &[LexerStyle] = &[
    LexerStyle {
        style: SCE_MARKDOWN_DEFAULT,
        foreground: rgb(32, 32, 32),
        background: rgb(255, 255, 255),
        bold: false,
    },
    LexerStyle {
        style: SCE_MARKDOWN_HEADER1,
        foreground: rgb(0, 92, 197),
        background: rgb(255, 255, 255),
        bold: true,
    },
    LexerStyle {
        style: SCE_MARKDOWN_CODE,
        foreground: rgb(110, 65, 15),
        background: rgb(246, 248, 250),
        bold: false,
    },
];

const MARKDOWN_DARK: &[LexerStyle] = &[
    LexerStyle {
        style: SCE_MARKDOWN_DEFAULT,
        foreground: rgb(220, 220, 220),
        background: rgb(30, 30, 30),
        bold: false,
    },
    LexerStyle {
        style: SCE_MARKDOWN_HEADER1,
        foreground: rgb(86, 156, 214),
        background: rgb(30, 30, 30),
        bold: true,
    },
    LexerStyle {
        style: SCE_MARKDOWN_CODE,
        foreground: rgb(215, 186, 125),
        background: rgb(45, 45, 45),
        bold: false,
    },
];

pub static MARKDOWN_STYLES: LanguageStyles = LanguageStyles {
    light: MARKDOWN_LIGHT,
    dark: MARKDOWN_DARK,
};

#[cfg(test)]
mod tests {
    use super::MARKDOWN_STYLES;
    use crate::editor::scintilla_constants::SCE_MARKDOWN_DEFAULT;

    #[test]
    fn light_and_dark_tables_both_style_the_default_markdown_text() {
        // Break caught: a style table missing an entry for SCE_MARKDOWN_DEFAULT leaves ordinary
        // Markdown text uncolored after a theme switch, instead of a deterministic FastPad color.
        assert!(
            MARKDOWN_STYLES
                .light
                .iter()
                .any(|style| style.style == SCE_MARKDOWN_DEFAULT)
        );
        assert!(
            MARKDOWN_STYLES
                .dark
                .iter()
                .any(|style| style.style == SCE_MARKDOWN_DEFAULT)
        );
    }
}
