mod json;
mod json_commands;
mod lexilla;
mod markdown;

use crate::Result;
use crate::catppuccin::{self, Flavor};
use crate::document::Language;
use crate::editor::Editor;
use crate::platform::theme::Theme;
use lexilla::LexillaLibrary;
use std::path::{Path, PathBuf};

pub use json_commands::{JsonIssue, format_json, json_invocation_count, validate_json};

/// One Scintilla lexer style's look: `style` is the lexer-specific `SCE_*` style number.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LexerStyle {
    pub(crate) style: u32,
    pub(crate) foreground: u32,
    pub(crate) background: u32,
    pub(crate) bold: bool,
}

/// The syntax roles every language's style table draws from, one set per theme. Each language
/// builds its per-theme tables from these at compile time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SyntaxColors {
    pub(crate) background: u32,
    pub(crate) text: u32,
    pub(crate) string: u32,
    pub(crate) number: u32,
    pub(crate) heading: u32,
    pub(crate) code: u32,
    pub(crate) code_background: u32,
}

const LIGHT_SYNTAX: SyntaxColors = SyntaxColors {
    background: rgb(255, 255, 255),
    text: rgb(32, 32, 32),
    string: rgb(163, 21, 21),
    number: rgb(9, 134, 88),
    heading: rgb(0, 92, 197),
    code: rgb(110, 65, 15),
    code_background: rgb(246, 248, 250),
};

const DARK_SYNTAX: SyntaxColors = SyntaxColors {
    background: rgb(30, 30, 30),
    text: rgb(220, 220, 220),
    string: rgb(206, 145, 120),
    number: rgb(181, 206, 168),
    heading: rgb(86, 156, 214),
    code: rgb(215, 186, 125),
    code_background: rgb(45, 45, 45),
};

/// Catppuccin style guide roles: green strings, peach numbers, red first-level headings.
const fn catppuccin_syntax(flavor: &Flavor) -> SyntaxColors {
    SyntaxColors {
        background: flavor.base,
        text: flavor.text,
        string: flavor.green,
        number: flavor.peach,
        heading: flavor.red,
        code: flavor.blue,
        code_background: flavor.mantle,
    }
}

/// Indexed by `Theme as usize`; order must match `Theme::ALL`.
const SYNTAX_COLORS: [SyntaxColors; Theme::COUNT] = [
    LIGHT_SYNTAX,
    DARK_SYNTAX,
    catppuccin_syntax(&catppuccin::LATTE),
    catppuccin_syntax(&catppuccin::FRAPPE),
    catppuccin_syntax(&catppuccin::MACCHIATO),
    catppuccin_syntax(&catppuccin::MOCHA),
];

#[cfg(test)]
pub(crate) const fn syntax_colors(theme: Theme) -> SyntaxColors {
    SYNTAX_COLORS[theme as usize]
}

/// Evaluates to one style table per theme, indexed by `Theme as usize`, from a language's
/// `const fn(&SyntaxColors) -> [LexerStyle; N]` builder. Meant for a `static` initializer, so every
/// table is built at compile time.
macro_rules! per_theme_styles {
    ($builder:path) => {{
        use $crate::platform::theme::Theme;
        let colors = &$crate::languages::SYNTAX_COLORS;
        let mut tables = [$builder(&colors[0]); Theme::COUNT];
        let mut index = 1;
        while index < Theme::COUNT {
            tables[index] = $builder(&colors[index]);
            index += 1;
        }
        tables
    }};
}
pub(crate) use per_theme_styles;

/// Packs 8-bit components into the `0x00BBGGRR` layout Scintilla's `SCI_STYLESETFORE`/`_BACK`
/// expect (Windows `COLORREF` byte order), so style tables can be written as ordinary `(r, g, b)`.
pub(crate) const fn rgb(r: u32, g: u32, b: u32) -> u32 {
    r | (g << 8) | (b << 16)
}

/// FastPad's compiled font-face fallback; there is no font configuration mechanism yet, so every
/// applied style currently uses this face unconditionally.
const DEFAULT_FONT_FACE: &str = "Consolas";

/// Lexilla's lexer catalog is not safe to touch concurrently from multiple OS threads on the same
/// loaded module (observed as a real `STATUS_ACCESS_VIOLATION` under `cargo test`'s default
/// parallelism). Every test that loads or calls into the real `Lexilla.dll` — here and in
/// `lexilla::tests` — holds this for its duration so the language test suite is reliable
/// regardless of `--test-threads`, matching the integration target's own `--test-threads=1` note.
#[cfg(test)]
pub(crate) static NATIVE_LEXILLA_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Detects the language of a file purely from its extension, case-insensitively. Never touches
/// disk or Win32; this is the pure-logic half of language handling, unit-testable without a
/// window (mirrors the `SearchState` pure-logic/window-integration split).
pub fn detect_language(path: &Path) -> Language {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("json") => Language::Json,
        Some("md") => Language::Markdown,
        _ => Language::PlainText,
    }
}

