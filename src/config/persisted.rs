use super::defaults::default_settings;
use crate::Result;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ThemePreference {
    #[default]
    System,
    Light,
    Dark,
    /// Catppuccin that follows the system: Latte when light, Mocha when dark.
    Catppuccin,
    CatppuccinLatte,
    CatppuccinFrappe,
    CatppuccinMacchiato,
    CatppuccinMocha,
}

impl ThemePreference {
    /// Whether resolving this preference needs the system light/dark state. Fixed themes never
    /// read it.
    pub const fn follows_system(self) -> bool {
        matches!(self, Self::System | Self::Catppuccin)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub font_face: String,
    pub font_size: u16,
    pub tab_width: u8,
    pub word_wrap: bool,
    pub line_numbers: bool,
    pub theme: ThemePreference,
    pub recovery_interval_seconds: u32,
    /// Whether the primary window reopens the last session's tabs and closes without prompting.
    pub restore_session: bool,
}

impl Settings {
    /// Applies every field `delta` actually specifies, leaving every other field untouched. Used to
    /// layer a freshly parsed `fastpad.ini` on top of compiled defaults (or, in principle, any prior
    /// `Settings`): an absent or rejected key keeps whatever `self` already had.
    pub fn apply_delta(&mut self, delta: &SettingsDelta) {
        if let Some(font_face) = &delta.font_face {
            self.font_face = font_face.clone();
        }
        if let Some(font_size) = delta.font_size {
            self.font_size = font_size;
        }
        if let Some(tab_width) = delta.tab_width {
            self.tab_width = tab_width;
        }
        if let Some(word_wrap) = delta.word_wrap {
            self.word_wrap = word_wrap;
        }
        if let Some(line_numbers) = delta.line_numbers {
            self.line_numbers = line_numbers;
        }
        if let Some(theme) = delta.theme {
            self.theme = theme;
        }
        if let Some(recovery_interval_seconds) = delta.recovery_interval_seconds {
            self.recovery_interval_seconds = recovery_interval_seconds;
        }
        if let Some(restore_session) = delta.restore_session {
            self.restore_session = restore_session;
        }
    }
}

/// One parse-time problem with a single line of a settings source: either the line was not a
/// recognized `key=value` setting at all, or its key was recognized but its value was rejected.
/// `line` is 1-based, matching how editors and error messages normally report line numbers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettingWarning {
    pub line: usize,
    pub message: String,
}

/// The result of parsing a settings source: every recognized, validly-valued key as `Some`, plus one
/// `SettingWarning` per line that was blank/comment-skipped-free but still invalid or unrecognized.
/// Fields default to `None` (not applied) rather than a compiled default, so `Settings::apply_delta`
/// can tell "the file said nothing about this" apart from "the file explicitly chose the default".
#[derive(Default, Debug, PartialEq)]
pub struct SettingsDelta {
    pub font_face: Option<String>,
    pub font_size: Option<u16>,
    pub tab_width: Option<u8>,
    pub word_wrap: Option<bool>,
    pub line_numbers: Option<bool>,
    pub theme: Option<ThemePreference>,
    pub recovery_interval_seconds: Option<u32>,
    pub restore_session: Option<bool>,
    pub warnings: Vec<SettingWarning>,
}

/// Parses a hand-written, tolerant `.ini`-style settings source: one `key=value` pair per line: ASCII
/// whitespace is trimmed from both the raw line and the split key/value, blank lines and `#` comment
/// lines are skipped, and exactly `font_face`, `font_size`, `tab_width`, `word_wrap`,
/// `line_numbers`, `theme`, `recovery_interval_seconds`, and `restore_session` are recognized. Every
/// line is handled independently: a line with an unknown key, a value that fails to parse, or no `=`
/// at all records one `SettingWarning` and is otherwise skipped — it never discards, and is never
/// affected by, any other line's outcome.
pub fn parse(source: &str) -> SettingsDelta {
    let mut delta = SettingsDelta::default();
    // An editor that saves fastpad.ini with a UTF-8 BOM must not hide its first setting.
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    for (index, raw_line) in source.lines().enumerate() {
        let line_number = index + 1;
        let line = trim_ascii(raw_line);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            delta.warnings.push(SettingWarning {
                line: line_number,
                message: format!("line is not a recognized \"key=value\" setting: {raw_line}"),
            });
            continue;
        };
        apply_line(&mut delta, line_number, trim_ascii(key), trim_ascii(value));
    }
    delta
}

