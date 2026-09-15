# FastPad MVP Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build and release the complete FastPad MVP: a startup-latency-first Windows 10/11 x64 text editor using raw Win32, Scintilla, Lexilla, and Rust.

**Architecture:** A single Win32 UI thread creates one Scintilla editor before any optional initialization. Scintilla document handles back tabs; Rust owns only metadata. Settings, file loading, Lexilla, recovery, and named-pipe IPC enter through deferred `WM_APP` work after the first accepted input.

**Tech Stack:** Stable Rust, `windows-sys 0.61.2`, `serde_json 1.0.151`, Scintilla 5.6.6, Lexilla 5.5.3, MSVC 2022 Build Tools, PowerShell 7, GitHub Actions Windows runners.

**Spec:** `docs/superpowers/specs/2026-08-31-fastpad-mvp-design.md`

## Global Constraints

- Target Windows 10 and Windows 11 on x64 only.
- The distributable is a portable ZIP containing `FastPad.exe`, `Scintilla.dll`, `Lexilla.dll`, and license files.
- Scintilla is the only authoritative live text buffer; never store a persistent duplicate Rust `String`.
- Before first input, permit only counters, minimal launch/instance parsing, DPI setup, Scintilla loading, main/editor window creation, showing, focusing, and message dispatch.
- Defer settings, full chrome, file loading, Lexilla, recovery, IPC server creation, diagnostics formatting, and logging.
- Use no UI framework, async runtime, dependency-injection container, event bus, generalized error framework, network-capable runtime crate, or custom allocator.
- Use `serde_json` only for explicit Validate JSON and Format JSON commands.
- New documents use UTF-8 without BOM; supported file encodings are UTF-8, UTF-8 BOM, UTF-16 LE, and UTF-16 BE.
- Default to existing-instance reuse; `--new-window` bypasses it. Do not implement resident mode or clean-session restoration.
- Release targets: warm TTI below 25 ms p50 and 40 ms p95, warm first paint below 20 ms, cold TTI below 100 ms where the environment permits, and idle private working set below 20 MB.
- Run every implementation task test-first and commit only after its focused and regression tests pass.

## File Map

| Path | Responsibility |
|---|---|
| `Cargo.toml` | Package metadata, minimal runtime dependencies, release profile |
| `rust-toolchain.toml` | Stable toolchain and required components |
| `src/main.rs` | Windows-subsystem entry and top-level exit behavior |
| `src/lib.rs` | Testable module graph |
| `src/bootstrap.rs` | Strict pre-input bootstrap and deferred message posting |
| `src/app.rs` | Flat application state and Win32 dispatch |
| `src/error.rs` | Small application error enum |
| `src/launch.rs` | Minimal command-line interpretation |
| `src/document.rs` | Document metadata and Scintilla-handle ownership |
| `src/window/*` | Main window, integrated title tabs, menus, notifications, commands |
| `src/editor/*` | Typed Scintilla messages, generated constants, document operations |
| `src/file/*` | Encoding, deferred loading, atomic saving |
| `src/languages/*` | Extension detection, deferred Lexilla, JSON/Markdown styles |
| `src/config/*` | Compiled defaults and INI-like delta parser |
| `src/recovery/*` | Idle snapshots and deferred restore |
| `src/ipc/*` | Framed protocol, mutex, overlapped named-pipe server |
| `src/platform/*` | Unsafe Win32 boundary and owned native handles |
| `src/perf/*` | Allocation-free startup timestamps and diagnostic transport |
| `src/bin/fastpad-bench.rs` | External startup-distribution harness |
| `tests/windows/*` | Real Win32/Scintilla integration scenarios |
| `native/dependencies.json` | Native versions, URLs, and SHA-256 values |
| `tools/*.ps1` | Native acquisition/build, packaging, dependency audit, benchmark runner |
| `.github/workflows/ci.yml` | Linux unit, Windows integration, packaging, and trend jobs |

---

### Task 1: Rust foundation and launch contract

**Files:**
- Create: `Cargo.toml`
- Create: `rust-toolchain.toml`
- Create: `src/lib.rs`
- Create: `src/main.rs`
- Create: `src/error.rs`
- Create: `src/launch.rs`
- Test: `src/launch.rs`

**Interfaces:**
- Consumes: none
- Produces: `LaunchOptions`, `LaunchRequest`, `FastPadError`, `Result<T>`, and `bootstrap::run(LaunchOptions)` call site

- [ ] **Step 1: Write command-line parsing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn os(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn empty_launch_creates_new_document() {
        assert_eq!(parse(os(&[])), Ok(LaunchOptions::default()));
    }

    #[test]
    fn file_and_flags_are_order_independent() {
        assert_eq!(
            parse(os(&["notes.md", "--diagnostic", "--new-window"])),
            Ok(LaunchOptions {
                new_window: true,
                diagnostic: true,
                request: LaunchRequest::Open(OsString::from("notes.md")),
            })
        );
    }

    #[test]
    fn two_paths_are_rejected() {
        assert_eq!(parse(os(&["a.txt", "b.txt"])), Err(LaunchError::MultiplePaths));
    }
}
```

- [ ] **Step 2: Run the focused test and confirm the red state**

Run: `cargo test launch::tests --lib`  
Expected: FAIL because the Cargo package and launch types do not exist.

- [ ] **Step 3: Add the minimal package and launch implementation**

Use edition 2024, `windows-sys = "0.61.2"`, and `serde_json = "1.0.151"`. Enable only these `windows-sys` features: `Win32_Foundation`, `Win32_Graphics_Dwm`, `Win32_Graphics_Gdi`, `Win32_Security`, `Win32_Storage_FileSystem`, `Win32_System_Com`, `Win32_System_IO`, `Win32_System_LibraryLoader`, `Win32_System_Performance`, `Win32_System_Pipes`, `Win32_System_ProcessStatus`, `Win32_System_Threading`, `Win32_UI_Accessibility`, `Win32_UI_Controls`, `Win32_UI_HiDpi`, `Win32_UI_Shell`, and `Win32_UI_WindowsAndMessaging`.

```rust
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LaunchOptions {
    pub new_window: bool,
    pub diagnostic: bool,
    pub request: LaunchRequest,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum LaunchRequest {
    #[default]
    New,
    Open(std::ffi::OsString),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LaunchError {
    UnknownFlag,
    MultiplePaths,
}

pub fn parse(args: impl IntoIterator<Item = std::ffi::OsString>) -> core::result::Result<LaunchOptions, LaunchError> {
    let mut options = LaunchOptions::default();
    let mut path = None;
    for arg in args {
        match arg.to_str() {
            Some("--new-window") => options.new_window = true,
            Some("--diagnostic") => options.diagnostic = true,
            Some(value) if value.starts_with('-') => return Err(LaunchError::UnknownFlag),
            _ if path.is_some() => return Err(LaunchError::MultiplePaths),
            _ => path = Some(arg),
        }
    }
    if let Some(path) = path {
        options.request = LaunchRequest::Open(path);
    }
    Ok(options)
}
```

Define `FastPadError::{Launch(LaunchError), Win32(u32), Io(std::io::Error), UnsupportedEncoding, Json(serde_json::Error), Ipc(&'static str), Invariant(&'static str)}` with manual `Display`, `Error`, and `From` implementations. Define `pub type Result<T> = std::result::Result<T, FastPadError>`.

In `main.rs`, use `#![cfg_attr(windows, windows_subsystem = "windows")]`; on Windows parse `args_os().skip(1)` and call `fastpad::bootstrap::run`, and on non-Windows print that the executable requires Windows. Keep `bootstrap::run` as a temporary test-only stub returning `Ok(0)` in `lib.rs`; Task 6 replaces it.

- [ ] **Step 4: Verify the foundation on the host**

Run: `cargo fmt --check && cargo test --lib && cargo clippy --all-targets -- -D warnings`  
Expected: all launch tests pass and Clippy reports no warnings.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock rust-toolchain.toml src
git commit -m "build: establish FastPad Rust foundation"
```

---

### Task 2: UTF encoding boundary

**Files:**
- Create: `src/file/mod.rs`
- Create: `src/file/encoding.rs`
- Modify: `src/lib.rs`
- Test: `src/file/encoding.rs`

**Interfaces:**
- Consumes: `FastPadError::UnsupportedEncoding`
- Produces: `Encoding`, `DecodedText`, `decode(&[u8])`, and `encode(&str, Encoding)`

- [ ] **Step 1: Write encoding tests**

```rust
#[test]
fn detects_all_supported_encodings() {
    assert_eq!(decode(b"hello").unwrap().encoding, Encoding::Utf8);
    assert_eq!(decode(b"\xEF\xBB\xBFhello").unwrap().encoding, Encoding::Utf8Bom);
    assert_eq!(decode(b"\xFF\xFEh\0i\0").unwrap().encoding, Encoding::Utf16Le);
    assert_eq!(decode(b"\xFE\xFF\0h\0i").unwrap().encoding, Encoding::Utf16Be);
}

#[test]
fn every_supported_encoding_round_trips_non_ascii_text() {
    for encoding in [Encoding::Utf8, Encoding::Utf8Bom, Encoding::Utf16Le, Encoding::Utf16Be] {
        let bytes = encode("zăpadă 🦀", encoding);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.text, "zăpadă 🦀");
        assert_eq!(decoded.encoding, encoding);
    }
}

#[test]
fn rejects_invalid_utf8_without_a_bom() {
    assert!(matches!(decode(&[0x80]), Err(FastPadError::UnsupportedEncoding)));
}
```

- [ ] **Step 2: Run the focused red test**

Run: `cargo test file::encoding::tests --lib`  
Expected: FAIL because `Encoding`, `decode`, and `encode` are absent.

- [ ] **Step 3: Implement BOM-first decoding and preserving encoding**

```rust
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Encoding { #[default] Utf8, Utf8Bom, Utf16Le, Utf16Be }

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedText { pub text: String, pub encoding: Encoding }

pub fn decode(bytes: &[u8]) -> Result<DecodedText> {
    let (text, encoding) = if let Some(body) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        (std::str::from_utf8(body).map_err(|_| FastPadError::UnsupportedEncoding)?.to_owned(), Encoding::Utf8Bom)
    } else if let Some(body) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        (decode_utf16(body, u16::from_le_bytes)?, Encoding::Utf16Le)
    } else if let Some(body) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        (decode_utf16(body, u16::from_be_bytes)?, Encoding::Utf16Be)
    } else {
        (std::str::from_utf8(bytes).map_err(|_| FastPadError::UnsupportedEncoding)?.to_owned(), Encoding::Utf8)
    };
    Ok(DecodedText { text, encoding })
}

