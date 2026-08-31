# FastPad MVP Design

Status: Approved design  
Date: 31 August 2026  
Product specification: `FastPad_Rust_Product_Technical_Specification.docx`

## 1. Purpose

FastPad is a Windows-only text editor written in Rust and optimized for the shortest possible time from process launch to visibly rendered keyboard input. It serves scratch notes and lightweight editing of plain text, JSON, Markdown, logs, and configuration files. It is not an IDE, workspace manager, or extensible editor platform.

The architectural rule is:

> Nothing not required for the first editable frame may block the first editable frame.

The MVP covers the complete scope in the product specification, delivered in measured phases. Features that have no measured need remain deferred.

## 2. Goals and constraints

### Product goals

- Warm Time To Input (TTI) below 25 ms p50 and 40 ms p95 on the reference machine.
- Warm first paint below 20 ms.
- Cold TTI below 100 ms when storage and security software permit it.
- Idle private working set below 20 MB, with below 10 MB desirable.
- Native Windows behavior and excellent keyboard access.
- Fast open, save, search, JSON operations, and Markdown highlighting.
- Predictable performance without resident mode.

### Architectural constraints

- Rust stable toolchain and `windows-sys`; no UI framework.
- Scintilla owns all live document text.
- Lexilla provides JSON and Markdown lexers and initializes on demand.
- `serde_json` runs only for explicit JSON commands.
- No async runtime, generalized application framework, or permanent worker pool.
- No network access during normal startup or normal editing.
- Release builds use aborting panics and contain no backtrace framework.

### Explicitly deferred

Plugins, Markdown preview, cloud sync, Git integration, projects, sidebars, language servers, an embedded terminal, startup update checks, cross-platform support, a complex command palette, resident mode, and ordinary session restoration are not part of the MVP.

## 3. Product decisions

| Area | Decision |
|---|---|
| Supported systems | Windows 10 and Windows 11, x64 only |
| Distribution | Installation-free portable folder/ZIP |
| Package contents | `FastPad.exe`, `Scintilla.dll`, and `Lexilla.dll`, plus licenses |
| Application data | `%LocalAppData%\FastPad` |
| Encodings | UTF-8, UTF-8 BOM, UTF-16 LE, and UTF-16 BE |
| Instance policy | Reuse the existing instance by default; `--new-window` starts another process |
| Window shell | Editor-first, integrated title tabs, status bar, and overflow menu |
| Menu access | Alt/F10 reveals conventional File/Edit/Search/View/Help menu mode |
| Clean restart | Do not restore ordinary tabs |
| Crash restart | Restore snapshots as marked tabs after the editor becomes interactive |
| JSON format | Two spaces, preserve final-newline state, one undoable edit |
| Performance gate | Absolute limits on documented reference hardware; trends only on hosted CI |

## 4. Runtime architecture

FastPad uses one Win32 UI thread. Application state remains flat and explicit:

```rust
pub struct App {
    hwnd: HWND,
    editor: Editor,
    tabs: Tabs,
    settings: Settings,
    startup: StartupMetrics,
    ipc: Option<IpcServer>,
}

pub struct Document {
    id: DocumentId,
    handle: ScintillaDocumentHandle,
    path: Option<PathBuf>,
    language: Language,
    encoding: Encoding,
    dirty: bool,
    recovery_id: RecoveryId,
}
```

`Document` contains metadata and a Scintilla document handle, never a Rust copy of the live text. Temporary buffers are permitted only while opening, saving, transforming JSON, or writing recovery data. They never become authoritative state.

### 4.1 Latency-critical startup path

The pre-input allowlist is deliberately narrow:

1. Record the process-start counter.
2. Inspect only the arguments needed for `--new-window`, diagnostic mode, and a deferred file request.
3. Check the per-user/session instance mutex unless `--new-window` is present.
4. Configure per-monitor DPI awareness.
5. Resolve the executable directory and load only `Scintilla.dll` with safe loader flags.
6. Register the FastPad main-window class.
7. Create the main HWND, minimal integrated-title surface, and plain-text Scintilla child.
8. Show the window, focus Scintilla, and enter message dispatch.
9. Record first paint and first accepted/rendered input into fixed storage.