fn apply_line(delta: &mut SettingsDelta, line_number: usize, key: &str, value: &str) {
    match key {
        "font_face" => {
            if value.is_empty() {
                warn(delta, line_number, key, value);
            } else {
                delta.font_face = Some(value.to_owned());
            }
        }
        "font_size" => match value.parse::<u16>() {
            Ok(size) if size > 0 => delta.font_size = Some(size),
            _ => warn(delta, line_number, key, value),
        },
        "tab_width" => match value.parse::<u8>() {
            Ok(width) if width > 0 => delta.tab_width = Some(width),
            _ => warn(delta, line_number, key, value),
        },
        "word_wrap" => match parse_bool(value) {
            Some(word_wrap) => delta.word_wrap = Some(word_wrap),
            None => warn(delta, line_number, key, value),
        },
        "line_numbers" => match parse_bool(value) {
            Some(line_numbers) => delta.line_numbers = Some(line_numbers),
            None => warn(delta, line_number, key, value),
        },
        "theme" => match parse_theme(value) {
            Some(theme) => delta.theme = Some(theme),
            None => warn(delta, line_number, key, value),
        },
        "recovery_interval_seconds" => match value.parse::<u32>() {
            Ok(seconds) if seconds > 0 => delta.recovery_interval_seconds = Some(seconds),
            _ => warn(delta, line_number, key, value),
        },
        "restore_session" => match parse_bool(value) {
            Some(restore_session) => delta.restore_session = Some(restore_session),
            None => warn(delta, line_number, key, value),
        },
        _ => delta.warnings.push(SettingWarning {
            line: line_number,
            message: format!("unknown setting key: {key}"),
        }),
    }
}

fn warn(delta: &mut SettingsDelta, line_number: usize, key: &str, value: &str) {
    delta.warnings.push(SettingWarning {
        line: line_number,
        message: format!("invalid value for {key}: \"{value}\""),
    });
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn parse_theme(value: &str) -> Option<ThemePreference> {
    match value.to_ascii_lowercase().as_str() {
        "system" => Some(ThemePreference::System),
        "light" => Some(ThemePreference::Light),
        "dark" => Some(ThemePreference::Dark),
        "catppuccin" => Some(ThemePreference::Catppuccin),
        "catppuccin-latte" => Some(ThemePreference::CatppuccinLatte),
        "catppuccin-frappe" => Some(ThemePreference::CatppuccinFrappe),
        "catppuccin-macchiato" => Some(ThemePreference::CatppuccinMacchiato),
        "catppuccin-mocha" => Some(ThemePreference::CatppuccinMocha),
        _ => None,
    }
}

fn trim_ascii(value: &str) -> &str {
    value.trim_matches(|c: char| c.is_ascii_whitespace())
}

/// The settings file's fixed location: `%LocalAppData%\FastPad\fastpad.ini`. Resolved fresh on every
/// call (the underlying `LOCALAPPDATA` environment variable does not change during a process's
/// lifetime in practice, so this is cheap and always current).
pub fn settings_file_path() -> Result<PathBuf> {
    Ok(crate::platform::paths::fastpad_data_dir()?.join("fastpad.ini"))
}

/// Resolves and parses the settings file, applying every recognized key onto compiled defaults.
/// Never touches the document editor itself — that is the caller's job once it has a `Settings` in
/// hand. If the path cannot even be resolved (e.g. `LOCALAPPDATA` is unset), falls back to defaults
/// with no warnings: that is an environment problem, not a corrupt-settings problem.
pub fn load() -> (Settings, Vec<SettingWarning>) {
    match settings_file_path() {
        Ok(path) => load_from_path(&path),
        Err(_) => (default_settings(), Vec::new()),
    }
}

/// A missing settings file is not an error and not a warning: a brand-new profile has none yet, and
/// that is not "corrupt" — it simply produces defaults. A file that exists but cannot be read
/// (permissions, not valid UTF-8, ...) is reported as a single line-0 warning (0 meaning
/// "the file as a whole", not any specific line) and also falls back to defaults for every field.
fn load_from_path(path: &Path) -> (Settings, Vec<SettingWarning>) {
    let mut settings = default_settings();
    match std::fs::read(path) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(contents) => {
                let delta = parse(&contents);
                settings.apply_delta(&delta);
                (settings, delta.warnings)
            }
            Err(_) => (
                settings,
                vec![SettingWarning {
                    line: 0,
                    message: "settings file is not valid UTF-8".to_owned(),
                }],
            ),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (settings, Vec::new()),
        Err(error) => (
            settings,
            vec![SettingWarning {
                line: 0,
                message: format!("could not read settings file: {error}"),
            }],
        ),
    }
}