fn decode_utf16(bytes: &[u8], read: fn([u8; 2]) -> u16) -> Result<String> {
    if bytes.len() % 2 != 0 { return Err(FastPadError::UnsupportedEncoding); }
    let units = bytes.chunks_exact(2).map(|c| read([c[0], c[1]]));
    char::decode_utf16(units).collect::<core::result::Result<String, _>>()
        .map_err(|_| FastPadError::UnsupportedEncoding)
}
```

Implement `encode` with the appropriate BOM and `encode_utf16`, writing each unit with `to_le_bytes` or `to_be_bytes`.

- [ ] **Step 4: Verify all pure tests**

Run: `cargo test --lib`  
Expected: all launch and encoding tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/file src/lib.rs
git commit -m "feat: add supported text encodings"
```

---

### Task 3: Reproducible Scintilla and Lexilla artifacts

**Files:**
- Create: `native/dependencies.json`
- Create: `tools/fetch-native.ps1`
- Create: `tools/build-native.ps1`
- Create: `tools/generate-scintilla-constants.ps1`
- Create: `src/editor/scintilla_constants.rs`
- Create: `licenses/README.md`
- Modify: `.gitignore`

**Interfaces:**
- Consumes: Scintilla 5.6.6 and Lexilla 5.5.3 source archives
- Produces: `native/out/x64/Scintilla.dll`, `native/out/x64/Lexilla.dll`, generated Rust constants, and copied license texts

- [ ] **Step 1: Write the native dependency manifest**

Record these immutable inputs:

```json
{
  "scintilla": {
    "version": "5.6.6",
    "url": "https://www.scintilla.org/scintilla566.zip",
    "sha256": "a0c0cdf1cf226dc6252020ce9a87a939a8b615e65269db40ac9123b28fa20a9d"
  },
  "lexilla": {
    "version": "5.5.3",
    "url": "https://www.scintilla.org/lexilla553.zip",
    "sha256": "2092b1dd18355321717e3bde25148e4c87e691723ca2b06a65e29a307c5462a6"
  }
}
```

`fetch-native.ps1` must reject any archive whose computed lowercase hash does not equal this manifest before extraction.

- [ ] **Step 2: Add a failing verification mode**

`fetch-native.ps1 -VerifyOnly` must exit nonzero when an archive is missing, its SHA-256 differs, `scintilla/version.txt` is not `566`, or `lexilla/version.txt` is not `553`. It must never silently redownload in verification mode.

Run on Windows PowerShell: `pwsh -File tools/fetch-native.ps1 -VerifyOnly`  
Expected: FAIL because the archives have not been fetched.

- [ ] **Step 3: Implement acquisition and MSVC builds**

Use `Invoke-WebRequest`, `Get-FileHash -Algorithm SHA256`, and `Expand-Archive`. Extract peer directories `native/src/scintilla` and `native/src/lexilla`, then execute:

```powershell
Push-Location native/src/scintilla/win32
& nmake.exe /nologo -f scintilla.mak
if ($LASTEXITCODE -ne 0) { throw "Scintilla build failed" }
Pop-Location

Push-Location native/src/lexilla/src
& nmake.exe /nologo -f lexilla.mak
if ($LASTEXITCODE -ne 0) { throw "Lexilla build failed" }
Pop-Location
```

Copy only the x64 release DLLs into `native/out/x64`, copy both upstream licenses into `licenses`, and fail unless `dumpbin /headers` reports `machine (x64)` for each DLL. Add `/native/cache/`, `/native/src/`, and `/native/out/` to `.gitignore`; the manifest, scripts, generated constants, and licenses remain tracked.

- [ ] **Step 4: Generate only the Scintilla API subset FastPad uses**

`generate-scintilla-constants.ps1` parses `Scintilla.iface` and `LexicalStyles.iface` for a fixed `$RequiredNames` array. Include `SCI_GETDIRECTFUNCTION`, `SCI_GETDIRECTPOINTER`, `SCI_SETCODEPAGE`, `SCI_SETTEXT`, `SCI_GETTEXT`, `SCI_GETTEXTLENGTH`, `SCI_GETLENGTH`, `SCI_SETUNDOCOLLECTION`, `SCI_EMPTYUNDOBUFFER`, `SCI_SETSAVEPOINT`, `SCI_BEGINUNDOACTION`, `SCI_ENDUNDOACTION`, `SCI_CREATEDOCUMENT`, `SCI_ADDREFDOCUMENT`, `SCI_RELEASEDOCUMENT`, `SCI_GETDOCPOINTER`, `SCI_SETDOCPOINTER`, `SCI_UNDO`, `SCI_REDO`, `SCI_CANUNDO`, `SCI_CANREDO`, `SCI_CUT`, `SCI_COPY`, `SCI_PASTE`, `SCI_GETSELECTIONSTART`, `SCI_GETSELECTIONEND`, `SCI_SETSEL`, `SCI_LINEFROMPOSITION`, `SCI_GETCOLUMN`, `SCI_POSITIONFROMLINE`, `SCI_GETLINEENDPOSITION`, `SCI_SETTARGETRANGE`, `SCI_SEARCHINTARGET`, `SCI_REPLACETARGET`, `SCI_SETSEARCHFLAGS`, `SCI_SETILEXER`, `SCI_STYLESETFORE`, `SCI_STYLESETBACK`, `SCI_STYLESETFONT`, `SCI_STYLESETSIZEFRACTIONAL`, `SCI_STYLECLEARALL`, `SCI_SETMARGINTYPEN`, `SCI_SETMARGINWIDTHN`, `SCI_SETWRAPMODE`, `SCI_SETTABWIDTH`, `SCI_GETMODIFY`, `SC_CP_UTF8`, `SCN_SAVEPOINTREACHED`, `SCN_SAVEPOINTLEFT`, `SCN_MODIFIED`, `SC_MOD_INSERTTEXT`, `SC_MOD_DELETETEXT`, `SC_WRAP_NONE`, `SC_WRAP_WORD`, every `SCE_JSON_*` name, and every `SCE_MARKDOWN_*` name. It writes sorted `pub const NAME: u32 = VALUE;` entries and fails when a required name is absent. Commit the generated file so Cargo builds do not require native sources.

Run: `pwsh -File tools/generate-scintilla-constants.ps1; cargo fmt --check`  
Expected: generated Rust parses and formatting is clean.

- [ ] **Step 5: Verify and commit native metadata, scripts, licenses, and generated API**

Run: `pwsh -File tools/fetch-native.ps1 -VerifyOnly; pwsh -File tools/build-native.ps1; cargo check --target x86_64-pc-windows-msvc`  
Expected: both DLLs are AMD64, versions match, and the Rust crate checks.

```bash
git add native/dependencies.json tools licenses src/editor/scintilla_constants.rs .gitignore
git commit -m "build: pin Scintilla and Lexilla artifacts"
```

---

### Task 4: Win32 ownership and allocation-free startup metrics

**Files:**
- Create: `src/platform/mod.rs`
- Create: `src/platform/handles.rs`
- Create: `src/platform/win32.rs`
- Create: `src/perf/mod.rs`
- Create: `src/perf/startup.rs`
- Modify: `src/lib.rs`
- Test: `src/platform/win32.rs`
- Test: `src/perf/startup.rs`

**Interfaces:**
- Consumes: `FastPadError::Win32`
- Produces: `wide_null`, `last_error`, `OwnedModule`, `OwnedHandle`, `StartupMetrics`, and `Milestone`

- [ ] **Step 1: Write pure UTF-16 and metrics tests**

```rust
#[test]
fn wide_null_appends_exactly_one_terminator() {
    assert_eq!(wide_null("FastPad"), vec![70, 97, 115, 116, 80, 97, 100, 0]);
}

#[test]
fn metrics_record_each_milestone_once() {
    let mut metrics = StartupMetrics::with_frequency(10_000, 100);
    metrics.record(Milestone::WindowCreated, 143);
    metrics.record(Milestone::WindowCreated, 999);
    assert_eq!(metrics.micros(Milestone::WindowCreated), Some(4_300));
}
```

- [ ] **Step 2: Run the focused red tests**

Run: `cargo test platform:: perf:: --lib`  
Expected: FAIL because the platform and performance types do not exist.

- [ ] **Step 3: Implement owned handles and fixed metric slots**

```rust
#[repr(u8)]
#[derive(Clone, Copy, Debug)]
pub enum Milestone {
    ProcessStart, WindowCreated, EditorCreated, FirstPaint,
    FirstInputAccepted, FirstInputRendered, SettingsLoaded,
    FileLoaded, FullyReady,
}

pub struct StartupMetrics {
    frequency: i64,
    start: i64,
    ticks: [i64; 9],
}

impl StartupMetrics {
    pub fn with_frequency(frequency: i64, start: i64) -> Self {
        Self { frequency, start, ticks: [0; 9] }
    }
    pub fn record(&mut self, milestone: Milestone, tick: i64) {
        let slot = &mut self.ticks[milestone as usize];
        if *slot == 0 { *slot = tick; }
    }
    pub fn micros(&self, milestone: Milestone) -> Option<u64> {
        let tick = self.ticks[milestone as usize];
        (tick != 0).then(|| ((tick - self.start) * 1_000_000 / self.frequency) as u64)
    }
}
```

Implement `OwnedModule(HMODULE)` with `Drop` calling `FreeLibrary`, `OwnedHandle(HANDLE)` with `Drop` calling `CloseHandle`, nonzero constructors, `as_raw`, and no `Clone`. Centralize unsafe calls and convert zero/NULL results through `GetLastError`.

- [ ] **Step 4: Verify host and Windows compilation**

Run: `cargo test --lib`  
Run on Windows: `cargo check --all-targets --target x86_64-pc-windows-msvc`  
Expected: tests pass on the host and Windows-only wrappers type-check.

- [ ] **Step 5: Commit**

```bash
git add src/platform src/perf src/lib.rs
git commit -m "feat: add Win32 ownership and startup metrics"
```

---

### Task 5: Typed Scintilla editor boundary

**Files:**
- Create: `src/editor/mod.rs`
- Create: `src/editor/scintilla.rs`
- Modify: `src/lib.rs`
- Create: `tests/windows/support/mod.rs`
- Create: `tests/windows/support/win32.rs`
- Test: `tests/windows/editor_control.rs`

**Interfaces:**
- Consumes: generated `scintilla_constants`, `OwnedModule`, HWND
- Produces: `Editor`, `EditorDocument`, `Editor::create`, text/edit/search/save-point/document APIs

- [ ] **Step 1: Write a Windows integration test for the native buffer**

```rust
#[cfg(windows)]
#[test]
fn editor_owns_text_and_switches_native_documents() {
    let harness = WindowHarness::new().unwrap();
    let editor = Editor::create(harness.hwnd()).unwrap();
    editor.set_text("first").unwrap();
    let first = editor.current_document().unwrap();
    let second = editor.create_document().unwrap();
    editor.use_document(&second).unwrap();
    editor.set_text("second").unwrap();
    editor.use_document(&first).unwrap();
    assert_eq!(editor.text().unwrap(), "first");
}
```