The initial title surface paints one lightweight tab directly. It does not create tab child windows, animations, acrylic/Mica effects, decoded toolbar icons, a status bar, a full menu model, or theme infrastructure.

### 4.2 Deferred startup path

After the editor accepts input, FastPad posts ordered `WM_APP` work:

```rust
const WM_FASTPAD_LOAD_SETTINGS: u32 = WM_APP + 1;
const WM_FASTPAD_OPEN_REQUEST: u32 = WM_APP + 2;
const WM_FASTPAD_APPLY_LANGUAGE: u32 = WM_APP + 3;
const WM_FASTPAD_RECOVERY: u32 = WM_APP + 4;
const WM_FASTPAD_START_IPC: u32 = WM_APP + 5;
const WM_FASTPAD_BUILD_CHROME: u32 = WM_APP + 6;
```

Deferred work loads setting differences, builds the status/menu models, opens the requested file, loads Lexilla when needed, discovers recovery snapshots, and starts the named-pipe server. Theme lookup, font validation, file-path normalization, recovery timers, diagnostics formatting, and all logging are also deferred.

## 5. Component boundaries

### `bootstrap`

Owns the latency-critical path: counters, minimal arguments, instance detection, DPI, Scintilla loading, window creation, and initial deferred posts. It cannot depend on configuration, recovery, JSON, Lexilla, or pipe-server implementation.

### `platform`

Contains the narrow unsafe boundary around `windows-sys`: owned handle wrappers, UTF-16 conversion, DLL loading, DPI, DWM and non-client behavior, common dialogs, mutexes, overlapped pipes, monotonic counters, and atomic replacement. Unsafe functions expose small typed interfaces and document lifetime requirements.

### `window`

Owns `WndProc`, client layout, integrated title tabs, overflow and Alt/F10 menus, status and notification bars, command routing, and DPI/theme/high-contrast responses. DWM receives non-client messages before FastPad hit testing. The maximize area returns `HTMAXBUTTON` where required so Windows 11 snap layouts remain available.

The title strip uses direct, lightweight GDI painting with system metrics and colors. It contains no animation or composition effect. Caption buttons, dragging, resizing, double-click maximize, system menu, keyboard navigation, RTL-safe geometry, touch-sized hit targets, and high-contrast colors are covered by Windows integration tests.

### `editor`

Is the only module allowed to issue `SCI_*` messages. It wraps the editor HWND and exposes typed editing, search, styling, selection, undo-group, save-point, and document-handle operations. After creation, frequent synchronous operations may use Scintilla's direct-call interface; ordinary messages remain available where Windows notification ordering matters.

### `documents`

Owns tab order, the active document, path uniqueness checks, close decisions, dirty state, and the reference lifecycle of Scintilla document handles. Switching tabs attaches an existing native document handle to the single editor HWND without copying its text.

### Deferred feature modules

- `file`: byte loading, encoding conversion, dialogs, atomic saving, and path updates.
- `languages`: extension detection, deferred Lexilla loading, lexer creation, and styles.
- `json`: explicit validate and format commands using `serde_json`.
- `config`: compiled defaults and a small hand-written INI-like delta parser.
- `recovery`: idle snapshot scheduling, atomic snapshots, discovery, and cleanup.
- `ipc`: instance mutex, framed named-pipe protocol, and UI dispatch.
- `perf`: fixed startup counters, diagnostic transport, and measurement labels.

Dependencies flow from `bootstrap` to `app/window`, then to focused feature modules, then to `editor/platform`. There is no dependency-injection container, event bus, mediator, service locator, or cross-platform abstraction.

## 6. Native dependencies and packaging

The default integration is dynamic:

- Scintilla is loaded from the resolved application directory before editor creation.
- Lexilla is not loaded until a JSON or Markdown document requires a lexer.
- The loader never relies on the current working directory or an unrestricted DLL search path.
- Exact upstream versions and SHA-256 checksums are recorded in the repository.
- Windows build scripts produce the x64 DLLs and assemble a portable staging directory.
- Scintilla and Lexilla license files ship in the ZIP.

Dynamic loading is the baseline because it is the normal supported Windows integration and keeps native upgrades separate. Static Scintilla is a documented contingency: it is evaluated only if measurements show DLL loading materially threatens TTI. Fully static Lexilla is not planned for the MVP.

