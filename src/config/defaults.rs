use super::persisted::{Settings, ThemePreference};

/// FastPad's compiled fallback font face, used until a settings file overrides it. Kept separate
/// from `languages::DEFAULT_FONT_FACE` (the per-lexer-style face used while highlighting is
/// deliberately unconfigurable) even though both currently name the same font: this one is a
/// user-facing default meant to be overridden by `fastpad.ini`, that one is an internal constant.
pub const DEFAULT_FONT_FACE: &str = "Consolas";
pub const DEFAULT_FONT_SIZE: u16 = 11;
pub const DEFAULT_TAB_WIDTH: u8 = 4;
pub const DEFAULT_WORD_WRAP: bool = false;
pub const DEFAULT_LINE_NUMBERS: bool = true;
pub const DEFAULT_THEME: ThemePreference = ThemePreference::System;
pub const DEFAULT_RECOVERY_INTERVAL_SECONDS: u32 = 30;
pub const DEFAULT_RESTORE_SESSION: bool = true;

/// FastPad's compiled defaults: Consolas 11pt, 4-wide tabs, word wrap off, line numbers on, system
/// theme, a 30-second crash-recovery interval, and session restore on. Every value a settings file
/// does not (validly) specify keeps whatever `default_settings()` produced.
pub fn default_settings() -> Settings {
    Settings {
        font_face: DEFAULT_FONT_FACE.to_owned(),
        font_size: DEFAULT_FONT_SIZE,
        tab_width: DEFAULT_TAB_WIDTH,
        word_wrap: DEFAULT_WORD_WRAP,
        line_numbers: DEFAULT_LINE_NUMBERS,
        theme: DEFAULT_THEME,
        recovery_interval_seconds: DEFAULT_RECOVERY_INTERVAL_SECONDS,
        restore_session: DEFAULT_RESTORE_SESSION,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_match_the_compiled_defaults() {
        // Break caught: default_settings drifting from the documented compiled defaults silently
        // changes what a brand-new profile (no fastpad.ini yet) looks like.
        let settings = default_settings();
        assert_eq!(settings.font_face, "Consolas");
        assert_eq!(settings.font_size, 11);
        assert_eq!(settings.tab_width, 4);
        assert!(!settings.word_wrap);
        assert!(settings.line_numbers);
        assert_eq!(settings.theme, ThemePreference::System);
        assert_eq!(settings.recovery_interval_seconds, 30);
        assert!(settings.restore_session);
    }
}