- [ ] **Step 2: Confirm the Windows test is red**

Run on Windows: `cargo test --test editor_control --target x86_64-pc-windows-msvc`  
Expected: FAIL because `Editor` and `WindowHarness` do not exist.

- [ ] **Step 3: Implement the editor wrapper and test harness**

```rust
pub type SciFnDirect = unsafe extern "C" fn(isize, u32, usize, isize) -> isize;

pub struct Editor { hwnd: HWND, direct_fn: SciFnDirect, direct_ptr: isize }

pub struct EditorDocument { raw: isize, editor: HWND }

impl Editor {
    pub fn create(parent: HWND) -> Result<Self>;
    pub fn hwnd(&self) -> HWND;
    pub fn set_text(&self, text: &str) -> Result<()>;
    pub fn text(&self) -> Result<String>;
    pub fn create_document(&self) -> Result<EditorDocument>;
    pub fn current_document(&self) -> Result<EditorDocument>;
    pub fn use_document(&self, document: &EditorDocument) -> Result<()>;
    pub fn set_save_point(&self);
    pub fn begin_undo_action(&self);
    pub fn end_undo_action(&self);
}
```

Acquire `SCI_GETDIRECTFUNCTION` and `SCI_GETDIRECTPOINTER` once after HWND creation. Use the direct function for synchronous editor calls. `EditorDocument::clone` sends `SCI_ADDREFDOCUMENT`; `Drop` sends `SCI_RELEASEDOCUMENT`. Set `SC_CP_UTF8` immediately. Test harness window creation lives under `tests/windows/support/mod.rs` and pumps messages before assertions.

`current_document` sends `SCI_ADDREFDOCUMENT` before returning an owned handle. Under `#[cfg(test)]`, `EditorDocument::test_fixture()` returns a zero raw handle whose clone/drop operations are inert; release builds contain no fake-handle branch.

- [ ] **Step 4: Verify native document lifetime and text**

Run on Windows: `cargo test --test editor_control --target x86_64-pc-windows-msvc -- --test-threads=1`  
Expected: test passes without access violations or leaked document references under Application Verifier.

- [ ] **Step 5: Commit**

```bash
git add src/editor src/lib.rs tests/windows
git commit -m "feat: wrap Scintilla editor and documents"
```

---

### Task 6: Minimal editable Win32 bootstrap

**Files:**
- Create: `src/bootstrap.rs`
- Create: `src/app.rs`
- Create: `src/window/mod.rs`
- Create: `src/window/main_window.rs`
- Create: `src/window/messages.rs`
- Modify: `src/lib.rs`
- Modify: `src/main.rs`
- Create: `tests/windows/support/process.rs`
- Test: `tests/windows/startup_smoke.rs`
- Create: `benchmarks/reference-machine.md`

**Interfaces:**
- Consumes: `LaunchOptions`, `Editor`, `StartupMetrics`, `OwnedModule`
- Produces: `bootstrap::run(LaunchOptions) -> Result<i32>`, main `WndProc`, deferred message constants

- [ ] **Step 1: Write the editable-window smoke test**

```rust
#[cfg(windows)]
#[test]
fn launch_creates_a_focused_editable_scintilla() {
    let process = FastPadProcess::spawn(["--diagnostic"]).unwrap();
    let hwnd = process.wait_for_main_window(Duration::from_secs(2)).unwrap();
    let editor = find_child_by_class(hwnd, "Scintilla").unwrap();
    assert_eq!(unsafe { GetFocus() }, editor);
    send_text(editor, "x");
    assert_eq!(scintilla_text(editor), "x");
    process.close().unwrap();
}
```

- [ ] **Step 2: Run and observe the missing-window failure**

Run on Windows: `cargo test --test startup_smoke --target x86_64-pc-windows-msvc -- --test-threads=1`  
Expected: FAIL because FastPad does not create a window.

- [ ] **Step 3: Implement only the startup allowlist**

`bootstrap::run` must perform this exact order: record counter, configure DPI, load absolute `Scintilla.dll` with `LoadLibraryExW`, register class, allocate `App`, create main HWND with `App` pointer in `lpParam`, create Scintilla child, `ShowWindow`, `SetFocus`, and run the message loop. The first completed `WM_PAINT` records `FirstPaint` and posts the ordered deferred messages; no deferred message is posted before that paint returns. Before each deferred handler starts, check `GetQueueStatus(QS_INPUT)` and repost that unit when keyboard or mouse input is waiting.

```rust
pub const WM_FASTPAD_LOAD_SETTINGS: u32 = WM_APP + 1;
pub const WM_FASTPAD_OPEN_REQUEST: u32 = WM_APP + 2;
pub const WM_FASTPAD_APPLY_LANGUAGE: u32 = WM_APP + 3;
pub const WM_FASTPAD_RECOVERY: u32 = WM_APP + 4;
pub const WM_FASTPAD_START_IPC: u32 = WM_APP + 5;
pub const WM_FASTPAD_BUILD_CHROME: u32 = WM_APP + 6;

pub struct App {
    pub hwnd: HWND,
    pub editor: Editor,
    pub launch: LaunchOptions,
    pub startup: StartupMetrics,
}
```

Store the `Box<App>` pointer in `GWLP_USERDATA` during `WM_NCCREATE`, recover it for later messages, and drop it exactly once in `WM_NCDESTROY`. `WM_SIZE` resizes the Scintilla child; `WM_SETFOCUS` focuses it; `WM_CLOSE` destroys the window; `WM_DESTROY` posts quit. Deferred messages initially do nothing except record `FullyReady`.

Implement `FastPadProcess::{spawn, wait_for_main_window, close}` in `tests/windows/support/process.rs`. Implement `find_child_by_class`, `send_text`, and `scintilla_text` in `tests/windows/support/win32.rs`; each function uses a bounded two-second wait and returns a test error instead of sleeping indefinitely.

In `main.rs`, map missing/incompatible Scintilla, window-class registration failure, and editor creation failure to distinct exit codes 10, 11, and 12 and show one minimal `MessageBoxW`. Once the HWND exists, route all nonfatal errors to the deferred notification model rather than terminating.

- [ ] **Step 4: Verify editable startup and pure regressions**

Run on Windows: `cargo test --target x86_64-pc-windows-msvc -- --test-threads=1`  
Expected: smoke test enters `x`, closes cleanly, and all pure tests pass.

- [ ] **Step 5: Record the first baseline and commit**

Run on the reference machine: `cargo run --release --bin fastpad -- --diagnostic` and manually confirm the first caret accepts input before deferred-ready output. Record machine identity in `benchmarks/reference-machine.md`.

```bash
git add src tests/windows/startup_smoke.rs benchmarks/reference-machine.md
git commit -m "feat: create minimal editable Win32 shell"
```

---

### Task 7: Startup benchmark harness

**Files:**
- Create: `src/perf/protocol.rs`
- Create: `src/bin/fastpad-bench.rs`
- Create: `tools/benchmark.ps1`
- Create: `benchmarks/README.md`
- Modify: `src/perf/mod.rs`
- Modify: `src/bootstrap.rs`
- Test: `src/perf/protocol.rs`

**Interfaces:**
- Consumes: `StartupMetrics`, diagnostic launch mode
- Produces: `BenchmarkRecord`, fixed diagnostic frame, JSONL distribution output

- [ ] **Step 1: Write diagnostic frame tests**

```rust
#[test]
fn metric_frame_has_fixed_versioned_layout() {
    let record = BenchmarkRecord::sample();
    let bytes = record.encode();
    assert_eq!(&bytes[..4], b"FPB1");
    assert_eq!(BenchmarkRecord::decode(&bytes).unwrap(), record);
}

#[test]
fn truncated_frame_is_rejected() {
    assert!(BenchmarkRecord::decode(b"FPB1").is_err());
}
```

- [ ] **Step 2: Run the red protocol test**

Run: `cargo test perf::protocol::tests --lib`  
Expected: FAIL because the protocol is absent.

- [ ] **Step 3: Implement benchmark-only signaling**

Define `BenchmarkRecord` with `u32 version`, `u32 pid`, nine `u64` microsecond fields, and `u64 idle_private_working_set_bytes`. `--diagnostic` creates a uniquely named inherited mapping/event supplied by `fastpad-bench`; normal startup creates neither. Record first accepted input when the benchmark `WM_CHAR` reaches Scintilla and first rendered input at the next completed Scintilla paint containing that modification. Signal only after writing the timestamp.

In diagnostic mode only, install a Scintilla child-window subclass with `SetWindowSubclass`. Record `FirstInputAccepted` before forwarding the injected `WM_CHAR`; mark the next text-changing notification; record `FirstInputRendered` after the following `WM_PAINT` returns. Remove the subclass during `WM_NCDESTROY`. Normal launches never install it.

The harness records its `QueryPerformanceCounter` immediately before `CreateProcessW`, waits for the main HWND, sends one private-use Unicode character, waits for the diagnostic event, verifies the character through Scintilla, waits two input-free seconds after `FullyReady`, samples private working set with `GetProcessMemoryInfo`, closes the process, and appends one JSON object.

- [ ] **Step 4: Add distribution calculation and thresholds**

```rust
fn percentile(sorted: &[u64], percentile: f64) -> u64 {
    let index = ((sorted.len() - 1) as f64 * percentile).ceil() as usize;
    sorted[index]
}
```

`fastpad-bench --runs 100 --warmup 10 --output benchmarks/latest.jsonl` prints p50 and p95 for each milestone. It exits nonzero with `--enforce-reference` if warm TTI p50 is at least 25,000 µs or p95 is at least 40,000 µs.

Add `fastpad-bench compare baseline.jsonl candidate.jsonl`. It reports a regression when candidate p95 increases by at least the larger of 2,000 µs or 10% and the bootstrap 95% confidence interval for the p95 delta excludes zero. Use 10,000 deterministic resamples seeded with `0xFA57_0A0D`.

- [ ] **Step 5: Verify the harness and commit**

Run on Windows: `cargo test perf::protocol::tests --lib; cargo run --release --bin fastpad-bench -- --runs 20 --warmup 5`  
Expected: 20 valid records, sorted percentile output, and no orphaned FastPad processes.

```bash
git add src/perf src/bin tools/benchmark.ps1 benchmarks
git commit -m "perf: add startup distribution harness"
```

---

### Task 8: Editor-first title shell and command menus

**Files:**
- Create: `src/window/titlebar.rs`
- Create: `src/window/tabs.rs`
- Create: `src/window/menus.rs`
- Create: `src/window/commands.rs`
- Create: `src/window/accessibility.rs`
- Modify: `src/window/main_window.rs`
- Modify: `src/app.rs`
- Test: `src/window/titlebar.rs`
- Test: `tests/windows/titlebar.rs`