## 7. Document and command behavior

### 7.1 New and tabs

New creates a Scintilla document handle and UTF-8-without-BOM metadata. The plus button and Ctrl+N create tabs. A tab close requests Save, Discard, or Cancel when dirty. Closing the last tab creates a fresh empty tab; it does not terminate the process. Closing the window reviews dirty tabs in tab order; Cancel aborts the close and leaves the remaining tabs untouched.

FastPad does not restore tabs after a clean exit. Tabs originate from user commands, launch/IPC requests, or crash recovery.

### 7.2 Open

Opening occurs after the first-input boundary:

1. Read ordinary files with buffered I/O.
2. Detect UTF-8 BOM, UTF-16 LE BOM, or UTF-16 BE BOM; otherwise require valid UTF-8.
3. Convert transiently to the UTF-8 representation expected by Scintilla.
4. Populate the target native document with undo collection disabled.
5. clear undo history, establish a save point, and record metadata.
6. Detect language from extension and post lexer activation.

Unsupported encodings do not replace the current tab and produce a persistent notification. The MVP adds no speculative memory mapping or background loader. File-open duration is instrumented; large-file work is added only after measurements demonstrate a need.

### 7.3 Save

Saving reads the current Scintilla bytes transiently, converts them to the document's original UTF encoding and BOM policy, writes a same-directory temporary file, flushes and closes it, and atomically replaces the destination. A new destination is atomically renamed into place. Success updates the path, language, and save point. Failure preserves the original destination and leaves the tab dirty.

### 7.4 Search and editing

Undo, redo, clipboard commands, and search use Scintilla directly. Ctrl+F and Ctrl+H open a compact in-window find/replace bar. Search wraps once with clear feedback. Replace All is one undo group. Command enablement queries editor state directly rather than maintaining a parallel command-state graph.

### 7.5 JSON and Markdown

Extension or explicit language selection activates a Lexilla lexer without parsing document contents. Markdown supports highlighting only.

JSON Validate parses the complete current document on command. On failure it reports line and column and offers caret navigation without altering text. JSON Format parses and emits two-space indentation, preserves whether the document ended with a newline, replaces the full document as one undo group, and restores the selection by logical line/column with clamping. Invalid JSON leaves text and undo history unchanged.

## 8. Settings and theme

Compiled defaults produce a complete usable editor. After input, FastPad reads `%LocalAppData%\FastPad\fastpad.ini`, parses only recognized `key=value` differences, and applies them without rebuilding the editor. Initial settings include font face/size, tab width, word wrap, theme (`system`, `light`, or `dark`), and recovery interval.

The compiled startup theme is neutral and immediately usable. System-theme detection and any configured override apply afterward. High contrast always overrides decorative color choices. Corrupt keys are ignored individually; one bad setting never rejects the whole file.

## 9. Recovery

Recovery uses `SetTimer` only after deferred initialization. A dirty-generation counter changes on Scintilla modification notifications. After at least two seconds without input, each timer tick snapshots the next document whose dirty generation has not been recorded. The snapshot contains versioned metadata and UTF-8 text and is committed by atomic replacement under `%LocalAppData%\FastPad\Recovery`.

Snapshot work stays on the UI thread for the initial MVP, runs only while idle, handles at most one document per tick, and records its duration. If measurement shows a material input-latency risk, an operation-specific worker becomes justified; no general thread pool is introduced.

Recovery discovery happens after input. Valid snapshots open as clearly marked recovered tabs and a non-modal notice explains their origin. A recovered tab has no destructive relationship with its original file until the user explicitly saves. Malformed snapshots are quarantined, not deleted. Successfully saved or discarded recovered documents remove their snapshots.

## 10. Single-instance IPC

A local named mutex identifies the primary instance. Unless `--new-window` is present, a later process connects to a named pipe and sends one versioned, length-prefixed request: `Open`, `New`, or `Activate`. Frames are capped at 64 KiB and paths use UTF-16 payloads.

The primary creates the pipe server only after input. A second process that sees the mutex before the server is ready performs a short bounded `WaitNamedPipe` retry; this affects only secondary launches. Pipe access is restricted to the current interactive user.