impl ThemePreference {
    /// The `theme=` value that parses back to this preference.
    pub const fn ini_value(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
            Self::Catppuccin => "catppuccin",
            Self::CatppuccinLatte => "catppuccin-latte",
            Self::CatppuccinFrappe => "catppuccin-frappe",
            Self::CatppuccinMacchiato => "catppuccin-macchiato",
            Self::CatppuccinMocha => "catppuccin-mocha",
        }
    }
}

/// Returns `source` with every `key=` line set to `value`, or with `key=value` appended when no
/// line names the key. Everything else is kept byte for byte: comments, other keys, invalid lines,
/// a leading BOM, and the file's line ending style.
pub fn set_setting(source: &str, key: &str, value: &str) -> String {
    let newline = if source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut output = String::with_capacity(source.len() + key.len() + value.len() + 3);
    let mut found = false;
    for raw_line in source.split_inclusive('\n') {
        let content = raw_line.trim_end_matches(['\r', '\n']);
        let bom = content.starts_with('\u{feff}');
        let names_key = trim_ascii(content.trim_start_matches('\u{feff}'))
            .split_once('=')
            .is_some_and(|(line_key, _)| trim_ascii(line_key) == key);
        if names_key {
            found = true;
            if bom {
                output.push('\u{feff}');
            }
            output.push_str(key);
            output.push('=');
            output.push_str(value);
            output.push_str(&raw_line[content.len()..]);
        } else {
            output.push_str(raw_line);
        }
    }
    if !found {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push_str(newline);
        }
        output.push_str(key);
        output.push('=');
        output.push_str(value);
        output.push_str(newline);
    }
    output
}

/// Writes one setting into the settings file, creating the file and its directory when needed.
pub fn save_setting(key: &str, value: &str) -> Result<()> {
    save_setting_to(&settings_file_path()?, key, value)
}