**Interfaces:**
- Consumes: main `WndProc`, DWM/GDI wrappers, `Editor`
- Produces: `TitleBarLayout`, `HitTarget`, `CommandId`, menu-mode routing

- [ ] **Step 1: Write deterministic geometry tests before native painting**

```rust
#[test]
fn hit_test_preserves_drag_and_maximize_regions() {
    let layout = TitleBarLayout::calculate(Size::new(1200, 800), 144, 2);
    assert_eq!(layout.hit_test(layout.maximize.center()), HitTarget::Maximize);
    assert_eq!(layout.hit_test(layout.tab(0).center()), HitTarget::Tab(0));
    assert_eq!(layout.hit_test(layout.drag_region.center()), HitTarget::Caption);
}

#[test]
fn narrow_window_never_overlaps_caption_buttons() {
    let layout = TitleBarLayout::calculate(Size::new(320, 600), 96, 8);
    assert!(layout.tabs.right() <= layout.minimize.left());
}
```

- [ ] **Step 2: Run the red geometry tests**

Run: `cargo test window::titlebar::tests --lib`  
Expected: FAIL because layout types are absent.

- [ ] **Step 3: Implement pure layout, GDI paint, and DWM-first hit testing**

Define `HitTarget::{Client, Caption, Minimize, Maximize, Close, Tab(usize), CloseTab(usize), NewTab, Overflow}`. Calculate all geometry from `GetDpiForWindow` and system metrics. In `WM_NCHITTEST`, call `DwmDefWindowProc` first, return its handled result, then map FastPad targets to `HTCAPTION`, `HTMINBUTTON`, `HTMAXBUTTON`, `HTCLOSE`, or `HTCLIENT`.

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Point { pub x: i32, pub y: i32 }
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Size { pub width: i32, pub height: i32 }
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rect { pub left: i32, pub top: i32, pub right: i32, pub bottom: i32 }
pub struct TitleBarLayout {
    pub tabs: Rect,
    pub drag_region: Rect,
    pub minimize: Rect,
    pub maximize: Rect,
    pub close: Rect,
    tab_rects: Vec<Rect>,
}
impl TitleBarLayout {
    pub fn calculate(client: Size, dpi: u32, tab_count: usize) -> Self;
    pub fn hit_test(&self, point: Point) -> HitTarget;
    pub fn tab(&self, index: usize) -> Rect { self.tab_rects[index] }
}
```

Handle `WM_NCCALCSIZE` to reclaim the caption while preserving resize borders and `WM_GETMINMAXINFO` to respect the monitor work area. Paint one integrated strip with `BeginPaint`, `FillRect`, and `DrawTextW`; use system colors in high contrast. Do not add Mica, animation, image decoding, or child HWNDs.

Create the MSAA/UI Automation provider only when `WM_GETOBJECT` first requests it. Expose the title strip as a tab list, each tab as a selectable/closable tab, and New/Overflow/Minimize/Maximize/Close as named buttons. Keep its COM vtable and reference counting inside `window/accessibility.rs`; normal startup does not instantiate the provider.

- [ ] **Step 4: Implement one command model for shortcuts and menus**

```rust
#[repr(u16)]
pub enum CommandId {
    New = 100, Open, Save, SaveAs, CloseTab,
    Undo, Redo, Cut, Copy, Paste,
    Find, Replace, ValidateJson, FormatJson,
    LanguagePlainText, LanguageJson, LanguageMarkdown, Exit,
}
```

The overflow menu and transient Alt/F10 File/Edit/Search/View/Help menu use the same `CommandId` values. Create one accelerator table for Ctrl+N, Ctrl+O, Ctrl+S, Ctrl+Shift+S, Ctrl+F, Ctrl+H, Ctrl+Z, Ctrl+Y, and Ctrl+Shift+F. `WM_COMMAND` forwards to `App::execute(CommandId)`. Initially wire only Exit and focus-preserving no-op handlers for commands completed by later tasks.

- [ ] **Step 5: Verify native behavior and performance**

Run on Windows: `cargo test window::titlebar::tests --lib; cargo test --test titlebar -- --test-threads=1`  
The Windows test also calls `AccessibleObjectFromWindow` and asserts the tab list and five named buttons. Manually verify snap-layout hover, drag, double-click maximize, system menu, 100/150/200% DPI, keyboard menu mode, Narrator focus, and Windows high contrast.  
Run: `cargo run --release --bin fastpad-bench -- --runs 100 --warmup 10 --output benchmarks/titlebar.jsonl; cargo run --release --bin fastpad-bench -- compare benchmarks/baseline.jsonl benchmarks/titlebar.jsonl`  
Expected: comparison does not meet the defined 2 ms/10% p95 regression threshold and the absolute TTI budget passes on reference hardware.

- [ ] **Step 6: Commit**

```bash
git add src/window src/app.rs tests/windows/titlebar.rs benchmarks
git commit -m "feat: add editor-first native title shell"
```

---

### Task 9: Native document-backed tabs

**Files:**
- Create: `src/document.rs`
- Modify: `src/app.rs`
- Modify: `src/window/tabs.rs`
- Modify: `src/window/commands.rs`
- Test: `src/document.rs`
- Test: `tests/windows/tabs.rs`

**Interfaces:**
- Consumes: `EditorDocument`, `Encoding`, `CommandId::New/CloseTab`
- Produces: `DocumentId`, `RecoveryId`, `Language`, `Document`, `Tabs`

- [ ] **Step 1: Write tab-state tests with a fake native handle**

```rust
#[test]
fn closing_the_last_tab_replaces_it_with_a_new_document() {
    fn document(id: u64) -> Document { Document::test_fixture(DocumentId(id), false) }
    let mut tabs = Tabs::with_document(document(1));
    let closed = tabs.close_active(CloseDecision::Discard, || document(2)).unwrap();
    assert_eq!(closed.id, DocumentId(1));
    assert_eq!(tabs.len(), 1);
    assert_eq!(tabs.active().id, DocumentId(2));
}

#[test]
fn cancel_preserves_dirty_tab_and_order() {
    fn document(id: u64) -> Document { Document::test_fixture(DocumentId(id), false) }
    fn dirty_document(id: u64) -> Document { Document::test_fixture(DocumentId(id), true) }
    let mut tabs = Tabs::from_documents([dirty_document(1), document(2)]);
    tabs.activate(DocumentId(1)).unwrap();
    assert_eq!(tabs.close_active(CloseDecision::Cancel, || document(3)), Err(CloseCancelled));
    assert_eq!(tabs.ids().collect::<Vec<_>>(), [DocumentId(1), DocumentId(2)]);
}
```

- [ ] **Step 2: Run red tab tests**

Run: `cargo test document::tests --lib`  
Expected: FAIL because metadata and tab collection are absent.

- [ ] **Step 3: Implement metadata without text storage**

```rust
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DocumentId(pub u64);
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RecoveryId(pub u128);
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Language { PlainText, Json, Markdown }
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CloseDecision { Save, Discard, Cancel }
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CloseCancelled;

pub struct Document {
    pub id: DocumentId,
    pub handle: EditorDocument,
    pub path: Option<PathBuf>,
    pub language: Language,
    pub encoding: Encoding,
    pub dirty: bool,
    pub recovery_id: RecoveryId,
    pub generation: u64,
}
```

`Tabs` owns `Vec<Document>` plus active index. It rejects duplicate canonical paths, switches the editor through `Editor::use_document`, and never exposes mutable handle ownership separately from metadata.

Under `#[cfg(test)]`, add `Document::test_fixture(DocumentId, dirty)` backed by `EditorDocument::test_fixture()` from Task 5; it must not compile into the Windows release binary.

After window-close review succeeds, call `Tabs::clear_for_shutdown()` while the Scintilla HWND is still valid so every owned document reference releases before `DestroyWindow`. The Windows test runs this path under Application Verifier and asserts no invalid message or leaked document handle.

- [ ] **Step 4: Wire New, tab clicks, save-point notifications, and close review**

Handle `SCN_SAVEPOINTLEFT` and `SCN_SAVEPOINTREACHED` from `WM_NOTIFY` to update dirty state and invalidate the title strip. Ctrl+N and plus create a native document. Closing a dirty tab prompts Save/Discard/Cancel; window close reviews dirty tabs in order and Cancel aborts shutdown.

- [ ] **Step 5: Verify tabs and commit**

Run on Windows: `cargo test document::tests --lib; cargo test --test tabs -- --test-threads=1`  
Expected: switching tabs preserves separate native text, closing releases exactly one reference, and dirty indicators follow save points.

```bash
git add src/document.rs src/app.rs src/window tests/windows/tabs.rs
git commit -m "feat: add Scintilla-backed document tabs"
```

---

### Task 10: Deferred open and file dialogs

**Files:**
- Create: `src/file/loader.rs`
- Create: `src/platform/dialogs.rs`
- Modify: `src/file/mod.rs`
- Modify: `src/app.rs`
- Modify: `src/window/commands.rs`
- Modify: `src/bootstrap.rs`
- Test: `src/file/loader.rs`
- Test: `tests/windows/open_file.rs`

**Interfaces:**
- Consumes: `decode`, `Tabs`, `WM_FASTPAD_OPEN_REQUEST`, `CommandId::Open`
- Produces: `LoadedFile`, `load(Path)`, `show_open_dialog(HWND)`

- [ ] **Step 1: Write loader tests against temporary files**

```rust
#[test]
fn load_preserves_path_encoding_and_text() {
    let file = TempFixture::new("config.json", b"\xEF\xBB\xBF{\"ok\":true}");
    let loaded = load(file.path()).unwrap();
    assert_eq!(loaded.text, "{\"ok\":true}");
    assert_eq!(loaded.encoding, Encoding::Utf8Bom);
    assert_eq!(loaded.path, file.path());
}

#[test]
fn invalid_encoding_does_not_return_partial_text() {
    let file = TempFixture::new("bad.txt", &[0x80]);
    assert!(matches!(load(file.path()), Err(FastPadError::UnsupportedEncoding)));
}
```

Define the test-local `TempFixture::{new, path}` in the same test module. It creates a unique directory below `std::env::temp_dir()` using the PID plus an atomic counter, writes the requested bytes, and removes only its owned directory from `Drop`.

- [ ] **Step 2: Run the red loader test**

Run: `cargo test file::loader::tests --lib`  
Expected: FAIL because `LoadedFile` and `load` are absent.

- [ ] **Step 3: Implement buffered load and deferred population**