The pipe uses overlapped I/O. The single UI thread multiplexes the pipe event and Windows messages with `MsgWaitForMultipleObjectsEx`; complete requests become `WM_APP` commands. No async runtime, thread pool, or permanent IPC thread is required. Invalid frames are rejected without changing application state.

## 11. Error handling and security

`FastPadError` is a small hand-written enum covering Win32 error codes, I/O, unsupported encoding, JSON, malformed IPC, and internal invariants. The core uses a local `Result<T>` alias without `anyhow`, `thiserror`, a logging framework, or release backtraces.

Only failure to create an editable window is startup-fatal: incompatible/missing Scintilla, main-window registration failure, or editor creation failure. FastPad shows a minimal native error dialog and exits with a distinct code.

Deferred failures never invalidate startup:

- Bad settings fall back to compiled defaults.
- Missing Lexilla leaves the document in plain-text mode.
- IPC-server failure allows the current process to continue.
- Recovery failures preserve the source snapshots.
- Failed file operations leave the current document and dirty state intact.
- JSON failures never modify the buffer.

User-action failures appear in a persistent in-window notification bar. Diagnostic details are gathered only under `--diagnostic` and formatted after input.

Release DLLs load by absolute path with safe Windows loader flags. Pipe messages are bounded and access-controlled. FastPad canonicalizes paths for identity comparisons but does not follow them speculatively during startup. No network-capable crate is allowed in the runtime dependency graph. Distributed release artifacts are code-signed.

Release builds use `panic = "abort"`; prior atomic recovery snapshots are the crash-safety mechanism.

## 12. Performance engineering

### 12.1 Measurement

Startup counters use `QueryPerformanceCounter` and a fixed `StartupMetrics` structure. Recording performs no formatting, file I/O, logging, or heap growth. Diagnostic formatting and transport occur after input.

A companion `fastpad-bench` binary launches release artifacts, injects a known keystroke, and collects:

- process start;
- window created;
- editor created;
- first paint;
- first accepted and rendered input;
- settings loaded;
- requested file loaded;
- fully deferred-ready.

The harness reports distributions, not single observations. "Accepted input" is the first benchmark keystroke delivered to Scintilla; "rendered input" is the first completed Scintilla paint containing the resulting modification. Benchmark-only signaling occurs after each timestamp and is included consistently in baseline and comparison runs. The documented Windows reference machine enforces warm p50/p95 TTI and idle-memory limits. Hosted CI reports trends but does not enforce absolute milliseconds. Cold runs are tracked separately and final release numbers use signed artifacts.

### 12.2 Performance gates and contingencies

Every startup-path change requires before/after distributions. A statistically material regression blocks the change even while the absolute budget still passes.

The specification's `opt-level = 3`, fat LTO, one codegen unit, aborting panic, and stripping form the initial release profile. Before release it is compared with smaller-image profiles using `opt-level = "s"` or `"z"` and thin LTO. The fastest measured signed artifact wins; the profile is not selected by assumption.

The following require measurements before adoption:

- static Scintilla linking;
- background large-file loading;
- background JSON transformation;
- memory mapping;
- profile-guided optimization;
- any resident process.

The default allocator remains the Windows-backed Rust system allocator. FastPad adds no custom allocator, startup logger, font enumeration, eager lexer configuration, or eager shell integration.

## 13. Testing strategy

### Cross-platform unit tests

Pure Rust tests cover minimal arguments, encoding detection and round trips, INI parsing, language detection, document/tab transitions, JSON formatting, and IPC frame validation. They run on Linux and Windows.

### Windows integration tests

Tests create real Scintilla controls and cover document-handle switching, reference release, save-point notifications, undo grouping, Lexilla activation, file round trips, dialog adapters, menu access, custom-title hit testing, snap-layout hover behavior, DPI changes, and high contrast.

### CI gates

- `cargo fmt --check`;
- Clippy with warnings denied;
- unit and Windows integration tests;
- native DLL build and version compatibility;
- portable-package integrity and licenses;
- runtime dependency scan for network-capable crates;
- benchmark trend report;
- all product acceptance scenarios.

Acceptance scenarios include corrupt settings, malformed recovery data, missing Lexilla, malformed IPC, launch with JSON/Markdown files, first input before deferred work, and operation with resident mode absent.

## 14. Proposed source layout

