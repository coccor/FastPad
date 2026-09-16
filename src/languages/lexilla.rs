use crate::platform::{OwnedModule, last_error, wide_null};
use crate::{FastPadError, Result};
use std::ffi::{CString, c_char, c_void};
use std::path::{Path, PathBuf};
use windows_sys::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};

/// `ILexer5* __stdcall CreateLexer(const char *name)` (Lexilla.h). The returned pointer is
/// completely opaque on the Rust side: it is handed straight to Scintilla's `SCI_SETILEXER`, which
/// takes ownership of it (see `Document.h`'s `LexerInstance` / `LexInterface::SetInstance`). FastPad
/// never defines an `ILexer5` vtable and never calls `Release()` on it itself.
type CreateLexerFn = unsafe extern "system" fn(*const c_char) -> *mut c_void;

/// A loaded `Lexilla.dll`, resolved and kept alive only once JSON or Markdown highlighting is
/// first requested; a plain-text-only session never touches this.
#[derive(Debug)]
pub(crate) struct LexillaLibrary {
    _module: OwnedModule,
    create_lexer: CreateLexerFn,
}

impl LexillaLibrary {
    /// Loads `path` with the same safe search flags `bootstrap.rs` uses for Scintilla.dll, then
    /// resolves the `CreateLexer` export by name.
    pub(crate) fn load(path: &Path) -> Result<Self> {
        let text = path.to_str().ok_or(FastPadError::Invariant(
            "Lexilla path was not valid Unicode",
        ))?;
        let wide_path = wide_null(text);
        let raw = unsafe {
            LoadLibraryExW(
                wide_path.as_ptr(),
                std::ptr::null_mut(),
                LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
        };
        let module = unsafe { OwnedModule::from_raw_owned(raw) }?;

        let proc = unsafe { GetProcAddress(module.as_raw(), c"CreateLexer".as_ptr().cast()) };
        let Some(proc) = proc else {
            return Err(last_error());
        };
        // SAFETY: `proc` was just resolved from the "CreateLexer" export, whose signature is
        // pinned by the vendored Lexilla.h header to `ILexer5* __stdcall CreateLexer(const char*)`.
        let create_lexer = unsafe {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, CreateLexerFn>(proc)
        };

        Ok(Self {
            _module: module,
            create_lexer,
        })
    }

    /// Requests a lexer by its Lexilla-registered name (e.g. `"json"`, `"markdown"`). Returns the
    /// opaque pointer as a plain `isize` for `Editor::set_lexer`, or an error if Lexilla does not
    /// recognize the name (it returns null in that case).
    pub(crate) fn create_lexer(&self, name: &str) -> Result<isize> {
        let name = CString::new(name)
            .map_err(|_| FastPadError::Invariant("lexer name may not contain NUL bytes"))?;
        let pointer = unsafe { (self.create_lexer)(name.as_ptr()) };
        if pointer.is_null() {
            return Err(FastPadError::Invariant(
                "Lexilla did not recognize the requested lexer name",
            ));
        }
        Ok(pointer as isize)
    }
}

/// Resolves `Lexilla.dll` next to the running executable, matching the portable-ZIP layout that
/// ships `Scintilla.dll`/`Lexilla.dll` beside `FastPad.exe` (unlike `bootstrap.rs`'s pre-existing,
/// out-of-scope `CARGO_MANIFEST_DIR`-based Scintilla path, which only works from a source checkout).
pub(crate) fn default_dll_path() -> Result<PathBuf> {
    let exe = std::env::current_exe()
        .map_err(|_| FastPadError::Invariant("could not locate the running executable"))?;
    dll_path_from_exe(&exe).ok_or(FastPadError::Invariant(
        "running executable had no parent directory",
    ))
}

fn dll_path_from_exe(exe: &Path) -> Option<PathBuf> {
    Some(exe.parent()?.join("Lexilla.dll"))
}

#[cfg(test)]
mod tests {
    use super::{LexillaLibrary, dll_path_from_exe};
    use std::path::{Path, PathBuf};

    #[test]
    fn dll_path_is_resolved_next_to_the_executable() {
        let exe = Path::new(r"C:\Apps\FastPad\FastPad.exe");
        assert_eq!(
            dll_path_from_exe(exe),
            Some(PathBuf::from(r"C:\Apps\FastPad\Lexilla.dll"))
        );
    }

    #[test]
    fn load_rejects_a_missing_dll_path() {
        let missing = std::env::temp_dir().join("fastpad-lexilla-missing-test.dll");
        let _ = std::fs::remove_file(&missing);

        assert!(LexillaLibrary::load(&missing).is_err());
    }

    #[test]
    fn load_and_create_lexer_succeed_against_the_real_lexilla_dll() {
        // Break caught: a wrong CreateLexer signature/calling convention, or resolving the wrong
        // export, would corrupt the stack or return a bogus pointer instead of a real ILexer5*.
        // Lexilla's lexer catalog is not safe to touch concurrently across OS threads, so this
        // holds the shared native-DLL test lock (see `languages::NATIVE_LEXILLA_TEST_LOCK`).
        let _guard = crate::languages::NATIVE_LEXILLA_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let lexilla = LexillaLibrary::load(&native_lexilla_path()).unwrap();

        let json = lexilla.create_lexer("json").unwrap();
        assert_ne!(json, 0);

        let markdown = lexilla.create_lexer("markdown").unwrap();
        assert_ne!(markdown, 0);
    }

    #[test]
    fn create_lexer_rejects_an_unrecognized_lexer_name() {
        let _guard = crate::languages::NATIVE_LEXILLA_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let lexilla = LexillaLibrary::load(&native_lexilla_path()).unwrap();

        assert!(lexilla.create_lexer("not-a-real-lexer").is_err());
    }

    fn native_lexilla_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("native")
            .join("out")
            .join("x64")
            .join("Lexilla.dll")
    }
}