```rust
pub struct LoadedFile { pub path: PathBuf, pub text: String, pub encoding: Encoding }

pub fn load(path: &Path) -> Result<LoadedFile> {
    let bytes = std::fs::read(path)?;
    let decoded = decode(&bytes)?;
    Ok(LoadedFile { path: path.to_path_buf(), text: decoded.text, encoding: decoded.encoding })
}
```

`App::open_path` creates or reuses an empty clean tab, disables undo collection, sets text, empties undo history, reenables undo, sets the save point, updates metadata, records `FileLoaded`, and posts language activation. On error, leave the prior tab active and unchanged.

- [ ] **Step 4: Implement the native open dialog after startup**

Use `IFileOpenDialog` with `FOS_FORCEFILESYSTEM | FOS_FILEMUSTEXIST`; initialize COM only inside the command. Convert the selected `IShellItem` path to `PathBuf`, release all COM interfaces, and treat dialog cancellation as `Ok(None)`.

- [ ] **Step 5: Verify launch-order and open behavior**

Run on Windows: `cargo test file::loader::tests --lib; cargo test --test open_file -- --test-threads=1`  
The integration test launches with a JSON fixture and asserts `FirstInputAccepted < FileLoaded`, correct text, path, encoding, clean state, and no duplicate tab when opening the same canonical path.

- [ ] **Step 6: Commit**

```bash
git add src/file src/platform/dialogs.rs src/app.rs src/window/commands.rs src/bootstrap.rs tests/windows/open_file.rs
git commit -m "feat: defer file opening until after input"
```

---

### Task 11: Atomic save, Save As, and failure safety

**Files:**
- Create: `src/file/saver.rs`
- Modify: `src/platform/dialogs.rs`
- Modify: `src/file/mod.rs`
- Modify: `src/app.rs`
- Modify: `src/window/commands.rs`
- Test: `src/file/saver.rs`
- Test: `tests/windows/save_file.rs`

**Interfaces:**
- Consumes: `encode`, `Document.encoding`, `Editor::text`, Save/SaveAs commands
- Produces: `save_atomic(Path, &[u8])`, `show_save_dialog(HWND, suggested_name)`

- [ ] **Step 1: Write atomic-save tests**

```rust
#[test]
fn replacing_existing_file_never_exposes_partial_content() {
    let fixture = ExistingFile::new(b"old");
    save_atomic(fixture.path(), b"new content").unwrap();
    assert_eq!(std::fs::read(fixture.path()).unwrap(), b"new content");
    assert!(fixture.sibling_temp_files().is_empty());
}

#[test]
fn failed_replace_preserves_original() {
    let fixture = ExistingFile::new(b"original");
    fixture.deny_replacement();
    assert!(save_atomic(fixture.path(), b"replacement").is_err());
    assert_eq!(std::fs::read(fixture.path()).unwrap(), b"original");
}
```

Define the test-local `ExistingFile::{new, path, sibling_temp_files, deny_replacement}` in the same test module. It owns a unique temporary directory. On Windows, `deny_replacement` retains an open destination handle without `FILE_SHARE_DELETE`; `Drop` closes that handle before removing only its owned directory.

- [ ] **Step 2: Run the focused red test**

Run: `cargo test file::saver::tests --lib`  
Expected: FAIL because `save_atomic` does not exist.

- [ ] **Step 3: Implement same-directory atomic replacement**

Generate a collision-resistant sibling name from PID plus an incrementing counter, open with `create_new`, write all bytes, call `sync_all`, close, then use `ReplaceFileW` for an existing destination or `MoveFileExW(..., MOVEFILE_WRITE_THROUGH)` for a new destination. On every error, attempt to delete only the explicit sibling temp path and return the original error.

```rust
pub fn save_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let temp = next_sibling_temp(path)?;
    let result = (|| {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        replace_or_move(&temp, path)
    })();
    if result.is_err() { let _ = std::fs::remove_file(&temp); }
    result
}
```

- [ ] **Step 4: Wire Save and Save As**

Read current editor text transiently, call `encode(text, document.encoding)`, save, update canonical path and language, and send `SCI_SETSAVEPOINT` only on success. Save As uses `IFileSaveDialog`; cancellation is not an error. Failed saves keep the dirty indicator and display a persistent notification.

- [ ] **Step 5: Verify all encodings and failure invariants**

Run on Windows: `cargo test file::saver::tests --lib; cargo test --test save_file -- --test-threads=1`  
Expected: each supported encoding round-trips, failed replacement retains the original bytes, and dirty state clears only after success.

- [ ] **Step 6: Commit**

```bash
git add src/file src/platform/dialogs.rs src/app.rs src/window/commands.rs tests/windows/save_file.rs
git commit -m "feat: add atomic save and Save As"
```

---

### Task 12: Editing commands and find/replace bar

**Files:**
- Create: `src/window/find_bar.rs`
- Modify: `src/editor/scintilla.rs`
- Modify: `src/window/commands.rs`
- Modify: `src/window/main_window.rs`
- Test: `src/window/find_bar.rs`
- Test: `tests/windows/editing.rs`

**Interfaces:**
- Consumes: `Editor`, `CommandId::{Undo,Redo,Cut,Copy,Paste,Find,Replace}`
- Produces: `SearchState`, typed edit/search operations, replace-all undo grouping

- [ ] **Step 1: Write search progression tests**

```rust
#[test]
fn next_match_wraps_once_then_stops() {
    let mut state = SearchState::new("one", SearchDirection::Forward, 8);
    assert_eq!(state.next_range("one two one"), Some(8..11));
    assert_eq!(state.next_range("one two one"), Some(0..3));
    assert_eq!(state.next_range("one two one"), None);
}
```

- [ ] **Step 2: Run the red search test**

Run: `cargo test window::find_bar::tests --lib`  
Expected: FAIL because `SearchState` is absent.

- [ ] **Step 3: Add typed Scintilla editing and search APIs**

Add `undo`, `redo`, `cut`, `copy`, `paste`, `can_undo`, `can_redo`, `selection`, `set_selection`, `find`, `replace_target`, and `replace_all`. Use `SCI_SETTARGETRANGE` and `SCI_SEARCHINTARGET`; never retrieve the complete document for ordinary search. Wrap Replace All in exactly one begin/end undo pair.

```rust
pub struct SearchMatch { pub start: isize, pub end: isize }
impl Editor {
    pub fn find(&self, query: &str, range: Range<isize>, flags: u32) -> Result<Option<SearchMatch>>;
    pub fn replace_target(&self, range: Range<isize>, replacement: &str) -> Result<Range<isize>>;
    pub fn replace_all(&self, query: &str, replacement: &str, flags: u32) -> Result<usize>;
}
```

- [ ] **Step 4: Build the in-window bar and shortcuts**

Ctrl+F opens Find, Ctrl+H opens Replace, Enter/Shift+Enter navigate, Escape closes and returns focus to Scintilla. Ctrl+Z/Y and clipboard accelerators dispatch the shared command model. Preserve the editor selection when opening the bar and prefill the query from a single-line selection.

- [ ] **Step 5: Verify editing behavior and commit**

Run on Windows: `cargo test window::find_bar::tests --lib; cargo test --test editing -- --test-threads=1`  
Expected: wrapping occurs once, replace-all is undone once, and focus returns to the editor.

```bash
git add src/editor/scintilla.rs src/window tests/windows/editing.rs
git commit -m "feat: add editing and find replace commands"
```

---

### Task 13: Deferred Lexilla and language highlighting

**Files:**
- Create: `src/languages/mod.rs`
- Create: `src/languages/lexilla.rs`
- Create: `src/languages/json.rs`
- Create: `src/languages/markdown.rs`
- Modify: `src/app.rs`
- Modify: `src/editor/scintilla.rs`
- Test: `src/languages/mod.rs`
- Test: `tests/windows/highlighting.rs`

**Interfaces:**
- Consumes: `Language`, `WM_FASTPAD_APPLY_LANGUAGE`, absolute application directory
- Produces: `detect_language`, `LanguageManager`, deferred `CreateLexer` and styling

- [ ] **Step 1: Write extension detection tests**

```rust
#[test]
fn detects_supported_languages_case_insensitively() {
    assert_eq!(detect_language(Path::new("CONFIG.JSON")), Language::Json);
    assert_eq!(detect_language(Path::new("readme.md")), Language::Markdown);
    assert_eq!(detect_language(Path::new("notes.txt")), Language::PlainText);
}
```

- [ ] **Step 2: Run the red language test**

Run: `cargo test languages::tests --lib`  
Expected: FAIL because language detection is absent.

- [ ] **Step 3: Implement deferred Lexilla loading**

```rust
pub struct LanguageManager { lexilla: Option<LexillaLibrary> }

impl LanguageManager {
    pub fn apply(&mut self, editor: &Editor, language: Language) -> Result<()>;
}
```

For plain text send a null lexer. For JSON or Markdown, load absolute `Lexilla.dll` on first use with safe flags, resolve `CreateLexer` via `GetProcAddress`, request `json` or `markdown`, and pass the returned `ILexer5*` through `SCI_SETILEXER`. If loading fails, keep plain text and return an error for the notification bar.

Wire `LanguagePlainText`, `LanguageJson`, and `LanguageMarkdown` into the View menu. Explicit selection updates document metadata and applies the lexer without changing the path or parsing JSON.

- [ ] **Step 4: Apply deterministic light/dark style tables**

Define static style records for default text, comments, strings, numbers, keywords, headings, emphasis, code, and links. Send only styles relevant to each lexer. Do not enumerate fonts; use the configured face or compiled `Consolas` fallback.