```text
Cargo.toml
src/
  main.rs
  app.rs
  bootstrap.rs
  error.rs
  document.rs
  window/
    mod.rs
    main_window.rs
    titlebar.rs
    tabs.rs
    menus.rs
    commands.rs
    messages.rs
  editor/
    mod.rs
    scintilla.rs
    lexer.rs
  file/
    mod.rs
    loader.rs
    saver.rs
    encoding.rs
  languages/
    mod.rs
    json.rs
    markdown.rs
  config/
    mod.rs
    defaults.rs
    persisted.rs
  recovery/
    mod.rs
    snapshot.rs
  ipc/
    mod.rs
    protocol.rs
    server.rs
  platform/
    mod.rs
    handles.rs
    win32.rs
    dpi.rs
    theme.rs
  perf/
    mod.rs
    startup.rs
src/bin/
  fastpad-bench.rs
tests/
tools/
  build-native.ps1
  package.ps1
```

Files remain focused and are split when a module begins mixing ownership domains. The proposed layout is a guide, not permission to create unused placeholders.

## 15. Delivery phases

1. **Native dependency foundation:** initialize Cargo, pin dependencies, reproducibly build x64 Scintilla/Lexilla DLLs, and package the portable artifact.
2. **Startup baseline:** create a stock Win32 frame with one plain Scintilla editor, fixed metrics, and the benchmark harness. Record the reference baseline.
3. **Editor-first shell:** add integrated title tabs, custom caption behavior, Alt/F10 menus, deferred status bar, theme, DPI, and high contrast. Compare TTI before and after.
4. **Document lifecycle:** add native document handles, tabs, new/open/save/save-as, UTF encodings, dirty state, close review, command-line files, and atomic saves.
5. **Editing features:** add undo/redo, clipboard, find/replace, keyboard routing, and caret/selection behavior.
6. **Languages and JSON:** add deferred Lexilla, JSON/Markdown highlighting, validation, and formatting.
7. **Persistence and resilience:** add settings, system theme overrides, idle recovery, restoration, and failure notifications.
8. **Process integration:** add the mutex, overlapped named pipe, instance reuse, and `--new-window`.
9. **Hardening and release:** finish acceptance tests, performance and memory distributions, release-profile comparison, signing, licensing, packaging, and Windows 10/11 verification.

Each phase ends with a working application, focused tests, and a startup comparison where it can affect TTI. A later phase cannot erase a failed performance gate from an earlier one.

This master design maps to one phased implementation plan. Each phase remains independently executable and reviewable; work does not begin on a later phase until the preceding phase's correctness and performance gates pass.

## 16. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Scintilla DLL loading dominates warm TTI | Measure immediately; test static Scintilla only if material |
| Integrated title tabs break native behavior | DWM-first dispatch, explicit hit tests, and Windows 10/11 accessibility tests |
| Security scanning dominates cold start | Keep and sign a small portable artifact; measure signed builds separately |
| Recovery snapshot stalls input | Run only after idle, one document per tick, instrument duration, then justify a worker if needed |
| Large files stall the UI | Instrument open and editor-population stages before selecting a loading strategy |
| JSON transforms block on huge documents | Instrument command duration; introduce an operation-specific worker only after a measured threshold |
| Deferred theme causes visible repaint | Use a neutral compiled theme and apply only changed properties |
| Mutex exists before pipe is ready | Use bounded secondary-client retry; never delay primary TTI for server creation |
| Native dependency drift breaks ABI | Pin versions/checksums and test the packaged DLL pair in CI |

## 17. Definition of MVP completion

The MVP is complete when all product-specification acceptance criteria pass, the portable x64 package runs on supported Windows 10 and 11 systems, the reference machine meets warm TTI and memory budgets, all optional initialization occurs after first input or explicit invocation, and the editor functions correctly without resident mode or network access.

## 18. Implementation references

- [Scintilla documentation](https://www.scintilla.org/ScintillaDoc.html)
- [Lexilla documentation](https://www.scintilla.org/LexillaDoc.html)
- [Scintilla 5 lexer migration](https://www.scintilla.org/Scintilla5Migration.html)
- [Microsoft: support snap layouts for Windows 11 desktop apps](https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/ui/apply-snap-layout-menu)