/// A file that exists but is not UTF-8 is left alone rather than overwritten.
pub fn save_setting_to(path: &Path, key: &str, value: &str) -> Result<()> {
    let source = match std::fs::read(path) {
        Ok(bytes) => String::from_utf8(bytes)
            .map_err(|_| crate::FastPadError::Invariant("the settings file is not valid UTF-8"))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::file::saver::save_atomic(path, set_setting(&source, key, value).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setting_a_key_rewrites_only_its_lines_and_keeps_everything_else() {
        // Break caught: a palette toggle that rewrites fastpad.ini from scratch, dropping the
        // user's comments, unknown keys and line endings.
        let source = "\u{feff}# mine\r\ntheme = light\r\nbogus\r\nfont_size=12\r\ntheme=dark";
        let updated = set_setting(source, "theme", "system");
        assert_eq!(
            updated,
            "\u{feff}# mine\r\ntheme=system\r\nbogus\r\nfont_size=12\r\ntheme=system"
        );
        assert_eq!(parse(&updated).theme, Some(ThemePreference::System));
        assert_eq!(set_setting("", "word_wrap", "true"), "word_wrap=true\n");
        assert_eq!(
            set_setting("font_size=12", "tab_width", "2"),
            "font_size=12\ntab_width=2\n"
        );
        assert_eq!(
            set_setting("\u{feff}tab_width=4\n", "tab_width", "8"),
            "\u{feff}tab_width=8\n"
        );
        // A key that only appears inside a comment is not a setting line.
        assert_eq!(
            set_setting("# theme=dark\n", "theme", "light"),
            "# theme=dark\ntheme=light\n"
        );
    }

    #[test]
    fn saving_a_setting_creates_the_file_and_preserves_an_existing_one() {
        let directory = std::env::temp_dir().join(format!(
            "fastpad-save-setting-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&directory);
        let path = directory.join("nested").join("fastpad.ini");
        save_setting_to(&path, "line_numbers", "false").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "line_numbers=false\n"
        );
        save_setting_to(&path, "theme", "dark").unwrap();
        save_setting_to(&path, "line_numbers", "true").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "line_numbers=true\ntheme=dark\n"
        );
        // Break caught: clobbering a file the user saved in another encoding.
        std::fs::write(&path, b"theme=dark\n\xff\n").unwrap();
        assert!(save_setting_to(&path, "theme", "light").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"theme=dark\n\xff\n");
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn bad_key_does_not_discard_valid_keys() {
        // Break caught: one invalid or unrecognized line in fastpad.ini discarding every other,
        // otherwise-valid, key in the same file.
        let delta = parse("font_size=13\ntab_width=nope\ntheme=dark\nunknown=x\n");
        assert_eq!(delta.font_size, Some(13));
        assert_eq!(delta.tab_width, None);
        assert_eq!(delta.theme, Some(ThemePreference::Dark));
        assert_eq!(delta.warnings.len(), 2);
    }

    #[test]
    fn a_leading_utf8_bom_does_not_hide_the_first_setting() {
        // Break caught: an editor that saves fastpad.ini with a UTF-8 BOM makes its first line
        // parse as an unknown key, so that setting is dropped and a warning is shown instead.
        let delta = parse("\u{feff}font_size=13\ntab_width=4\n");

        assert_eq!(delta.font_size, Some(13));
        assert_eq!(delta.tab_width, Some(4));
        assert!(delta.warnings.is_empty(), "{:?}", delta.warnings);
    }

    #[test]
    fn blank_lines_and_comments_are_ignored_without_warnings() {
        let delta = parse("\n  \n# a comment\n   # indented comment\nfont_size=12\n");
        assert_eq!(delta.font_size, Some(12));
        assert!(delta.warnings.is_empty());
    }

    #[test]
    fn keys_and_values_are_trimmed_of_ascii_whitespace() {
        let delta = parse("  font_face = Cascadia Code  \n\ttab_width\t=\t8\t\n");
        assert_eq!(delta.font_face.as_deref(), Some("Cascadia Code"));
        assert_eq!(delta.tab_width, Some(8));
        assert!(delta.warnings.is_empty());
    }

    #[test]
    fn word_wrap_accepts_common_boolean_spellings_case_insensitively() {
        assert_eq!(parse("word_wrap=true").word_wrap, Some(true));
        assert_eq!(parse("word_wrap=ON").word_wrap, Some(true));
        assert_eq!(parse("word_wrap=false").word_wrap, Some(false));
        assert_eq!(parse("word_wrap=No").word_wrap, Some(false));
        let delta = parse("word_wrap=maybe");
        assert_eq!(delta.word_wrap, None);
        assert_eq!(delta.warnings.len(), 1);
    }

    #[test]
    fn line_numbers_accepts_the_same_boolean_spellings_as_word_wrap() {
        // Break caught: line_numbers being ignored or reported as an unknown key leaves no way to
        // hide the gutter.
        assert_eq!(parse("line_numbers=off").line_numbers, Some(false));
        assert_eq!(parse("line_numbers=Yes").line_numbers, Some(true));
        let delta = parse("line_numbers=sometimes");
        assert_eq!(delta.line_numbers, None);
        assert_eq!(delta.warnings.len(), 1);

        let mut settings = default_settings();
        settings.apply_delta(&parse("line_numbers=0"));
        assert!(!settings.line_numbers);
    }

    #[test]
    fn restore_session_defaults_on_and_accepts_the_boolean_spellings() {
        // Break caught: session restore that is off for a brand-new profile, or that cannot be
        // switched off from fastpad.ini.
        assert!(default_settings().restore_session);
        assert_eq!(parse("restore_session=off").restore_session, Some(false));
        assert_eq!(parse("restore_session=Yes").restore_session, Some(true));
        let delta = parse("restore_session=later");
        assert_eq!(delta.restore_session, None);
        assert_eq!(delta.warnings.len(), 1);

        let mut settings = default_settings();
        settings.apply_delta(&parse("restore_session=0"));
        assert!(!settings.restore_session);
    }

    #[test]
    fn every_theme_writes_the_ini_value_that_parses_back_to_it() {
        use ThemePreference::*;
        for theme in [
            System,
            Light,
            Dark,
            Catppuccin,
            CatppuccinLatte,
            CatppuccinFrappe,
            CatppuccinMacchiato,
            CatppuccinMocha,
        ] {
            assert_eq!(
                parse(&format!("theme={}", theme.ini_value())).theme,
                Some(theme)
            );
        }
    }

    #[test]
    fn theme_accepts_every_variant_case_insensitively() {
        assert_eq!(parse("theme=System").theme, Some(ThemePreference::System));
        assert_eq!(parse("theme=LIGHT").theme, Some(ThemePreference::Light));
        assert_eq!(parse("theme=dark").theme, Some(ThemePreference::Dark));
        assert_eq!(
            parse("theme=Catppuccin").theme,
            Some(ThemePreference::Catppuccin)
        );
        assert_eq!(
            parse("theme=catppuccin-latte").theme,
            Some(ThemePreference::CatppuccinLatte)
        );
        assert_eq!(
            parse("theme=catppuccin-frappe").theme,
            Some(ThemePreference::CatppuccinFrappe)
        );
        assert_eq!(
            parse("theme=CATPPUCCIN-MACCHIATO").theme,
            Some(ThemePreference::CatppuccinMacchiato)
        );
        assert_eq!(
            parse("theme=catppuccin-mocha").theme,
            Some(ThemePreference::CatppuccinMocha)
        );
        // Break caught: a typo'd flavor silently falling back to some theme instead of warning.
        assert_eq!(parse("theme=catppuccin-espresso").theme, None);
    }

    #[test]
    fn zero_is_rejected_for_every_positive_numeric_key() {
        // Break caught: accepting a zero-width tab, zero-point font, or zero-second recovery
        // interval produces settings the editor cannot sensibly apply.
        assert_eq!(parse("font_size=0").font_size, None);
        assert_eq!(parse("tab_width=0").tab_width, None);
        assert_eq!(
            parse("recovery_interval_seconds=0").recovery_interval_seconds,
            None
        );
    }

    #[test]
    fn empty_font_face_is_rejected() {
        let delta = parse("font_face=\n");
        assert_eq!(delta.font_face, None);
        assert_eq!(delta.warnings.len(), 1);
    }

    #[test]
    fn a_line_without_an_equals_sign_is_one_warning() {
        let delta = parse("this is not a setting\nfont_size=10\n");
        assert_eq!(delta.font_size, Some(10));
        assert_eq!(delta.warnings.len(), 1);
        assert_eq!(delta.warnings[0].line, 1);
    }

    #[test]
    fn warning_line_numbers_are_one_based_and_match_the_source() {
        let delta = parse("font_size=13\nbogus\ntab_width=4\n");
        assert_eq!(delta.warnings.len(), 1);
        assert_eq!(delta.warnings[0].line, 2);
    }

    #[test]
    fn apply_delta_only_overwrites_fields_the_delta_specifies() {
        let mut settings = default_settings();
        settings.font_size = 99;
        let delta = SettingsDelta {
            tab_width: Some(2),
            ..SettingsDelta::default()
        };
        settings.apply_delta(&delta);
        assert_eq!(settings.font_size, 99);
        assert_eq!(settings.tab_width, 2);
    }

    #[test]
    fn missing_settings_file_produces_defaults_with_no_warnings() {
        let directory = std::env::temp_dir().join(format!(
            "fastpad-settings-test-missing-{}",
            std::process::id()
        ));
        let path = directory.join("fastpad.ini");
        let (settings, warnings) = load_from_path(&path);
        assert_eq!(settings, default_settings());
        assert!(warnings.is_empty());
    }

    #[test]
    fn corrupt_settings_file_still_yields_valid_keys_and_one_warning_per_bad_line() {
        let directory = std::env::temp_dir().join(format!(
            "fastpad-settings-test-corrupt-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("fastpad.ini");
        std::fs::write(&path, "tab_width=8\nfont_size=nope\nunknown=x\n").unwrap();

        let (settings, warnings) = load_from_path(&path);

        assert_eq!(settings.tab_width, 8);
        assert_eq!(settings.font_size, default_settings().font_size);
        assert_eq!(warnings.len(), 2);

        std::fs::remove_dir_all(&directory).unwrap();
    }
}