```rust
pub struct LexerStyle { pub style: u32, pub foreground: u32, pub background: u32, pub bold: bool }
pub struct LanguageStyles { pub light: &'static [LexerStyle], pub dark: &'static [LexerStyle] }
const fn rgb(r: u32, g: u32, b: u32) -> u32 { r | (g << 8) | (b << 16) }
const JSON_LIGHT: &[LexerStyle] = &[
    LexerStyle { style: SCE_JSON_DEFAULT, foreground: rgb(32, 32, 32), background: rgb(255, 255, 255), bold: false },
    LexerStyle { style: SCE_JSON_STRING, foreground: rgb(163, 21, 21), background: rgb(255, 255, 255), bold: false },
    LexerStyle { style: SCE_JSON_NUMBER, foreground: rgb(9, 134, 88), background: rgb(255, 255, 255), bold: false },
];
const MARKDOWN_LIGHT: &[LexerStyle] = &[
    LexerStyle { style: SCE_MARKDOWN_DEFAULT, foreground: rgb(32, 32, 32), background: rgb(255, 255, 255), bold: false },
    LexerStyle { style: SCE_MARKDOWN_HEADER1, foreground: rgb(0, 92, 197), background: rgb(255, 255, 255), bold: true },
    LexerStyle { style: SCE_MARKDOWN_CODE, foreground: rgb(110, 65, 15), background: rgb(246, 248, 250), bold: false },
];
const JSON_DARK: &[LexerStyle] = &[
    LexerStyle { style: SCE_JSON_DEFAULT, foreground: rgb(220, 220, 220), background: rgb(30, 30, 30), bold: false },
    LexerStyle { style: SCE_JSON_STRING, foreground: rgb(206, 145, 120), background: rgb(30, 30, 30), bold: false },
    LexerStyle { style: SCE_JSON_NUMBER, foreground: rgb(181, 206, 168), background: rgb(30, 30, 30), bold: false },
];
const MARKDOWN_DARK: &[LexerStyle] = &[
    LexerStyle { style: SCE_MARKDOWN_DEFAULT, foreground: rgb(220, 220, 220), background: rgb(30, 30, 30), bold: false },
    LexerStyle { style: SCE_MARKDOWN_HEADER1, foreground: rgb(86, 156, 214), background: rgb(30, 30, 30), bold: true },
    LexerStyle { style: SCE_MARKDOWN_CODE, foreground: rgb(215, 186, 125), background: rgb(45, 45, 45), bold: false },
];
pub static JSON_STYLES: LanguageStyles = LanguageStyles { light: JSON_LIGHT, dark: JSON_DARK };
pub static MARKDOWN_STYLES: LanguageStyles = LanguageStyles { light: MARKDOWN_LIGHT, dark: MARKDOWN_DARK };
```

- [ ] **Step 5: Verify deferred loading and highlighting**

Run on Windows: `cargo test languages::tests --lib; cargo test --test highlighting -- --test-threads=1`  
The integration test asserts Lexilla is not loaded after an empty launch, becomes loaded after JSON/Markdown activation, and missing Lexilla leaves editable plain text.

- [ ] **Step 6: Commit**

```bash
git add src/languages src/app.rs src/editor/scintilla.rs tests/windows/highlighting.rs
git commit -m "feat: add deferred JSON and Markdown lexers"
```

---

### Task 14: Explicit JSON validation and formatting

**Files:**
- Create: `src/languages/json_commands.rs`
- Modify: `src/languages/mod.rs`
- Modify: `src/app.rs`
- Modify: `src/window/commands.rs`
- Modify: `src/editor/scintilla.rs`
- Test: `src/languages/json_commands.rs`
- Test: `tests/windows/json_commands.rs`

**Interfaces:**
- Consumes: `serde_json`, editor text/undo/selection methods
- Produces: `validate_json`, `format_json`, `JsonIssue`

- [ ] **Step 1: Write formatting and failure tests**

```rust
#[test]
fn format_uses_two_spaces_and_preserves_final_newline() {
    assert_eq!(format_json("{\"a\":[1,2]}\n").unwrap(), "{\n  \"a\": [\n    1,\n    2\n  ]\n}\n");
    assert!(!format_json("{\"a\":1}").unwrap().ends_with('\n'));
}

#[test]
fn validation_returns_line_and_column() {
    let issue = validate_json("{\n  bad\n}").unwrap_err();
    assert_eq!((issue.line, issue.column), (2, 3));
}
```

- [ ] **Step 2: Run the red JSON tests**

Run: `cargo test languages::json_commands::tests --lib`  
Expected: FAIL because command functions do not exist.

- [ ] **Step 3: Implement pure JSON commands**

Parse into `serde_json::Value`. Serialize with `serde_json::to_writer_pretty`, whose default indent is two spaces and whose output has no trailing newline. Append exactly one `\n` only when the source ended in `\n` or `\r\n`. Convert `serde_json::Error::line/column` into one-based `JsonIssue`.

```rust
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JsonIssue { pub line: usize, pub column: usize, pub message: String }
impl JsonIssue {
    fn invariant() -> Self { Self { line: 0, column: 0, message: "JSON output was not UTF-8".into() } }
}
impl From<serde_json::Error> for JsonIssue {
    fn from(error: serde_json::Error) -> Self {
        Self { line: error.line(), column: error.column(), message: error.to_string() }
    }
}

pub fn validate_json(source: &str) -> core::result::Result<(), JsonIssue> {
    serde_json::from_str::<serde_json::Value>(source)
        .map(|_| ())
        .map_err(JsonIssue::from)
}

pub fn format_json(source: &str) -> core::result::Result<String, JsonIssue> {
    let value: serde_json::Value = serde_json::from_str(source).map_err(JsonIssue::from)?;
    let mut bytes = Vec::new();
    serde_json::to_writer_pretty(&mut bytes, &value).map_err(JsonIssue::from)?;
    let mut output = String::from_utf8(bytes).map_err(|_| JsonIssue::invariant())?;
    if source.ends_with('\n') { output.push('\n'); }
    Ok(output)
}
```

- [ ] **Step 4: Wire commands without partial mutation**

Validate reads the current text and displays success or a line/column action. Format captures selection as logical start/end line-column pairs, formats completely before mutation, performs one full replacement inside one undo group, and restores clamped positions. Invalid JSON never starts an undo action and leaves bytes unchanged.

- [ ] **Step 5: Verify command semantics and commit**

Run on Windows: `cargo test languages::json_commands::tests --lib; cargo test --test json_commands -- --test-threads=1`  
Expected: two-space output, one Undo restores exact original bytes, invalid JSON is unchanged, and Ctrl+Shift+F triggers Format JSON.

```bash
git add src/languages src/app.rs src/window/commands.rs src/editor/scintilla.rs tests/windows/json_commands.rs
git commit -m "feat: add explicit JSON commands"
```

---

### Task 15: Deferred settings, theme, status, and notifications

**Files:**
- Create: `src/config/mod.rs`
- Create: `src/config/defaults.rs`
- Create: `src/config/persisted.rs`
- Create: `src/platform/theme.rs`
- Create: `src/window/status.rs`
- Create: `src/window/notification.rs`
- Modify: `src/app.rs`
- Modify: `src/window/main_window.rs`
- Test: `src/config/persisted.rs`

**Interfaces:**
- Consumes: `WM_FASTPAD_LOAD_SETTINGS`, `WM_FASTPAD_BUILD_CHROME`, editor style APIs
- Produces: `Settings`, `SettingsDelta`, `Theme`, status and persistent notification models

- [ ] **Step 1: Write tolerant settings tests**

```rust
#[test]
fn bad_key_does_not_discard_valid_keys() {
    let delta = parse("font_size=13\ntab_width=nope\ntheme=dark\nunknown=x\n");
    assert_eq!(delta.font_size, Some(13));
    assert_eq!(delta.tab_width, None);
    assert_eq!(delta.theme, Some(ThemePreference::Dark));
    assert_eq!(delta.warnings.len(), 2);
}
```

- [ ] **Step 2: Run the red settings test**

Run: `cargo test config::persisted::tests --lib`  
Expected: FAIL because parser and settings types are absent.

- [ ] **Step 3: Implement compiled defaults and a hand-written delta parser**

Defaults: Consolas 11 pt, tab width 4, word wrap off, system theme, recovery interval 30 seconds. Recognize exactly `font_face`, `font_size`, `tab_width`, `word_wrap`, `theme`, and `recovery_interval_seconds`. Trim ASCII whitespace, ignore blank and `#` comment lines, retain one warning per invalid/unknown line, and apply valid keys independently.

```rust
pub struct Settings {
    pub font_face: String,
    pub font_size: u16,
    pub tab_width: u8,
    pub word_wrap: bool,
    pub theme: ThemePreference,
    pub recovery_interval_seconds: u32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThemePreference { System, Light, Dark }
pub struct SettingWarning { pub line: usize, pub message: String }
#[derive(Default)]
pub struct SettingsDelta {
    pub font_face: Option<String>, pub font_size: Option<u16>,
    pub tab_width: Option<u8>, pub word_wrap: Option<bool>,
    pub theme: Option<ThemePreference>, pub recovery_interval_seconds: Option<u32>,
    pub warnings: Vec<SettingWarning>,
}
pub fn parse(source: &str) -> SettingsDelta;
```

- [ ] **Step 4: Apply settings and build deferred chrome**

Resolve `%LocalAppData%\FastPad\fastpad.ini` only in `WM_FASTPAD_LOAD_SETTINGS`. Apply changed editor properties without replacing text or selection. In `WM_FASTPAD_BUILD_CHROME`, create status and notification models and repaint; no status child HWND is necessary. Query system light/dark and high contrast only here and later on `WM_SETTINGCHANGE`, `WM_THEMECHANGED`, and `WM_DWMCOLORIZATIONCOLORCHANGED`.

- [ ] **Step 5: Verify corrupt settings and startup ordering**

Run on Windows: `cargo test config::persisted::tests --lib; cargo test --test startup_smoke -- --test-threads=1`  
Add a smoke fixture with corrupt settings and assert the editor accepts the benchmark character before `SettingsLoaded`, then uses valid keys and reports invalid ones non-modally.

- [ ] **Step 6: Commit**

```bash
git add src/config src/platform/theme.rs src/window src/app.rs tests/windows/startup_smoke.rs
git commit -m "feat: defer settings theme and editor chrome"
```

---

### Task 16: Idle crash recovery

**Files:**
- Create: `src/recovery/mod.rs`
- Create: `src/recovery/snapshot.rs`
- Modify: `src/app.rs`
- Modify: `src/window/main_window.rs`
- Modify: `src/document.rs`
- Test: `src/recovery/snapshot.rs`
- Test: `tests/windows/recovery.rs`

**Interfaces:**
- Consumes: document generations, editor text, atomic saver, `%LocalAppData%`
- Produces: `SnapshotHeader`, `write_snapshot`, `discover_snapshots`, idle scheduling

- [ ] **Step 1: Write versioned snapshot tests**

```rust
#[test]
fn snapshot_round_trip_preserves_metadata_and_text() {
    let snapshot = Snapshot::new(RecoveryId::from_u128(7), Some(PathBuf::from("a.txt")), Encoding::Utf16Le, "unsaved");
    let bytes = snapshot.encode().unwrap();
    assert_eq!(Snapshot::decode(&bytes).unwrap(), snapshot);
}

#[test]
fn corrupt_snapshot_is_rejected_without_panicking() {
    assert!(Snapshot::decode(b"FPS1\xFF").is_err());
}
```

- [ ] **Step 2: Run the red recovery tests**

Run: `cargo test recovery::snapshot::tests --lib`  
Expected: FAIL because the snapshot format is absent.

- [ ] **Step 3: Implement bounded, versioned snapshot encoding**

Use magic `FPS1`, format version 1, 128-bit recovery ID, encoding byte, optional UTF-16LE path length/path, and UTF-8 text length/text. Reject odd path lengths, lengths larger than the remaining bytes, and files larger than `isize::MAX`. Construct recovery IDs from the process-start counter in the high 64 bits and PID plus an atomic document counter in the low 64 bits; add no UUID crate. Use the existing same-directory atomic replacement for writes.