/// Owns the deferred `Lexilla.dll` and applies a document's language (lexer + style table) to the
/// live editor. `Lexilla.dll` is only ever loaded the first time `apply` is called with
/// `Language::Json` or `Language::Markdown`; a plain-text-only session never touches it.
#[derive(Debug, Default)]
pub struct LanguageManager {
    lexilla: Option<LexillaLibrary>,
    #[cfg(test)]
    dll_path_override: Option<PathBuf>,
}

impl LanguageManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies `language`'s lexer to `editor`. `Language::PlainText` installs Scintilla's null
    /// lexer (`SCI_SETILEXER` with a null pointer), which never touches Lexilla. JSON/Markdown
    /// load `Lexilla.dll` on first use, request the matching lexer by name, install it, and apply
    /// `theme`'s compiled style table. On a Lexilla load or lexer-creation failure, the editor is
    /// left exactly as it was (this method never calls `set_lexer` before a real lexer pointer is
    /// in hand) and the error is returned for the caller's notification.
    pub fn apply(&mut self, editor: &Editor, language: Language, theme: Theme) -> Result<()> {
        match language {
            Language::PlainText => {
                editor.set_lexer(0)?;
                Ok(())
            }
            Language::Json => self.apply_lexer(editor, "json", json::styles(theme)),
            Language::Markdown => self.apply_lexer(editor, "markdown", markdown::styles(theme)),
        }
    }

    fn apply_lexer(&mut self, editor: &Editor, name: &str, table: &[LexerStyle]) -> Result<()> {
        let lexer = self.ensure_lexilla()?.create_lexer(name)?;
        editor.set_lexer(lexer)?;
        editor.clear_all_styles()?;
        for style in table {
            editor.set_style(
                style.style,
                style.foreground,
                style.background,
                style.bold,
                DEFAULT_FONT_FACE,
            )?;
        }
        Ok(())
    }

    fn ensure_lexilla(&mut self) -> Result<&LexillaLibrary> {
        if self.lexilla.is_none() {
            let path = self.dll_path()?;
            self.lexilla = Some(LexillaLibrary::load(&path)?);
        }
        Ok(self
            .lexilla
            .as_ref()
            .expect("just populated above if it was absent"))
    }

    fn dll_path(&self) -> Result<PathBuf> {
        #[cfg(test)]
        if let Some(path) = &self.dll_path_override {
            return Ok(path.clone());
        }
        lexilla::default_dll_path()
    }

    #[cfg(test)]
    pub(crate) fn is_loaded(&self) -> bool {
        self.lexilla.is_some()
    }

    #[cfg(test)]
    pub(crate) fn with_dll_path_for_test(path: PathBuf) -> Self {
        Self {
            lexilla: None,
            dll_path_override: Some(path),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{LanguageManager, detect_language, rgb};
    use crate::document::Language;
    use crate::editor::Editor;
    use crate::editor::scintilla_constants::{SCE_JSON_DEFAULT, SCI_SETILEXER, SCI_STYLESETFORE};
    use crate::platform::theme::Theme;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    #[test]
    fn detects_supported_languages_case_insensitively() {
        assert_eq!(detect_language(Path::new("CONFIG.JSON")), Language::Json);
        assert_eq!(detect_language(Path::new("readme.md")), Language::Markdown);
        assert_eq!(detect_language(Path::new("notes.txt")), Language::PlainText);
    }

    #[test]
    fn rgb_packs_components_in_windows_colorref_byte_order() {
        // Break caught: swapping the byte order would silently flip red and blue on every style.
        assert_eq!(rgb(0x11, 0x22, 0x33), 0x00332211);
    }

    #[test]
    fn apply_plain_text_sends_the_null_lexer_and_never_loads_lexilla() {
        // Break caught: a plain-text-only session must never touch Lexilla.dll.
        let harness = FakeHarness::new();
        let editor = fake_editor(&harness);
        let mut manager = LanguageManager::new();

        manager
            .apply(&editor, Language::PlainText, Theme::Light)
            .unwrap();

        assert_eq!(harness.setilexer_calls(), vec![0]);
        assert!(!manager.is_loaded());
    }

    #[test]
    fn apply_json_with_a_missing_lexilla_dll_returns_an_error_and_leaves_the_editor_untouched() {
        // Break caught: a failed Lexilla load must not install a bad/partial lexer, and must
        // surface an error the caller can show in a notification instead of silently keeping
        // stale state.
        let harness = FakeHarness::new();
        let editor = fake_editor(&harness);
        let missing = std::env::temp_dir().join(format!(
            "fastpad-languages-missing-lexilla-{}-{}.dll",
            std::process::id(),
            next_unique()
        ));
        let mut manager = LanguageManager::with_dll_path_for_test(missing);

        assert!(
            manager
                .apply(&editor, Language::Json, Theme::Light)
                .is_err()
        );

        assert!(harness.setilexer_calls().is_empty());
        assert!(!manager.is_loaded());
    }

    #[test]
    fn json_activation_loads_lexilla_and_installs_a_non_null_lexer() {
        let _guard = super::NATIVE_LEXILLA_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let harness = FakeHarness::new();
        let editor = fake_editor(&harness);
        let mut manager = LanguageManager::with_dll_path_for_test(native_lexilla_path());

        manager
            .apply(&editor, Language::Json, Theme::Light)
            .unwrap();

        let calls = harness.setilexer_calls();
        assert_eq!(calls.len(), 1);
        assert_ne!(calls[0], 0);
        assert!(manager.is_loaded());
    }

    #[test]
    fn lexilla_is_loaded_once_and_reused_across_subsequent_language_switches() {
        // Break caught: reloading Lexilla.dll on every language switch instead of caching it in
        // the lazy `Option<LexillaLibrary>`. Proven by pointing the override at a nonexistent path
        // right after the first load succeeds: a second load attempt would now fail, so the second
        // `apply` only succeeds if it reused the already-loaded library instead of reloading.
        let _guard = super::NATIVE_LEXILLA_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let harness = FakeHarness::new();
        let editor = fake_editor(&harness);
        let mut manager = LanguageManager::with_dll_path_for_test(native_lexilla_path());

        manager
            .apply(&editor, Language::Json, Theme::Light)
            .unwrap();
        manager.dll_path_override = Some(std::env::temp_dir().join(format!(
            "fastpad-languages-lexilla-missing-after-first-load-{}-{}.dll",
            std::process::id(),
            next_unique()
        )));

        manager
            .apply(&editor, Language::Markdown, Theme::Light)
            .unwrap();

        assert_eq!(harness.setilexer_calls().len(), 2);
        assert!(manager.is_loaded());
    }

    #[test]
    fn apply_selects_the_theme_style_table_for_the_default_style() {
        let _guard = super::NATIVE_LEXILLA_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let default_foreground = |theme| {
            let harness = FakeHarness::new();
            let editor = fake_editor(&harness);
            let mut manager = LanguageManager::with_dll_path_for_test(native_lexilla_path());
            manager.apply(&editor, Language::Json, theme).unwrap();
            harness.style_fore_for(SCE_JSON_DEFAULT as usize)
        };

        assert_eq!(
            default_foreground(Theme::Light),
            Some(rgb(32, 32, 32) as isize)
        );
        assert_eq!(
            default_foreground(Theme::Dark),
            Some(rgb(220, 220, 220) as isize)
        );
        assert_eq!(
            default_foreground(Theme::CatppuccinMocha),
            Some(crate::catppuccin::MOCHA.text as isize)
        );
    }

    fn next_unique() -> u64 {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        COUNTER.fetch_add(1, Ordering::Relaxed)
    }

    fn native_lexilla_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("native")
            .join("out")
            .join("x64")
            .join("Lexilla.dll")
    }

    #[derive(Default)]
    struct FakeState {
        setilexer_calls: Vec<isize>,
        style_fore: Vec<(usize, isize)>,
    }

    struct FakeHarness {
        state: Arc<Mutex<FakeState>>,
    }

    impl FakeHarness {
        fn new() -> Self {
            Self {
                state: Arc::new(Mutex::new(FakeState::default())),
            }
        }

        fn direct_ptr(&self) -> isize {
            Arc::as_ptr(&self.state) as isize
        }

        fn setilexer_calls(&self) -> Vec<isize> {
            self.state.lock().unwrap().setilexer_calls.clone()
        }

        fn style_fore_for(&self, style: usize) -> Option<isize> {
            self.state
                .lock()
                .unwrap()
                .style_fore
                .iter()
                .rev()
                .find(|(recorded_style, _)| *recorded_style == style)
                .map(|(_, foreground)| *foreground)
        }
    }

    fn fake_editor(harness: &FakeHarness) -> Editor {
        Editor::test_fixture(fake_direct, harness.direct_ptr())
    }

    unsafe extern "C" fn fake_direct(
        direct_ptr: isize,
        message: u32,
        wparam: usize,
        lparam: isize,
    ) -> isize {
        let shared = unsafe { &*(direct_ptr as *const Mutex<FakeState>) };
        let mut state = shared.lock().unwrap();
        match message {
            SCI_SETILEXER => state.setilexer_calls.push(lparam),
            SCI_STYLESETFORE => state.style_fore.push((wparam, lparam)),
            _ => {}
        }
        0
    }
}