```rust
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Snapshot {
    pub recovery_id: RecoveryId,
    pub original_path: Option<PathBuf>,
    pub encoding: Encoding,
    pub text: String,
}
impl Snapshot {
    pub fn new(recovery_id: RecoveryId, original_path: Option<PathBuf>, encoding: Encoding, text: impl Into<String>) -> Self {
        Self { recovery_id, original_path, encoding, text: text.into() }
    }
    pub fn encode(&self) -> Result<Vec<u8>>;
    pub fn decode(bytes: &[u8]) -> Result<Self>;
}
pub struct SnapshotCandidate { pub path: PathBuf, pub snapshot: Snapshot }
pub fn write_snapshot(root: &Path, snapshot: &Snapshot) -> Result<PathBuf>;
pub fn discover_snapshots(root: &Path) -> Result<Vec<SnapshotCandidate>>;
```

- [ ] **Step 4: Add idle generation scheduling and deferred discovery**

Start `SetTimer` only after settings. Each modification increments the active document generation. After two input-idle seconds, snapshot at most one dirty document whose generation differs from its recorded recovery generation. Record duration. `WM_FASTPAD_RECOVERY` scans after first input, quarantines malformed files with a `.invalid` suffix, and opens valid snapshots as `Recovered: <name>` tabs without overwriting originals.

- [ ] **Step 5: Verify restore, cleanup, and timing**

Run on Windows: `cargo test recovery::snapshot::tests --lib; cargo test --test recovery -- --test-threads=1`  
Expected: restore happens after first input, saved/discarded recovered tabs remove their snapshot, malformed data survives as `.invalid`, and no timer exists before deferred setup.

- [ ] **Step 6: Commit**

```bash
git add src/recovery src/app.rs src/window/main_window.rs src/document.rs tests/windows/recovery.rs
git commit -m "feat: add deferred crash recovery"
```

---

### Task 17: Existing-instance reuse and overlapped pipe IPC

**Files:**
- Create: `src/ipc/mod.rs`
- Create: `src/ipc/protocol.rs`
- Create: `src/ipc/server.rs`
- Modify: `src/bootstrap.rs`
- Modify: `src/app.rs`
- Modify: `src/window/main_window.rs`
- Test: `src/ipc/protocol.rs`
- Test: `tests/windows/single_instance.rs`

**Interfaces:**
- Consumes: `LaunchOptions`, `LaunchRequest`, deferred `WM_FASTPAD_START_IPC`
- Produces: `IpcRequest`, 64 KiB bounded frames, `IpcServer::event`, `IpcServer::poll`

- [ ] **Step 1: Write strict frame tests**

```rust
#[test]
fn request_round_trips_utf16_path() {
    let request = IpcRequest::Open(PathBuf::from(r"C:\notes\zăpadă.md"));
    assert_eq!(decode_frame(&encode_frame(&request).unwrap()).unwrap(), request);
}

#[test]
fn oversized_and_unknown_frames_are_rejected() {
    assert!(decode_frame(&vec![0; 65_537]).is_err());
    assert!(decode_frame(b"FPI1\xFF\0\0\0").is_err());
}
```

- [ ] **Step 2: Run the red protocol tests**

Run: `cargo test ipc::protocol::tests --lib`  
Expected: FAIL because IPC types are absent.

- [ ] **Step 3: Implement versioned frames and per-session names**

Use magic `FPI1`, one-byte command (`1=Open`, `2=New`, `3=Activate`), little-endian payload length, and UTF-16LE path payload. Reject frames above 64 KiB, odd path byte lengths, embedded NULs, and unknown commands. Use the `Local\FastPad-<session-id>` mutex and `\\.\pipe\FastPad-<session-id>` pipe with an ACL limited to the current interactive user.

```rust
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IpcRequest { Open(PathBuf), New, Activate }
pub fn encode_frame(request: &IpcRequest) -> Result<Vec<u8>>;
pub fn decode_frame(bytes: &[u8]) -> Result<IpcRequest>;

pub struct InstanceNames { pub mutex: Vec<u16>, pub pipe: Vec<u16> }
pub struct CurrentUserAcl { descriptor: Vec<u8> }
impl InstanceNames { pub fn for_current_session() -> Result<Self>; }
impl CurrentUserAcl { pub fn current() -> Result<Self>; }

pub struct IpcServer {
    pipe: OwnedHandle,
    event: OwnedHandle,
    read: Pin<Box<OVERLAPPED>>,
    buffer: Box<[u8; 65_536]>,
}
impl IpcServer {
    pub fn bind(names: &InstanceNames, security: &CurrentUserAcl) -> Result<Self>;
    pub fn event(&self) -> HANDLE;
    pub fn poll(&mut self) -> Result<Vec<IpcRequest>>;
}
```

- [ ] **Step 4: Implement primary/secondary behavior**

Before window creation, unless `--new-window`, create/check the mutex. If already present, use bounded `WaitNamedPipeW` retries totaling at most 500 ms, send the request, and exit. A new primary retains the mutex but posts pipe-server creation only after first input.

Create the pipe with `FILE_FLAG_OVERLAPPED`. Expose its event handle and replace the plain loop with `MsgWaitForMultipleObjectsEx`. When signaled, consume complete frames, reconnect the listening instance, and post `WM_FASTPAD_IPC_REQUEST`; never execute file work inside pipe parsing.

If deferred `IpcServer::bind` fails, release the instance mutex, keep the editor operational, and show one persistent notification; later launches then create independent processes instead of waiting on a nonexistent pipe.

- [ ] **Step 5: Verify reuse, race, and override**

Run on Windows: `cargo test ipc::protocol::tests --lib; cargo test --test single_instance -- --test-threads=1`  
Expected: a second launch opens one tab in the primary, Activate foregrounds it, an early secondary survives the server-start race, malformed frames do nothing, and `--new-window` creates a distinct process.

- [ ] **Step 6: Commit**

```bash
git add src/ipc src/bootstrap.rs src/app.rs src/window/main_window.rs tests/windows/single_instance.rs
git commit -m "feat: add single-instance named-pipe IPC"
```

---

### Task 18: Hardening, CI, packaging, and release gates

**Files:**
- Create: `tools/package.ps1`
- Create: `tools/audit-dependencies.ps1`
- Create: `tools/verify-package.ps1`
- Create: `.github/workflows/ci.yml`
- Create: `tests/windows/acceptance.rs`
- Create: `tests/windows/support/acceptance.rs`
- Create: `LICENSES.md`
- Create: `benchmarks/release-profile.md`
- Modify: `Cargo.toml`
- Create: `README.md`

**Interfaces:**
- Consumes: complete application, native artifacts, benchmark harness
- Produces: signed-ready portable ZIP and final acceptance evidence

- [ ] **Step 1: Write acceptance tests directly from the product criteria**

Create named tests:

```rust
#[test] fn empty_launch_accepts_input_before_optional_work() { AcceptanceHarness::new().empty_launch_order(); }
#[test] fn json_window_appears_before_file_and_lexer_finish() { AcceptanceHarness::new().json_launch_order(); }
#[test] fn json_parse_is_not_called_on_startup() { AcceptanceHarness::new().assert_no_startup_json_parse(); }
#[test] fn markdown_never_loads_a_browser_runtime() { AcceptanceHarness::new().assert_no_browser_module(); }
#[test] fn corrupt_settings_and_recovery_do_not_block_input() { AcceptanceHarness::new().corrupt_state_order(); }
#[test] fn normal_binary_has_no_network_imports() { AcceptanceHarness::new().assert_no_network_imports(); }
#[test] fn app_works_without_resident_mode() { AcceptanceHarness::new().assert_no_resident_process(); }
```

Define `AcceptanceHarness` in `tests/windows/support/acceptance.rs` with the seven methods shown above. Implement them using `FastPadProcess`, diagnostic milestones, loaded-module enumeration, a temporary LocalAppData override available only to tests, and `dumpbin /imports` rejection of `ws2_32.dll`, `winhttp.dll`, `wininet.dll`, and `urlmon.dll`. Each method closes every process and removes only its owned temporary data.

- [ ] **Step 2: Add release-profile comparison**

Define Cargo profiles exactly as follows, then build signed artifacts and run 100 warm iterations for each. Record executable size, p50, p95, first paint, and idle working set in `benchmarks/release-profile.md`; choose the profile with the lowest compliant TTI rather than assuming fat LTO wins. Record cold TTI from the first launch after each of ten documented clean reboots of the reference machine; do not use an undocumented cache-purge utility.

```toml
[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
panic = "abort"
strip = true

[profile.release-size]
inherits = "release"
opt-level = "z"
lto = "thin"

[profile.release-thin]
inherits = "release"
opt-level = 3
lto = "thin"
```

- [ ] **Step 3: Implement dependency and package verification**

`audit-dependencies.ps1` parses `cargo metadata --locked --format-version 1`, allows only the transitive closure of `windows-sys 0.61.2` and `serde_json 1.0.151`, and explicitly rejects HTTP, TLS, socket, updater, telemetry, async-runtime, and UI-framework crates.

`package.ps1` builds the selected release profile, stages exactly the executable, two DLLs, README, and license files, verifies hashes and AMD64 machine type, invokes configured signing when `FASTPAD_SIGNING_CERTIFICATE` is present, then creates `dist/FastPad-0.1.0-windows-x64.zip`.

`verify-package.ps1` expands the ZIP into a fresh directory, rejects extra/missing files, verifies signatures when `-RequireSignature` is used, launches from a working directory unrelated to the package, runs the smoke test, and confirms no DLL was loaded from outside the package or System32.

- [ ] **Step 4: Add CI jobs with correct performance semantics**

The workflow contains:

- `unit-linux`: format, Clippy, unit tests;
- `windows-integration`: native fetch verification/build, Windows tests serially;
- `dependency-audit`: locked metadata audit;
- `package`: portable ZIP verification and artifact upload;
- `benchmark-trend`: 30 hosted-runner samples uploaded as JSONL, never enforcing absolute milliseconds.

Only the documented reference-machine workflow may pass `--enforce-reference`.

- [ ] **Step 5: Run the complete release candidate gate**

Run on Windows reference hardware:

```powershell
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --target x86_64-pc-windows-msvc -- --test-threads=1
pwsh -File tools/audit-dependencies.ps1
pwsh -File tools/package.ps1
pwsh -File tools/verify-package.ps1 -RequireSignature
cargo run --release --bin fastpad-bench -- --runs 100 --warmup 10 --enforce-reference
```

Expected: every command succeeds; warm TTI is below 25 ms p50 and 40 ms p95; warm first paint is below 20 ms; the ten-reboot cold TTI p50 is below 100 ms; idle private working set is below 20 MB; the signed portable package passes all acceptance tests on Windows 10 and Windows 11 x64.

- [ ] **Step 6: Commit the release system**

```bash
git add Cargo.toml Cargo.lock README.md LICENSES.md tools .github tests/windows/acceptance.rs benchmarks/release-profile.md
git commit -m "build: add FastPad release and acceptance gates"
```

---

### Task 19: Title shell and theme polish

Added 2026-09-16 at the owner's request after manual review of the running build. Owner-approved design: FastPad owns the whole top edge and custom-draws theme-matched caption buttons; tabs, editor margin, selection, and scrollbars follow the theme.

**Files:**
- Modify: `src/window/titlebar.rs`
- Create: `src/window/palette.rs`
- Modify: `src/window/main_window.rs`
- Modify: `src/editor/scintilla.rs`
- Modify: `tools/generate-scintilla-constants.ps1` (regenerate `src/editor/scintilla_constants.rs`; never hand-edit)
- Modify: `src/platform/theme.rs`
- Test: `src/window/titlebar.rs`, `src/window/palette.rs`
- Test: `tests/windows/titlebar.rs`

**Interfaces:**
- Consumes: `App::theme` (`SystemTheme`), `Settings::theme`, `effective_dark`, `WM_FASTPAD_BUILD_CHROME` and the theme-change messages, `TitleBarLayout`, `Editor` style APIs
- Produces: `Palette`, per-DPI cached title fonts, hover/pressed caption state, themed editor chrome

- [ ] **Step 1: Write failing layout, hit-test, and palette tests**

Unit tests: caption buttons occupy the right edge with no native-caption gap (layout top is 0 and the window keeps no DWM caption band); `hit_test` maps the maximize rect to `HitTarget::Maximize` (reported as `HTMAXBUTTON` for snap layouts) and the top resize band of a restored window to a resize target; `Palette::for_theme(dark, high_contrast)` returns distinct dark/light palettes, high contrast uses system colors, and the active tab background equals the editor background. Integration: the title-bar target asserts exactly one set of caption buttons (no native caption buttons visible above the strip: the client area starts at the window's top border) and that minimize/maximize/close still work.

- [ ] **Step 2: Remove the native caption band and draw caption buttons**

`WM_NCCALCSIZE` keeps the proposed top edge (no reserved frame strip) while preserving left/right/bottom resize borders and correct maximized insets; call `SetWindowPos(..., SWP_FRAMECHANGED)` once after creation. Top-edge resizing of a restored window comes from hit testing a DPI-scaled band. Paint minimize/maximize/restore/close with Segoe MDL2 Assets glyphs (`E921`, `E922`/`E923`, `E8BB`), hover and pressed states via `WM_MOUSEMOVE`/`WM_MOUSELEAVE` (`TrackMouseEvent`) and non-client mouse messages, close hover red with white glyph. Keep DWM-first hit testing and `HTMAXBUTTON` so Windows 11 snap layouts remain available.

- [ ] **Step 3: Apply the palette to tabs and chrome**

`src/window/palette.rs` owns dark, light, and high-contrast colors. The first paint uses the neutral compiled palette; the theme palette applies only after `WM_FASTPAD_BUILD_CHROME` and on theme changes. Tab text uses Segoe UI at the window DPI; fonts are created lazily, cached, and recreated on `WM_DPICHANGED`. Active tab background equals the editor background; inactive tabs, `+`, `⋯`, and tab close glyphs are muted until hovered. Double-buffer the strip paint to avoid flicker.

- [ ] **Step 4: Theme the editor chrome**

Disable Scintilla's default symbol margin (width 0), set `SCI_SETSCROLLWIDTH(1)` with `SCI_SETSCROLLWIDTHTRACKING(1)` so the horizontal scrollbar appears only when needed, and apply theme selection and caret-line colors. In dark mode apply `DWMWA_USE_IMMERSIVE_DARK_MODE` to the frame and the `DarkMode_Explorer` window theme to the editor scrollbars; failures leave the light appearance without error.

- [ ] **Step 5: Verify**

Run on Windows: `cargo test --lib -- window::titlebar window::palette --test-threads=1; cargo test --test titlebar -- --test-threads=1`, strict Clippy, and one 30-run `fastpad-bench` comparison against the pre-task `release` numbers recorded in `benchmarks/release-profile.md`. Capture before/after screenshots for owner review.

- [ ] **Step 6: Commit**

```bash
git add src/window src/editor src/platform/theme.rs tools/generate-scintilla-constants.ps1 tests/windows/titlebar.rs benchmarks/release-profile.md
git commit -m "feat: polish title shell and theme"
```

---

### Task 20: Ignore unbound control-character keystrokes

Added 2026-09-16 at the owner's request: key combinations with no command must do nothing instead of inserting control characters. Root cause: an unbound Ctrl combination leaves Scintilla's `WM_KEYDOWN` unconsumed, `TranslateMessage` emits a C0 control-character `WM_CHAR` (Ctrl+A = 0x01 … Ctrl+Z = 0x1A, Ctrl+[ = 0x1B, Ctrl+2 = 0x00, Ctrl+Enter = 0x0A), and `ScintillaWin` inserts it because `IsVisualCharacter(c) || !lastKeyDownConsumed` (ScintillaWin.cxx:1983-1986), rendering blocks such as `SOH`/`DC1`. Owner-approved rule: ignore C0 (0x00-0x1F) and DEL (0x7F) keystroke characters, except Tab, CR, and LF when Ctrl is not held.

**Files:**
- Create: `src/editor/input_filter.rs`
- Modify: `src/editor/mod.rs`
- Modify: `src/editor/scintilla.rs`
- Test: `src/editor/input_filter.rs`
- Test: `tests/windows/editing.rs`

**Interfaces:**
- Consumes: the always-installed editor endpoint subclass (`editor_endpoint_subclass_proc`), Win32 key state
- Produces: `pub fn should_ignore_char(code: u16, ctrl_down: bool) -> bool`

- [ ] **Step 1: Write failing filter tests**

```rust
#[test]
fn unbound_control_characters_are_ignored_but_plain_whitespace_is_not() {
    assert!(should_ignore_char(0x11, true));   // Ctrl+Q -> DC1
    assert!(should_ignore_char(0x01, true));   // Ctrl+A -> SOH
    assert!(should_ignore_char(0x00, true));   // Ctrl+2 -> NUL
    assert!(should_ignore_char(0x1B, true));   // Ctrl+[ -> ESC
    assert!(should_ignore_char(0x7F, false));  // DEL
    assert!(should_ignore_char(0x0A, true));   // Ctrl+Enter -> LF
    assert!(should_ignore_char(0x09, true));   // Ctrl+I -> TAB
    assert!(!should_ignore_char(0x09, false));
    assert!(!should_ignore_char(0x0D, false));
    assert!(!should_ignore_char(0x0A, false));
    assert!(!should_ignore_char(u16::from(b'q'), true));
    assert!(!should_ignore_char(0x00E9, true)); // AltGr/Ctrl+Alt printable output stays
}
```

Integration (`tests/windows/editing.rs`): with a real editor, typing text containing `0x11`, `0x01`, and `0x7F` through `WM_CHAR` while Ctrl is held (via `SendInput` key state, matching the file's existing `send_key` helper) leaves the document unchanged, while plain `a\tb\r` still inserts `a`, a tab, `b`, and a line end.

- [ ] **Step 2: Run the red tests**

Run: `cargo test --lib -- editor::input_filter --test-threads=1`
Expected: FAIL because the filter is absent.

- [ ] **Step 3: Filter in the editor subclass**

In `editor_endpoint_subclass_proc`, return 0 without calling `DefSubclassProc` for `WM_CHAR` when `should_ignore_char(wparam as u16, ctrl_down)` is true, with `ctrl_down` from `GetKeyState(VK_CONTROL) < 0`. All other messages (including `WM_NCDESTROY` handling) are unchanged. No accelerator, menu, find-bar, or Scintilla keymap changes.

- [ ] **Step 4: Verify**

Run on Windows: `cargo test --lib -- editor::input_filter --test-threads=1; cargo test --test editing -- --test-threads=1; cargo test --test startup_smoke -- --test-threads=1`, plus strict Clippy and fmt.

- [ ] **Step 5: Commit**

```bash
git add src/editor tests/windows/editing.rs
git commit -m "fix: ignore unbound control-character keystrokes"
```

---

## Pinned Source References

- [`windows-sys` 0.61.2](https://docs.rs/crate/windows-sys/0.61.2)
- [`serde_json` 1.0.151](https://docs.rs/crate/serde_json/1.0.151)
- [Scintilla 5.6.6 source](https://www.scintilla.org/ScintillaDownload.html)
- [Lexilla 5.5.3 source](https://www.scintilla.org/LexillaDownload.html)
- [Scintilla direct calls, document handles, and static/dynamic integration](https://www.scintilla.org/ScintillaDoc.html)
- [Lexilla `CreateLexer` protocol](https://www.scintilla.org/LexillaDoc.html)
- [Microsoft snap-layout requirements for custom Win32 title bars](https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/ui/apply-snap-layout-menu)

---

## Specification Coverage Map

| Design requirement | Implemented and verified by |
|---|---|
| Dependency policy and release profile | Tasks 1, 3, 18 |
| Startup allowlist and deferred work | Tasks 4, 6, 7, 15–17 |
| Raw Win32 editor-first shell | Tasks 6 and 8 |
| Scintilla-owned buffers and tabs | Tasks 5 and 9 |
| UTF file open/save and atomic replacement | Tasks 2, 10, 11 |
| Undo/redo, clipboard, find/replace | Task 12 |
| JSON/Markdown highlighting | Task 13 |
| Explicit JSON validation/formatting | Task 14 |
| Settings, themes, status, and nonfatal errors | Task 15 |
| Deferred crash recovery | Task 16 |
| Default instance reuse and `--new-window` | Task 17 |
| TTI, first paint, cold start, and idle memory | Tasks 7, 8, 18 |
| Security, dependency audit, signing, portable ZIP | Tasks 3, 17, 18 |
| Product acceptance criteria and Windows 10/11 x64 | Task 18 |
| Deferred resident mode, plugins, preview, cloud, Git, projects, LSP, terminal, updater, and cross-platform work | Global constraints and Task 18 dependency/import assertions |

---

## Execution Checkpoints

- After Task 7: review the minimal editor and reference startup baseline before custom UI.
- After Task 8: approve the integrated title shell only if its TTI distribution remains compliant.
- After Task 11: manually review document safety, encoding preservation, and atomic-save failure behavior.
- After Task 14: review the complete core editor feature set before persistence and IPC.
- After Task 17: review resilience and process integration with all optional work deferred.
- After Task 18: review signed-package performance and acceptance evidence before declaring MVP complete.
