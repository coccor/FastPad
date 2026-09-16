# Markdown Preview Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A live, GitHub-like Markdown preview (Split and Full modes) rendered natively with Direct2D/DirectWrite, costing nothing at startup and never delaying typing.

**Architecture:** A pure-Rust block model (`pulldown-cmark`) with an incremental reparse engine feeds a lazily created `FastPadPreview` child window that lays out only visible blocks with DirectWrite and paints them with Direct2D. A window-level preview host in `src/window/preview_host.rs` owns the mode, split layout, edit log, debounce timer, scroll sync, and link actions; `main_window.rs` only calls into it from existing hooks.

**Tech Stack:** Rust 2024, `windows-sys 0.61.2` (existing), `pulldown-cmark 0.13.4`, `windows 0.62.2` + `windows-numerics 0.3.1` (Direct2D, DirectWrite, WIC interface types), Scintilla 5.6.6.

**Spec:** `docs/superpowers/specs/2026-09-16-markdown-preview-design.md`

## Global Constraints

- Nothing not required for the first editable frame may block the first editable frame.
- No preview code runs and no `d2d1.dll`, `dwrite.dll`, or `windowscodecs.dll` loads before a preview command; none of them may appear in the `FastPad.exe` import table.
- `D2D1CreateFactory` and `DWriteCreateFactory` are resolved only through `LoadLibraryExW(LOAD_LIBRARY_SEARCH_SYSTEM32)` + `GetProcAddress`; never call the `windows` crate's free functions of the same names (they add static imports).
- `SCN_MODIFIED` handling for the preview does O(1) work plus a scan of the changed text length only; no parsing, no layout.
- Debounce: `PREVIEW_UPDATE_DELAY_MS = 120`. Pending edits cap: `MAX_PENDING_EDITS = 64`.
- Large documents: above `WORKER_PARSE_THRESHOLD = 1_048_576` bytes full reparses run on a worker; above `LIVE_UPDATE_LIMIT = 10_485_760` bytes live updates pause behind a "Refresh preview" bar.
- Images: local files only, decoded on a worker, never upscaled, rejected above 64 megapixels.
- Links act only on click or Enter; `http:`, `https:`, `mailto:` via `ShellExecuteW`; nothing is ever fetched.
- Divider: 4 px at 96 DPI, ratio clamped to 0.2–0.8, double-click resets to 0.5, session-only.
- Content padding 16 DIPs; Full mode content width capped at 980 DIPs and centered.
- Dependencies are pinned exactly: `pulldown-cmark = "=0.13.4"` (default features off), `windows = "=0.62.2"` (default features off, features limited to the list in Task 1), `windows-numerics = "=0.3.1"`.
- Run discipline: compile via `cargo clippy --all-targets`, run only the targeted tests named in each task; the full suite `cargo test -- --test-threads=1` and benchmarks run only in Task 18. Back up and restore `%LocalAppData%\FastPad\fastpad.ini` around every live-app run.
- Commit messages end with `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`.

## Clarifications to the spec made while planning

These resolve details the spec left open; implementers follow them.

1. **Inline images.** A paragraph whose only content is images becomes an `Images` block (each image drawn scaled to width). An image inside a text paragraph renders as its alt text in italic muted style.
2. **Link and muted colors inside a paragraph** use DirectWrite drawing effects, which must be Direct2D brushes. Text layouts therefore depend on the render target's brushes: device loss clears layouts (visible blocks re-lay out on the next paint) in addition to recreating the target. This replaces the spec's "text layouts survive device loss".
3. **Yielding to input.** The flush checks `input_pending()` before parsing and re-arms the timer if input is queued. Layout happens only inside `WM_PAINT`, which Windows delivers only when no input is queued.
4. **Incremental boundary rule.** The reparse slice spans two unchanged sentinel blocks on each side of the dirty span; it is accepted only when the first and last reparsed blocks equal their sentinels, otherwise it widens (up to 8 times) and then falls back to a full parse. Slices containing `]:` and edits overlapping a link reference definition force a full parse. Slices resolve references through the document's definition list.
5. **Accessibility child ids.** The two title-strip buttons are appended after Close (ids `tabs + 5` and `tabs + 6`) so existing ids stay stable.
6. **Palette and menu.** Preview commands are listed in the palette only while the active tab is Markdown. The View menu always lists them, grayed out on non-Markdown tabs. Ctrl+Shift+V on a non-Markdown tab shows the notice.

## File Structure

| Path | Status | Responsibility |
|---|---|---|
| `Cargo.toml` | Modify | New pinned dependencies; `markdown_preview` test target |
| `tools/audit-dependencies.ps1` | Modify | Allow the new roots and their exact closure; restrict `windows` features |
| `LICENSES.md` | Modify | New crate rows |
| `tools/generate-scintilla-constants.ps1`, `src/editor/scintilla_constants.rs` | Modify | Constants for range pointers, visible lines, update flags |
| `src/editor/scintilla.rs` | Modify | `range_bytes`, `line_from_position`, `first_visible_line`, `set_first_visible_line`, `doc_line_from_visible`, `visible_from_doc_line`, `ScintillaNotification` |
| `src/preview/mod.rs` | Create | Module root, `PreviewMode`, size thresholds |
| `src/preview/model.rs` | Create | `Block`, `BlockKind`, `RichText`, `RefDef`, `parse_document`, `parse_blocks` |
| `src/preview/incremental.rs` | Create | `Edit`, `EditLog`, `SourceText`, `PreviewDocument`, `Update` |
| `src/preview/links.rs` | Create | `LinkAction`, `classify_link`, `resolve_image_path`, `SlugSet` |
| `src/preview/heights.rs` | Create | `HeightIndex`, `estimate_height`, `offset_for_line`, `line_for_offset` |
| `src/preview/colors.rs` | Create | `PreviewColors`, `ColorRole`, `preview_colors` |
| `src/preview/dwrite.rs` | Create | `Graphics` lazy factory loader |
| `src/preview/render.rs` | Create | `Brushes`, `RectF`, render-target creation, `draw_ops` |
| `src/preview/layout.rs` | Create | `LayoutContext`, `LaidBlock`, `DrawOp`, `LinkHit`, `ImageSlot`, `layout_block` |
| `src/preview/images.rs` | Create | `DecodedImage`, `decode_image`, `ImageCache` |
| `src/preview/view.rs` | Create | `PreviewView` window: paint, scroll, keys, links, stats |
| `src/preview/accessible.rs` | Create | MSAA provider for the preview document and its links |
| `src/window/preview_host.rs` | Create | `PreviewHost` state, `content_rects`, mode changes, update flush, scroll sync, link actions |
| `src/window/commands.rs`, `menus.rs`, `command_palette.rs` | Modify | Four commands, Ctrl+Shift+V, View menu, palette entries |
| `src/window/titlebar.rs` | Modify | Preview buttons: layout, hit targets, paint |
| `src/window/tabs.rs` | Modify | `TabView` carries `preview_buttons` for accessibility |
| `src/window/accessibility.rs` | Modify | Button children and default actions; share `RawVariant`/`allocate_bstr` |
| `src/window/messages.rs` | Modify | Preview window messages and diagnostic messages |
| `src/window/main_window.rs`, `src/app.rs` | Modify | Hook calls only |
| `tests/windows/markdown_preview.rs` | Create | Import guard, end-to-end behavior, `#[ignore]` performance measurements |
| `tests/windows/support/acceptance.rs` | Modify | `assert_no_preview_imports` |
| `README.md`, `benchmarks/README.md` | Modify | User docs and measurement procedure |

---

### Task 1: Dependencies, audit, licenses, and the import guard

**Files:**
- Modify: `Cargo.toml`
- Modify: `tools/audit-dependencies.ps1`
- Modify: `LICENSES.md`
- Modify: `tests/windows/support/acceptance.rs`
- Create: `tests/windows/markdown_preview.rs`
- Create: `src/preview/mod.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Produces: crate module `fastpad::preview` (initially only `PreviewMode` and constants); `AcceptanceHarness::assert_no_preview_imports(&self)`.

- [ ] **Step 1: Write the failing import-guard test**

Add to `tests/windows/support/acceptance.rs` next to `NETWORK_IMPORTS`:

```rust
const PREVIEW_IMPORTS: [&str; 3] = ["d2d1.dll", "dwrite.dll", "windowscodecs.dll"];
```

and next to `assert_no_network_imports`:

```rust
    pub fn assert_no_preview_imports(&self) {
        let dumpbin = locate_dumpbin();
        let binary = Path::new(env!("CARGO_BIN_EXE_fastpad"));
        let output = run_bounded(
            Command::new(&dumpbin)
                .arg("/nologo")
                .arg("/imports")
                .arg(binary),
            Duration::from_secs(120),
        )
        .unwrap_or_else(|error| panic!("dumpbin /imports {} failed: {error}", binary.display()));
        let imports = imported_dlls(&output);
        assert!(
            imports.iter().any(|name| name == "kernel32.dll"),
            "dumpbin output had no KERNEL32 import; parsing failed:\n{output}"
        );
        for forbidden in PREVIEW_IMPORTS {
            assert!(
                !imports.iter().any(|name| name == forbidden),
                "{} statically imports preview library {forbidden}: {imports:?}",
                binary.display()
            );
        }
    }
```

Create `tests/windows/markdown_preview.rs`:

```rust
#[cfg(windows)]
mod support;

#[cfg(windows)]
use support::acceptance::AcceptanceHarness;

/// Direct2D, DirectWrite, and WIC must load only when a preview opens, never through the import
/// table: a static import would load them into every launch and slow cold startup.
#[cfg(windows)]
#[test]
fn binary_does_not_statically_import_preview_graphics_libraries() {
    AcceptanceHarness::new().assert_no_preview_imports();
}
```

Add to `Cargo.toml` after the `single_instance` test target:

```toml
[[test]]
name = "markdown_preview"
path = "tests/windows/markdown_preview.rs"
```

- [ ] **Step 2: Run the guard to confirm the harness works (it passes today)**

Run: `cargo test --test markdown_preview binary_does_not_statically_import -- --test-threads=1`
Expected: PASS. It must pass before and after every later task; it is the regression tripwire.

- [ ] **Step 3: Add the dependencies**

In `Cargo.toml` `[dependencies]`, after `serde_json`:

```toml
pulldown-cmark = { version = "=0.13.4", default-features = false }
windows = { version = "=0.62.2", default-features = false, features = [
    "Win32_Foundation",
    "Win32_Graphics_Direct2D",
    "Win32_Graphics_Direct2D_Common",
    "Win32_Graphics_DirectWrite",
    "Win32_Graphics_Dxgi_Common",
    "Win32_Graphics_Imaging",
    "Win32_System_Com",
] }
windows-numerics = { version = "=0.3.1", default-features = false }
```

Create `src/preview/mod.rs`:

```rust
//! Markdown preview: a block model, incremental reparsing, and a lazily created Direct2D view.
//! Nothing here runs until the user opens a preview.

/// How the preview shares the content area with the editor. Owned by the window, not the tab.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PreviewMode {
    #[default]
    Off,
    Split,
    Full,
}

/// Documents larger than this reparse fully on a worker thread.
pub const WORKER_PARSE_THRESHOLD: usize = 1_048_576;
/// Documents larger than this pause live updates behind a "Refresh preview" bar.
pub const LIVE_UPDATE_LIMIT: usize = 10_485_760;
/// Idle time after the last edit before the preview updates.
pub const PREVIEW_UPDATE_DELAY_MS: u32 = 120;
```

Add `pub mod preview;` to `src/lib.rs` in alphabetical order (after `platform`).

- [ ] **Step 4: Update the dependency audit**

In `tools/audit-dependencies.ps1`:

1. Replace `$AllowedRoots` with:

```powershell
$AllowedRoots = @(
    [pscustomobject]@{ Name = "windows-sys"; Version = "0.61.2" },
    [pscustomobject]@{ Name = "serde_json"; Version = "1.0.151" },
    [pscustomobject]@{ Name = "pulldown-cmark"; Version = "0.13.4" },
    [pscustomobject]@{ Name = "windows"; Version = "0.62.2" },
    [pscustomobject]@{ Name = "windows-numerics"; Version = "0.3.1" }
)
# The `windows` crate is allowed only as the pinned direct dependency, with exactly these features.
$AllowedWindowsFeatures = @(
    "Win32_Foundation", "Win32_Graphics_Direct2D", "Win32_Graphics_Direct2D_Common",
    "Win32_Graphics_DirectWrite", "Win32_Graphics_Dxgi_Common", "Win32_Graphics_Imaging",
    "Win32_System_Com"
)
```

2. In the last `$RejectedPatterns` entry, delete the `windows|` alternative (leave `winit`, `winsafe`, `webview2`, and the rest untouched).

3. Inside the `foreach ($node in $metadata.resolve.nodes)` loop, after the `$RegistrySource` check, add:

```powershell
    if ($package.name -eq "windows") {
        $isRoot = $directDependencies | Where-Object { $_.id -eq $node.id }
        if ($null -eq $isRoot) {
            $failures.Add("$label may only appear as FastPad's direct dependency")
        }
        foreach ($feature in $node.features) {
            if ($AllowedWindowsFeatures -notcontains $feature) {
                $failures.Add("$label enables feature $feature, which is not allowed")
            }
        }
    }
```

4. Change the two messages that mention `windows-sys/serde_json closure` to `allowed dependency closure`, and the final `Write-Output` to:

```powershell
Write-Output "Dependency audit passed: $($closure.Count) crates in the allowed dependency closure."
```

- [ ] **Step 5: Run the audit and build**

Run: `cargo clippy --all-targets` then `pwsh -File tools/audit-dependencies.ps1`
Expected: Clippy clean; audit prints `Dependency audit passed` and lists `pulldown-cmark 0.13.4`, `bitflags`, `unicase`, `windows 0.62.2`, `windows-core`, `windows-result`, `windows-strings`, `windows-future`, `windows-threading`, `windows-collections`, `windows-numerics`, `windows-implement`, `windows-interface`, `syn 2.x`.

If the audit reports a `windows` feature beyond the list (for example `std` pulled in by another crate), stop and report it rather than widening the list.

- [ ] **Step 6: Update `LICENSES.md`**

Change the sentence under `## Rust crates` to: `FastPad depends directly on windows-sys 0.61.2, serde_json 1.0.151, pulldown-cmark 0.13.4, windows 0.62.2, and windows-numerics 0.3.1.` Then append one table row per crate newly present in `Cargo.lock`, taking version and license from `cargo metadata --locked --format-version 1`. Use these roles:

| Crate | Role |
|---|---|
| `pulldown-cmark` | Markdown preview parser (linked) |
| `bitflags`, `unicase` | `pulldown-cmark` dependency (linked) |
| `windows`, `windows-core`, `windows-result`, `windows-strings`, `windows-numerics` | Direct2D/DirectWrite/WIC interface bindings (linked) |
| `windows-future`, `windows-threading`, `windows-collections` | `windows` dependency (linked only if referenced) |
| `windows-implement`, `windows-interface`, `syn` 2.x | Build-time procedural macro |

`tools/package.ps1` already regenerates `licenses\rust-crates.txt` from metadata; no change there.

- [ ] **Step 7: Re-run the import guard and commit**

Run: `cargo test --test markdown_preview binary_does_not_statically_import -- --test-threads=1`
Expected: PASS.

```bash
git add Cargo.toml Cargo.lock tools/audit-dependencies.ps1 LICENSES.md src/lib.rs src/preview/mod.rs tests/windows/markdown_preview.rs tests/windows/support/acceptance.rs
git commit -m "build: add Markdown preview dependencies behind the audit and import guard"
```

---

### Task 2: Scintilla accessors for ranges, lines, and scroll notifications

**Files:**
- Modify: `tools/generate-scintilla-constants.ps1`
- Modify: `src/editor/scintilla_constants.rs` (regenerated)
- Modify: `src/editor/scintilla.rs`
- Modify: `src/window/main_window.rs` (replace `TextModificationNotification` with `ScintillaNotification`)

**Interfaces:**
- Produces on `Editor` (all `pub fn`, returning `crate::Result`):
  - `range_bytes(&self, range: Range<usize>) -> Result<&[u8]>` (borrow valid until the document is next modified)
  - `line_from_position(&self, position: usize) -> Result<usize>`
  - `first_visible_line(&self) -> Result<usize>` (display line)
  - `set_first_visible_line(&self, display_line: usize) -> Result<()>`
  - `doc_line_from_visible(&self, display_line: usize) -> Result<usize>`
  - `visible_from_doc_line(&self, doc_line: usize) -> Result<usize>`
- Produces in `crate::editor`: `#[repr(C)] pub struct ScintillaNotification` with public fields `header`, `position`, `modification_type`, `text`, `length`, `lines_added`, `updated`.

- [ ] **Step 1: Add constant names and regenerate**

Append to `$RequiredNames` in `tools/generate-scintilla-constants.ps1`:

```powershell
    "SCI_GETRANGEPOINTER", "SCI_SETFIRSTVISIBLELINE", "SCI_DOCLINEFROMVISIBLE",
    "SCI_VISIBLEFROMDOCLINE", "SC_UPDATE_V_SCROLL", "SCI_GOTOPOS", "SCI_DOCUMENTEND",
```

(`SCI_GETFIRSTVISIBLELINE`, `SCI_LINEFROMPOSITION`, `SCN_UPDATEUI` already exist; the generator skips names that are already present if it de-duplicates, otherwise drop any duplicate it reports. `SCI_GOTOPOS` and `SCI_DOCUMENTEND` are used by the Task 15 and Task 18 tests.)

Run: `pwsh -File tools/generate-scintilla-constants.ps1`
Expected: `src/editor/scintilla_constants.rs` gains the five constants (`SCI_GETRANGEPOINTER = 2643`, `SCI_SETFIRSTVISIBLELINE = 2613`, `SCI_DOCLINEFROMVISIBLE = 2221`, `SCI_VISIBLEFROMDOCLINE = 2220`, `SC_UPDATE_V_SCROLL = 0x4`).

- [ ] **Step 2: Write failing tests**

Add to the `#[cfg(test)]` module of `src/editor/scintilla.rs`, using the module's existing native-editor test helper (the same one its `set_text`/`text` tests use):

```rust
    #[test]
    fn range_bytes_returns_the_requested_slice_without_copying_the_document() {
        let editor = test_editor();
        editor.set_text("alpha\nbeta\ngamma").unwrap();
        assert_eq!(editor.range_bytes(6..10).unwrap(), b"beta");
        assert_eq!(editor.range_bytes(0..0).unwrap(), b"");
    }

    #[test]
    fn line_queries_map_positions_and_visible_lines() {
        let editor = test_editor();
        editor.set_text("a\nb\nc\nd\n").unwrap();
        assert_eq!(editor.line_from_position(4).unwrap(), 2);
        assert_eq!(editor.doc_line_from_visible(3).unwrap(), 3);
        assert_eq!(editor.visible_from_doc_line(3).unwrap(), 3);
        editor.set_first_visible_line(2).unwrap();
        assert!(editor.first_visible_line().unwrap() <= 2);
    }

    #[test]
    fn notification_struct_matches_scnotification_layout() {
        use std::mem::offset_of;
        assert_eq!(offset_of!(super::ScintillaNotification, position), 24);
        assert_eq!(offset_of!(super::ScintillaNotification, modification_type), 40);
        assert_eq!(offset_of!(super::ScintillaNotification, lines_added), 64);
        assert_eq!(offset_of!(super::ScintillaNotification, updated), 136);
    }
```

If the module's helper has a different name, use that name; do not add a second helper.

Run: `cargo test --lib editor::scintilla -- --test-threads=1`
Expected: FAIL to compile (`range_bytes` and `ScintillaNotification` not found).

- [ ] **Step 3: Implement**

In `src/editor/scintilla.rs` add, beside the other `Editor` methods (the real implementation block, not the `#[cfg(not(windows))]` stubs if the file has them; add matching stubs returning `Err(FastPadError::Invariant("Scintilla unavailable"))` there):

```rust
    /// Borrows `range` straight out of Scintilla's buffer. The slice stays valid only until the
    /// document is next modified, so callers must finish with it inside the current message.
    pub fn range_bytes(&self, range: Range<usize>) -> Result<&[u8]> {
        let length = range.end.saturating_sub(range.start);
        if length == 0 {
            return Ok(&[]);
        }
        let pointer =
            self.send_direct_checked(SCI_GETRANGEPOINTER, range.start, length as isize)?;
        if pointer == 0 {
            return Err(FastPadError::Invariant("Scintilla returned no range pointer"));
        }
        Ok(unsafe { std::slice::from_raw_parts(pointer as *const u8, length) })
    }

    pub fn line_from_position(&self, position: usize) -> Result<usize> {
        Ok(self.send_direct_checked(SCI_LINEFROMPOSITION, position, 0)?.max(0) as usize)
    }

    pub fn first_visible_line(&self) -> Result<usize> {
        Ok(self.send_direct_checked(SCI_GETFIRSTVISIBLELINE, 0, 0)?.max(0) as usize)
    }

    pub fn set_first_visible_line(&self, display_line: usize) -> Result<()> {
        self.send_direct_checked(SCI_SETFIRSTVISIBLELINE, display_line, 0)
            .map(|_| ())
    }

    pub fn doc_line_from_visible(&self, display_line: usize) -> Result<usize> {
        Ok(self.send_direct_checked(SCI_DOCLINEFROMVISIBLE, display_line, 0)?.max(0) as usize)
    }

    pub fn visible_from_doc_line(&self, doc_line: usize) -> Result<usize> {
        Ok(self.send_direct_checked(SCI_VISIBLEFROMDOCLINE, doc_line, 0)?.max(0) as usize)
    }
```

Add the constants to the file's `scintilla_constants` import list and `use std::ops::Range;` if absent.

Add to `src/editor/scintilla.rs` (top level, `pub`), and re-export it from `src/editor/mod.rs` next to `Editor`:

```rust
/// Scintilla's `SCNotification` (Scintilla.h), complete through `updated` so both `SCN_MODIFIED`
/// and `SCN_UPDATEUI` can be read from one definition.
#[repr(C)]
pub struct ScintillaNotification {
    pub header: windows_sys::Win32::UI::Controls::NMHDR,
    pub position: isize,
    pub ch: i32,
    pub modifiers: i32,
    pub modification_type: i32,
    pub text: *const u8,
    pub length: isize,
    pub lines_added: isize,
    pub message: i32,
    pub wparam: usize,
    pub lparam: isize,
    pub line: isize,
    pub fold_level_now: i32,
    pub fold_level_prev: i32,
    pub margin: i32,
    pub list_type: i32,
    pub x: i32,
    pub y: i32,
    pub token: i32,
    pub annotation_lines_added: isize,
    pub updated: i32,
    pub list_completion_method: i32,
    pub character_source: i32,
}
```

In `src/window/main_window.rs`, delete `struct TextModificationNotification` and change the cast in `handle_editor_notification` to `&*(lparam as *const crate::editor::ScintillaNotification)`; the fields it reads (`modification_type`, `lines_added`) keep their names.

- [ ] **Step 4: Run tests**

Run: `cargo clippy --all-targets` then `cargo test --lib editor::scintilla -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add tools/generate-scintilla-constants.ps1 src/editor
git add src/window/main_window.rs
git commit -m "feat: Scintilla range, visible-line, and notification accessors for the preview"
```

---

### Task 3: Block model

**Files:**
- Create: `src/preview/model.rs`
- Modify: `src/preview/mod.rs` (add `pub mod model;`)

**Interfaces:**
- Produces:
  - `pub struct Block { pub kind: BlockKind, pub bytes: Range<usize>, pub lines: Range<usize> }` (`Clone, Debug, PartialEq, Eq`)
  - `pub enum BlockKind { Heading { level: u8, text: RichText }, Paragraph(RichText), Images(Vec<ImageRef>), List { start: Option<u64>, items: Vec<ListItem> }, Quote(Vec<BlockKind>), Code { language: String, text: String }, Table { alignments: Vec<CellAlign>, head: Vec<RichText>, rows: Vec<Vec<RichText>> }, Rule, Html(String) }`
  - `pub struct ListItem { pub task: Option<bool>, pub blocks: Vec<BlockKind> }`
  - `pub enum CellAlign { None, Left, Center, Right }`
  - `pub struct RichText { pub text: String, pub utf16_len: u32, pub spans: Vec<Span> }`
  - `pub struct Span { pub range: Range<u32>, pub style: InlineStyle }` (UTF-16 offsets into `text`)
  - `pub enum InlineStyle { Strong, Emphasis, Strikethrough, Code, Link(String), ImageAlt }`
  - `pub struct ImageRef { pub dest: String, pub alt: String }`
  - `pub struct RefDef { pub key: String, pub dest: String, pub title: String, pub span: Range<usize> }`
  - `pub fn parse_document(source: &str) -> (Vec<Block>, Vec<RefDef>)`
  - `pub fn parse_blocks(source: &str, base_byte: usize, base_line: usize, refdefs: &[RefDef]) -> Vec<Block>`
  - `pub fn normalize_label(label: &str) -> String`
  - `impl RichText { pub fn plain_text(&self) -> &str }`

- [ ] **Step 1: Write the failing tests**

Create `src/preview/model.rs` containing only the test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<BlockKind> {
        parse_document(source).0.into_iter().map(|block| block.kind).collect()
    }

    fn plain(text: &str) -> RichText {
        RichText {
            text: text.to_owned(),
            utf16_len: text.encode_utf16().count() as u32,
            spans: Vec::new(),
        }
    }

    fn styled(text: &str, spans: Vec<Span>) -> RichText {
        RichText { spans, ..plain(text) }
    }

    fn span(range: Range<u32>, style: InlineStyle) -> Span {
        Span { range, style }
    }

    #[test]
    fn headings_and_paragraphs_carry_source_ranges() {
        let source = "# Title\n\nHello *world*\n";
        let (blocks, _) = parse_document(source);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].kind, BlockKind::Heading { level: 1, text: plain("Title") });
        assert_eq!(source[blocks[0].bytes.clone()].trim_end(), "# Title");
        assert_eq!(blocks[0].lines, 0..1);
        assert_eq!(
            blocks[1].kind,
            BlockKind::Paragraph(styled("Hello world", vec![span(6..11, InlineStyle::Emphasis)]))
        );
        assert_eq!(source[blocks[1].bytes.clone()].trim_end(), "Hello *world*");
        assert_eq!(blocks[1].lines, 2..3);
    }

    #[test]
    fn setext_headings_are_headings() {
        assert_eq!(
            kinds("Title\n=====\n"),
            vec![BlockKind::Heading { level: 1, text: plain("Title") }]
        );
    }

    #[test]
    fn task_lists_nest_and_record_checked_state() {
        let expected = BlockKind::List {
            start: None,
            items: vec![
                ListItem { task: Some(true), blocks: vec![BlockKind::Paragraph(plain("done"))] },
                ListItem {
                    task: Some(false),
                    blocks: vec![
                        BlockKind::Paragraph(plain("todo")),
                        BlockKind::List {
                            start: None,
                            items: vec![ListItem {
                                task: None,
                                blocks: vec![BlockKind::Paragraph(plain("nested"))],
                            }],
                        },
                    ],
                },
            ],
        };
        assert_eq!(kinds("- [x] done\n- [ ] todo\n  - nested\n"), vec![expected]);
    }

    #[test]
    fn ordered_lists_keep_their_start_number() {
        let kinds = kinds("3. a\n4. b\n");
        let [BlockKind::List { start, items }] = kinds.as_slice() else {
            panic!("expected one list, got {kinds:?}");
        };
        assert_eq!(*start, Some(3));
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn quotes_nest() {
        assert_eq!(
            kinds("> quote\n>\n> > inner\n"),
            vec![BlockKind::Quote(vec![
                BlockKind::Paragraph(plain("quote")),
                BlockKind::Quote(vec![BlockKind::Paragraph(plain("inner"))]),
            ])]
        );
    }

    #[test]
    fn fenced_and_indented_code_blocks() {
        assert_eq!(
            kinds("```rust\nfn main() {}\n```\n"),
            vec![BlockKind::Code { language: "rust".into(), text: "fn main() {}".into() }]
        );
        assert_eq!(
            kinds("    x = 1\n"),
            vec![BlockKind::Code { language: String::new(), text: "x = 1".into() }]
        );
    }

    #[test]
    fn tables_keep_alignment_head_and_styled_cells() {
        assert_eq!(
            kinds("| a | b |\n|:--|--:|\n| 1 | **2** |\n"),
            vec![BlockKind::Table {
                alignments: vec![CellAlign::Left, CellAlign::Right],
                head: vec![plain("a"), plain("b")],
                rows: vec![vec![plain("1"), styled("2", vec![span(0..1, InlineStyle::Strong)])]],
            }]
        );
    }

    #[test]
    fn strikethrough_links_and_inline_code_become_spans() {
        assert_eq!(
            kinds("~~old~~ [site](https://x.dev) `code`\n"),
            vec![BlockKind::Paragraph(styled(
                "old site code",
                vec![
                    span(0..3, InlineStyle::Strikethrough),
                    span(4..8, InlineStyle::Link("https://x.dev".into())),
                    span(9..13, InlineStyle::Code),
                ],
            ))]
        );
    }

    #[test]
    fn image_only_paragraphs_become_image_blocks() {
        assert_eq!(
            kinds("![logo](img/logo.png)\n"),
            vec![BlockKind::Images(vec![ImageRef {
                dest: "img/logo.png".into(),
                alt: "logo".into()
            }])]
        );
    }

    #[test]
    fn images_inside_text_render_as_alt_text() {
        assert_eq!(
            kinds("See ![logo](a.png) here\n"),
            vec![BlockKind::Paragraph(styled(
                "See logo here",
                vec![span(4..8, InlineStyle::ImageAlt)]
            ))]
        );
    }

    #[test]
    fn rules_and_raw_html() {
        assert_eq!(
            kinds("---\n\n<div>hi</div>\n"),
            vec![BlockKind::Rule, BlockKind::Html("<div>hi</div>".into())]
        );
    }

    #[test]
    fn span_offsets_are_utf16() {
        assert_eq!(
            kinds("**é😀**\n"),
            vec![BlockKind::Paragraph(styled("é😀", vec![span(0..3, InlineStyle::Strong)]))]
        );
    }

    #[test]
    fn slices_are_offset_by_their_base_position() {
        let blocks = parse_blocks("para\n", 100, 7, &[]);
        assert_eq!(blocks[0].bytes.start, 100);
        assert_eq!(blocks[0].lines, 7..8);
    }

    #[test]
    fn reference_definitions_are_collected_and_resolve_in_slices() {
        let (blocks, refdefs) = parse_document("[site]\n\n[Site]: https://x.dev\n");
        assert_eq!(refdefs.len(), 1);
        assert_eq!(refdefs[0].key, "site");
        assert_eq!(refdefs[0].dest, "https://x.dev");
        let link = BlockKind::Paragraph(styled(
            "site",
            vec![span(0..4, InlineStyle::Link("https://x.dev".into()))],
        ));
        assert_eq!(blocks[0].kind, link);
        assert_eq!(parse_blocks("[site]\n", 0, 0, &refdefs)[0].kind, link);
    }

    #[test]
    fn footnote_syntax_stays_literal() {
        assert_eq!(kinds("a[^1]\n"), vec![BlockKind::Paragraph(plain("a[^1]"))]);
    }
}
```

Add `pub mod model;` to `src/preview/mod.rs`.

Run: `cargo test --lib preview::model`
Expected: FAIL to compile (types not defined).

- [ ] **Step 2: Implement the model**

Put this above the test module in `src/preview/model.rs`:

```rust
//! Markdown source to top-level preview blocks. Pure Rust with no Win32, so every rule here is
//! unit-tested. Inline styles are recorded as UTF-16 ranges because DirectWrite addresses text in
//! UTF-16 code units.

use pulldown_cmark::{
    Alignment, BrokenLink, CodeBlockKind, CowStr, Event, Options, Parser, Tag, TagEnd,
};
use std::ops::Range;

/// GitHub-flavored extensions the preview renders. Footnotes, math, and metadata stay literal.
pub const PARSE_OPTIONS: Options = Options::ENABLE_TABLES
    .union(Options::ENABLE_STRIKETHROUGH)
    .union(Options::ENABLE_TASKLISTS);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    /// Source byte range, including the block's trailing newline when the parser includes it.
    pub bytes: Range<usize>,
    /// Zero-based source lines the block spans, end exclusive.
    pub lines: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockKind {
    Heading { level: u8, text: RichText },
    Paragraph(RichText),
    Images(Vec<ImageRef>),
    List { start: Option<u64>, items: Vec<ListItem> },
    Quote(Vec<BlockKind>),
    Code { language: String, text: String },
    Table { alignments: Vec<CellAlign>, head: Vec<RichText>, rows: Vec<Vec<RichText>> },
    Rule,
    Html(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListItem {
    pub task: Option<bool>,
    pub blocks: Vec<BlockKind>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellAlign {
    None,
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RichText {
    pub text: String,
    pub utf16_len: u32,
    pub spans: Vec<Span>,
}

impl RichText {
    pub fn plain_text(&self) -> &str {
        &self.text
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub range: Range<u32>,
    pub style: InlineStyle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InlineStyle {
    Strong,
    Emphasis,
    Strikethrough,
    Code,
    Link(String),
    ImageAlt,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageRef {
    pub dest: String,
    pub alt: String,
}

/// A link reference definition (`[label]: dest "title"`), kept so slices parsed on their own still
/// resolve references defined elsewhere in the document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefDef {
    pub key: String,
    pub dest: String,
    pub title: String,
    pub span: Range<usize>,
}

/// Case-folds and collapses whitespace the way reference labels are matched.
pub fn normalize_label(label: &str) -> String {
    label
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

pub fn parse_document(source: &str) -> (Vec<Block>, Vec<RefDef>) {
    let mut events = Parser::new_ext(source, PARSE_OPTIONS).into_offset_iter();
    let refdefs = events
        .reference_definitions()
        .iter()
        .map(|(label, definition)| RefDef {
            key: normalize_label(label),
            dest: definition.dest.to_string(),
            title: definition.title.as_deref().unwrap_or_default().to_owned(),
            span: definition.span.clone(),
        })
        .collect();
    let blocks = collect_blocks(&mut events, source, 0, 0);
    (blocks, refdefs)
}

pub fn parse_blocks(
    source: &str,
    base_byte: usize,
    base_line: usize,
    refdefs: &[RefDef],
) -> Vec<Block> {
    let resolve = |link: BrokenLink<'_>| {
        let key = normalize_label(&link.reference);
        refdefs.iter().find(|definition| definition.key == key).map(|definition| {
            (
                CowStr::from(definition.dest.clone()),
                CowStr::from(definition.title.clone()),
            )
        })
    };
    let mut events =
        Parser::new_with_broken_link_callback(source, PARSE_OPTIONS, Some(resolve))
            .into_offset_iter();
    collect_blocks(&mut events, source, base_byte, base_line)
}

fn collect_blocks<'a>(
    events: impl Iterator<Item = (Event<'a>, Range<usize>)>,
    source: &str,
    base_byte: usize,
    base_line: usize,
) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut builder = Builder::default();
    let mut lines = LineCounter::new(source);
    let mut top: Option<Range<usize>> = None;
    for (event, range) in events {
        if builder.stack.is_empty() && top.is_none() {
            top = Some(range.clone());
        }
        builder.event(event);
        if let Some(kind) = builder.finished.take() {
            let span = top.take().unwrap_or(range.clone());
            let bytes = span.start..span.end.max(range.end);
            let first_line = lines.line_of(bytes.start);
            let last_line = lines.line_of(bytes.end.saturating_sub(1).max(bytes.start));
            blocks.push(Block {
                kind,
                bytes: base_byte + bytes.start..base_byte + bytes.end,
                lines: base_line + first_line..base_line + last_line + 1,
            });
        }
    }
    blocks
}

/// Counts newlines forward from the last query; top-level block offsets only grow.
struct LineCounter<'a> {
    bytes: &'a [u8],
    position: usize,
    line: usize,
}

impl<'a> LineCounter<'a> {
    fn new(source: &'a str) -> Self {
        Self { bytes: source.as_bytes(), position: 0, line: 0 }
    }

    fn line_of(&mut self, offset: usize) -> usize {
        let offset = offset.min(self.bytes.len());
        if offset < self.position {
            self.position = 0;
            self.line = 0;
        }
        self.line += self.bytes[self.position..offset]
            .iter()
            .filter(|byte| **byte == b'\n')
            .count();
        self.position = offset;
        self.line
    }
}

#[derive(Default)]
struct TextBuilder {
    text: String,
    utf16_len: u32,
    spans: Vec<Span>,
    open: Vec<(InlineStyle, u32)>,
    images: Vec<ImageRef>,
    image_stack: Vec<ImageRef>,
    has_text: bool,
}

impl TextBuilder {
    fn push(&mut self, value: &str, counts_as_text: bool) {
        if let Some(image) = self.image_stack.last_mut() {
            image.alt.push_str(value);
            return;
        }
        self.text.push_str(value);
        self.utf16_len += value.encode_utf16().count() as u32;
        self.has_text |= counts_as_text && !value.trim().is_empty();
    }

    fn open(&mut self, style: InlineStyle) {
        self.open.push((style, self.utf16_len));
    }

    fn close(&mut self) {
        if let Some((style, start)) = self.open.pop()
            && start < self.utf16_len
        {
            self.spans.push(Span { range: start..self.utf16_len, style });
        }
    }

    fn finish_image(&mut self) {
        let Some(image) = self.image_stack.pop() else {
            return;
        };
        let start = self.utf16_len;
        let alt = if image.alt.is_empty() { "image".to_owned() } else { image.alt.clone() };
        self.push(&alt, false);
        self.spans.push(Span { range: start..self.utf16_len, style: InlineStyle::ImageAlt });
        self.images.push(image);
    }

    fn into_rich_text(self) -> RichText {
        RichText { text: self.text, utf16_len: self.utf16_len, spans: self.spans }
    }

    /// A paragraph of only images (and whitespace) becomes an image block.
    fn into_paragraph(self) -> BlockKind {
        if !self.has_text && !self.images.is_empty() {
            BlockKind::Images(self.images)
        } else {
            BlockKind::Paragraph(self.into_rich_text())
        }
    }
}

enum Frame {
    Heading { level: u8, text: TextBuilder },
    Paragraph(TextBuilder),
    List { start: Option<u64>, items: Vec<ListItem> },
    Item { task: Option<bool>, blocks: Vec<BlockKind>, loose: Option<TextBuilder> },
    Quote(Vec<BlockKind>),
    Code { language: String, text: String },
    Html(String),
    Table { alignments: Vec<CellAlign>, head: Vec<RichText>, rows: Vec<Vec<RichText>> },
    Row(Vec<RichText>),
    Cell(TextBuilder),
}

#[derive(Default)]
struct Builder {
    stack: Vec<Frame>,
    finished: Option<BlockKind>,
}

impl Builder {
    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(&text, true),
            Event::Code(code) => {
                if let Some(target) = self.inline_target() {
                    target.open(InlineStyle::Code);
                    target.push(&code, true);
                    target.close();
                }
            }
            Event::Html(html) => {
                if let Some(Frame::Html(buffer)) = self.stack.last_mut() {
                    buffer.push_str(&html);
                } else {
                    self.text(&html, true);
                }
            }
            Event::InlineHtml(html)
            | Event::InlineMath(html)
            | Event::DisplayMath(html)
            | Event::FootnoteReference(html) => self.text(&html, true),
            Event::SoftBreak => self.text(" ", false),
            Event::HardBreak => self.text("\n", false),
            Event::Rule => self.add_block(BlockKind::Rule),
            Event::TaskListMarker(checked) => {
                if let Some(Frame::Item { task, .. }) = self.stack.last_mut() {
                    *task = Some(checked);
                }
            }
        }
    }

    fn text(&mut self, value: &str, counts_as_text: bool) {
        match self.stack.last_mut() {
            Some(Frame::Code { text, .. }) => text.push_str(value),
            Some(Frame::Html(buffer)) => buffer.push_str(value),
            _ => {
                if let Some(target) = self.inline_target() {
                    target.push(value, counts_as_text);
                }
            }
        }
    }

    fn inline_target(&mut self) -> Option<&mut TextBuilder> {
        match self.stack.last_mut()? {
            Frame::Heading { text, .. } | Frame::Paragraph(text) | Frame::Cell(text) => Some(text),
            Frame::Item { loose, .. } => Some(loose.get_or_insert_with(TextBuilder::default)),
            _ => None,
        }
    }

    /// Tight list items hold text directly; a nested block ends that implicit paragraph.
    fn flush_loose_item_text(&mut self) {
        if let Some(Frame::Item { blocks, loose, .. }) = self.stack.last_mut()
            && let Some(text) = loose.take()
        {
            blocks.push(text.into_paragraph());
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Emphasis => self.open_style(InlineStyle::Emphasis),
            Tag::Strong => self.open_style(InlineStyle::Strong),
            Tag::Strikethrough => self.open_style(InlineStyle::Strikethrough),
            Tag::Link { dest_url, .. } => self.open_style(InlineStyle::Link(dest_url.to_string())),
            Tag::Image { dest_url, .. } => {
                if let Some(target) = self.inline_target() {
                    target.image_stack.push(ImageRef { dest: dest_url.to_string(), alt: String::new() });
                }
            }
            Tag::Paragraph => self.push_block_frame(Frame::Paragraph(TextBuilder::default())),
            Tag::Heading { level, .. } => self.push_block_frame(Frame::Heading {
                level: level as u8,
                text: TextBuilder::default(),
            }),
            Tag::BlockQuote(_) => self.push_block_frame(Frame::Quote(Vec::new())),
            Tag::CodeBlock(kind) => {
                let language = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().unwrap_or_default().to_owned()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                self.push_block_frame(Frame::Code { language, text: String::new() });
            }
            Tag::HtmlBlock => self.push_block_frame(Frame::Html(String::new())),
            Tag::List(start) => self.push_block_frame(Frame::List { start, items: Vec::new() }),
            Tag::Item => self.stack.push(Frame::Item { task: None, blocks: Vec::new(), loose: None }),
            Tag::Table(alignments) => self.push_block_frame(Frame::Table {
                alignments: alignments.into_iter().map(cell_align).collect(),
                head: Vec::new(),
                rows: Vec::new(),
            }),
            Tag::TableHead | Tag::TableRow => self.stack.push(Frame::Row(Vec::new())),
            Tag::TableCell => self.stack.push(Frame::Cell(TextBuilder::default())),
            // Not enabled by PARSE_OPTIONS; their content flows into the enclosing block.
            _ => {}
        }
    }

    fn open_style(&mut self, style: InlineStyle) {
        if let Some(target) = self.inline_target() {
            target.open(style);
        }
    }

    fn push_block_frame(&mut self, frame: Frame) {
        self.flush_loose_item_text();
        self.stack.push(frame);
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link => {
                if let Some(target) = self.inline_target() {
                    target.close();
                }
            }
            TagEnd::Image => {
                if let Some(target) = self.inline_target() {
                    target.finish_image();
                }
            }
            TagEnd::Paragraph
            | TagEnd::Heading(_)
            | TagEnd::BlockQuote(_)
            | TagEnd::CodeBlock
            | TagEnd::HtmlBlock
            | TagEnd::List(_)
            | TagEnd::Table => {
                if let Some(frame) = self.stack.pop() {
                    let kind = finish_block(frame);
                    self.add_block(kind);
                }
            }
            TagEnd::Item => {
                self.flush_loose_item_text();
                if let Some(Frame::Item { task, blocks, .. }) = self.stack.pop()
                    && let Some(Frame::List { items, .. }) = self.stack.last_mut()
                {
                    items.push(ListItem { task, blocks });
                }
            }
            TagEnd::TableHead => {
                if let Some(Frame::Row(cells)) = self.stack.pop()
                    && let Some(Frame::Table { head, .. }) = self.stack.last_mut()
                {
                    *head = cells;
                }
            }
            TagEnd::TableRow => {
                if let Some(Frame::Row(cells)) = self.stack.pop()
                    && let Some(Frame::Table { rows, .. }) = self.stack.last_mut()
                {
                    rows.push(cells);
                }
            }
            TagEnd::TableCell => {
                if let Some(Frame::Cell(text)) = self.stack.pop()
                    && let Some(Frame::Row(cells)) = self.stack.last_mut()
                {
                    cells.push(text.into_rich_text());
                }
            }
            _ => {}
        }
    }

    fn add_block(&mut self, kind: BlockKind) {
        self.flush_loose_item_text();
        match self.stack.last_mut() {
            None => self.finished = Some(kind),
            Some(Frame::Item { blocks, .. }) | Some(Frame::Quote(blocks)) => blocks.push(kind),
            // A block can only close into a container; anything else is a parser invariant break.
            Some(_) => {}
        }
    }
}

fn finish_block(frame: Frame) -> BlockKind {
    match frame {
        Frame::Heading { level, text } => BlockKind::Heading { level, text: text.into_rich_text() },
        Frame::Paragraph(text) => text.into_paragraph(),
        Frame::List { start, items } => BlockKind::List { start, items },
        Frame::Quote(blocks) => BlockKind::Quote(blocks),
        Frame::Code { language, text } => BlockKind::Code {
            language,
            text: text.strip_suffix('\n').unwrap_or(&text).to_owned(),
        },
        Frame::Html(html) => BlockKind::Html(html.trim_end().to_owned()),
        Frame::Table { alignments, head, rows } => BlockKind::Table { alignments, head, rows },
        // Items, rows, and cells close through their own end tags and never reach this function;
        // these arms only keep the match exhaustive without a panic in an abort-on-panic build.
        Frame::Item { blocks, .. } => BlockKind::Quote(blocks),
        Frame::Row(_) | Frame::Cell(_) => BlockKind::Rule,
    }
}

fn cell_align(alignment: Alignment) -> CellAlign {
    match alignment {
        Alignment::None => CellAlign::None,
        Alignment::Left => CellAlign::Left,
        Alignment::Center => CellAlign::Center,
        Alignment::Right => CellAlign::Right,
    }
}
```

Note on `into_offset_iter`: `reference_definitions()` is available on the offset iterator before iteration because references are resolved in the parser's first pass.

- [ ] **Step 3: Run tests**

Run: `cargo test --lib preview::model`
Expected: PASS. If a range assertion fails only because the parser excludes a trailing newline, keep the `trim_end` form of the assertion and do not change the implementation's range handling.

- [ ] **Step 4: Clippy and commit**

Run: `cargo clippy --all-targets`

```bash
git add src/preview/mod.rs src/preview/model.rs
git commit -m "feat: Markdown block model for the preview"
```

---

### Task 4: Incremental reparse

**Files:**
- Create: `src/preview/incremental.rs`
- Modify: `src/preview/mod.rs` (add `pub mod incremental;`)

**Interfaces:**
- Consumes: `model::{Block, RefDef, parse_document, parse_blocks}` (Task 3).
- Produces:
  - `pub struct Edit { pub position: usize, pub removed: usize, pub inserted: usize, pub lines_delta: isize }` (`Clone, Copy, Debug, PartialEq, Eq`)
  - `pub const MAX_PENDING_EDITS: usize = 64;`
  - `pub struct EditLog` with `record(&mut self, edit: Edit)`, `request_full(&mut self)`, `is_empty(&self) -> bool`, `take(&mut self) -> Pending`, `restore(&mut self, pending: Pending)`
  - `pub enum Pending { Nothing, Full, Edits(Vec<Edit>) }`
  - `pub trait SourceText { fn len(&self) -> usize; fn slice(&self, range: Range<usize>) -> Cow<'_, str>; fn line_of(&self, byte: usize) -> usize; }` implemented for `str`
  - `pub struct PreviewDocument { pub blocks: Vec<Block>, pub revision: u64, refdefs: Vec<RefDef> }`
  - `impl PreviewDocument { pub fn parse(source: &str) -> Self; pub fn apply(&mut self, source: &(impl SourceText + ?Sized), edits: &[Edit]) -> Update; pub fn try_apply(&mut self, source: &(impl SourceText + ?Sized), edits: &[Edit]) -> Option<Update>; pub fn reparse(&mut self, source: &(impl SourceText + ?Sized)) -> Update }` — `try_apply` returns `None` instead of parsing the whole document (the caller then reparses on a worker); after `None` the blocks are stale until a full parse replaces them.
  - `pub enum Update { Unchanged, Replaced { old: Range<usize>, new: Range<usize> }, Full }`

- [ ] **Step 1: Write the failing tests**

Create `src/preview/incremental.rs` with the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::model::parse_document;

    const FIXTURES: [&str; 6] = [
        "# Title\n\nFirst paragraph with *emphasis*.\n\nSecond paragraph.\n",
        "- one\n- two\n\n- three after a blank\n\n  continued item\n\nTail paragraph.\n",
        "> quote line\n> more\n\nlazy\n\n```rust\nfn main() {}\n\nlet x = 1;\n```\n\nafter code\n",
        "| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n\ntext\n\n    indented code\n\n    more code\n",
        "See [site] and [other][o].\n\n[site]: https://x.dev\n[o]: https://o.dev\n\nEnd.\n",
        "<!-- comment\n\nstill comment -->\n\nParagraph\n\n<div>\nhtml\n</div>\n",
    ];

    const INSERTS: [&str; 18] = [
        "x", "\n", "\n\n", "```", "~~~", "- ", "1. ", "> ", "    ", "|", "---", "[a]: /u\n",
        "<!--", "-->", "**", "`", " ", "===\n",
    ];

    /// xorshift64*: deterministic and dependency-free.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }

        fn below(&mut self, bound: usize) -> usize {
            (self.next() % bound.max(1) as u64) as usize
        }
    }

    fn boundary(text: &str, mut position: usize) -> usize {
        position = position.min(text.len());
        while !text.is_char_boundary(position) {
            position -= 1;
        }
        position
    }

    fn random_edit(rng: &mut Rng, text: &mut String) -> Edit {
        let position = boundary(text, rng.below(text.len() + 1));
        if rng.below(3) == 0 && position < text.len() {
            let end = boundary(text, position + 1 + rng.below(8));
            let end = if end == position { boundary(text, text.len()) } else { end };
            let removed_text = text[position..end].to_owned();
            text.replace_range(position..end, "");
            Edit {
                position,
                removed: removed_text.len(),
                inserted: 0,
                lines_delta: -(removed_text.matches('\n').count() as isize),
            }
        } else {
            let insert = INSERTS[rng.below(INSERTS.len())];
            text.insert_str(position, insert);
            Edit {
                position,
                removed: 0,
                inserted: insert.len(),
                lines_delta: insert.matches('\n').count() as isize,
            }
        }
    }

    #[test]
    fn incremental_updates_always_equal_a_full_parse() {
        for (fixture_index, fixture) in FIXTURES.iter().enumerate() {
            for seed in 1..=40_u64 {
                let mut rng = Rng(seed * 7919 + fixture_index as u64);
                let mut text = (*fixture).to_owned();
                let mut document = PreviewDocument::parse(&text);
                for step in 0..60 {
                    let batch = 1 + rng.below(4);
                    let edits = (0..batch)
                        .map(|_| random_edit(&mut rng, &mut text))
                        .collect::<Vec<_>>();
                    document.apply(text.as_str(), &edits);
                    let (expected, _) = parse_document(&text);
                    assert_eq!(
                        document.blocks, expected,
                        "fixture {fixture_index}, seed {seed}, step {step}, text {text:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_one_paragraph_edit_replaces_only_nearby_blocks() {
        let mut text = String::from("a\n\nb\n\nc\n\nd\n\ne\n\nf\n\ng\n");
        let mut document = PreviewDocument::parse(&text);
        // Byte 12 is the start of paragraph "e", the fifth of seven blocks.
        text.insert(12, 'x');
        let update = document.apply(
            text.as_str(),
            &[Edit { position: 12, removed: 0, inserted: 1, lines_delta: 0 }],
        );
        let Update::Replaced { old, new } = update else {
            panic!("expected a partial update, got {update:?}");
        };
        assert!(old.len() <= 5 && new.len() <= 5, "{old:?} {new:?}");
        assert_eq!(document.blocks, parse_document(&text).0);
    }

    #[test]
    fn slices_containing_reference_definitions_force_a_full_parse() {
        let mut text = String::from("[a]\n\n[a]: /one\n");
        let mut document = PreviewDocument::parse(&text);
        text.replace_range(10..13, "two");
        let update = document.apply(
            text.as_str(),
            &[Edit { position: 10, removed: 3, inserted: 3, lines_delta: 0 }],
        );
        assert_eq!(update, Update::Full);
        assert_eq!(document.blocks, parse_document(&text).0);
    }

    #[test]
    fn the_edit_log_caps_pending_edits_and_then_requests_a_full_parse() {
        let mut log = EditLog::default();
        let edit = Edit { position: 0, removed: 0, inserted: 1, lines_delta: 0 };
        for _ in 0..MAX_PENDING_EDITS {
            log.record(edit);
        }
        assert!(matches!(log.take(), Pending::Edits(edits) if edits.len() == MAX_PENDING_EDITS));
        for _ in 0..=MAX_PENDING_EDITS {
            log.record(edit);
        }
        assert_eq!(log.take(), Pending::Full);
        assert!(log.is_empty());
    }

    #[test]
    fn restored_pending_edits_are_not_lost() {
        let mut log = EditLog::default();
        let edit = Edit { position: 3, removed: 1, inserted: 0, lines_delta: 0 };
        log.record(edit);
        let pending = log.take();
        log.restore(pending);
        assert_eq!(log.take(), Pending::Edits(vec![edit]));
    }

    #[test]
    fn try_apply_declines_instead_of_parsing_everything() {
        let mut text = String::from("[a]\n\n[a]: /one\n");
        let mut document = PreviewDocument::parse(&text);
        text.replace_range(10..13, "two");
        let edit = Edit { position: 10, removed: 3, inserted: 3, lines_delta: 0 };
        assert_eq!(document.try_apply(text.as_str(), &[edit]), None);
        assert_eq!(document.try_apply(text.as_str(), &[]), Some(Update::Unchanged));
    }

    #[test]
    fn revisions_increase_on_every_change() {
        let mut text = String::from("a\n");
        let mut document = PreviewDocument::parse(&text);
        let first = document.revision;
        text.push('b');
        document.apply(text.as_str(), &[Edit { position: 2, removed: 0, inserted: 1, lines_delta: 0 }]);
        assert!(document.revision > first);
    }
}
```

Add `pub mod incremental;` to `src/preview/mod.rs`.

Run: `cargo test --lib preview::incremental`
Expected: FAIL to compile.

- [ ] **Step 2: Implement**

Above the tests in `src/preview/incremental.rs`:

```rust
//! Keeps a `PreviewDocument` in sync with edits by reparsing only the blocks around them. A slice
//! is accepted only when the unchanged sentinel blocks at both ends parse back identically, which
//! proves the parser state at the slice boundaries did not change; otherwise the slice widens and
//! finally falls back to a full parse. The property test in this file is the authority: a
//! divergence means a missing fallback rule, never a rendering special case.

use crate::preview::model::{Block, RefDef, parse_blocks, parse_document};
use std::borrow::Cow;
use std::ops::Range;

pub const MAX_PENDING_EDITS: usize = 64;
const SENTINELS: usize = 2;
const MAX_WIDENINGS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edit {
    pub position: usize,
    pub removed: usize,
    pub inserted: usize,
    pub lines_delta: isize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Pending {
    Nothing,
    Full,
    Edits(Vec<Edit>),
}

#[derive(Debug, Default)]
pub struct EditLog {
    edits: Vec<Edit>,
    full: bool,
}

impl EditLog {
    pub fn record(&mut self, edit: Edit) {
        if self.full {
            return;
        }
        if self.edits.len() == MAX_PENDING_EDITS {
            self.request_full();
        } else {
            self.edits.push(edit);
        }
    }

    pub fn request_full(&mut self) {
        self.full = true;
        self.edits.clear();
    }

    pub fn is_empty(&self) -> bool {
        !self.full && self.edits.is_empty()
    }

    pub fn take(&mut self) -> Pending {
        if std::mem::take(&mut self.full) {
            self.edits.clear();
            Pending::Full
        } else if self.edits.is_empty() {
            Pending::Nothing
        } else {
            Pending::Edits(std::mem::take(&mut self.edits))
        }
    }

    /// Puts back work taken by `take` that was not processed; later edits stay after it.
    pub fn restore(&mut self, pending: Pending) {
        match pending {
            Pending::Nothing => {}
            Pending::Full => self.request_full(),
            Pending::Edits(mut edits) => {
                if self.full {
                    return;
                }
                edits.append(&mut self.edits);
                if edits.len() > MAX_PENDING_EDITS {
                    self.request_full();
                } else {
                    self.edits = edits;
                }
            }
        }
    }
}

/// Read access to the current document text; implemented over Scintilla's buffer by the host.
pub trait SourceText {
    fn len(&self) -> usize;
    fn slice(&self, range: Range<usize>) -> Cow<'_, str>;
    fn line_of(&self, byte: usize) -> usize;
}

impl SourceText for str {
    fn len(&self) -> usize {
        str::len(self)
    }

    fn slice(&self, range: Range<usize>) -> Cow<'_, str> {
        Cow::Borrowed(&self[range])
    }

    fn line_of(&self, byte: usize) -> usize {
        self.as_bytes()[..byte.min(str::len(self))]
            .iter()
            .filter(|value| **value == b'\n')
            .count()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Update {
    Unchanged,
    /// Blocks `old` (pre-update indices) were replaced by blocks `new` (post-update indices).
    Replaced { old: Range<usize>, new: Range<usize> },
    Full,
}

#[derive(Debug, Default)]
pub struct PreviewDocument {
    pub blocks: Vec<Block>,
    pub revision: u64,
    refdefs: Vec<RefDef>,
}

impl PreviewDocument {
    pub fn parse(source: &str) -> Self {
        let (blocks, refdefs) = parse_document(source);
        Self { blocks, revision: 1, refdefs }
    }

    pub fn reparse(&mut self, source: &(impl SourceText + ?Sized)) -> Update {
        let text = source.slice(0..source.len());
        let (blocks, refdefs) = parse_document(&text);
        self.blocks = blocks;
        self.refdefs = refdefs;
        self.revision += 1;
        Update::Full
    }

    pub fn apply(&mut self, source: &(impl SourceText + ?Sized), edits: &[Edit]) -> Update {
        match self.try_apply(source, edits) {
            Some(update) => update,
            None => self.reparse(source),
        }
    }

    /// Like `apply`, but returns `None` where `apply` would parse the whole document.
    pub fn try_apply(&mut self, source: &(impl SourceText + ?Sized), edits: &[Edit]) -> Option<Update> {
        if edits.is_empty() {
            return Some(Update::Unchanged);
        }
        let dirty = self.shift_for_edits(edits)?;
        let count = self.blocks.len();
        let first = self.blocks.partition_point(|block| block.bytes.end < dirty.start);
        let last = self.blocks.partition_point(|block| block.bytes.start <= dirty.end);
        let mut low = first.saturating_sub(SENTINELS);
        let mut high = (last.max(first) + SENTINELS).min(count);
        for _ in 0..=MAX_WIDENINGS {
            if low == 0 && high == count {
                break;
            }
            let start = if low == 0 { 0 } else { self.blocks[low].bytes.start };
            let end = if high == count { source.len() } else { self.blocks[high - 1].bytes.end };
            let text = source.slice(start..end);
            if text.contains("]:") {
                return None;
            }
            let parsed = parse_blocks(&text, start, source.line_of(start), &self.refdefs);
            let start_ok = low == 0 || parsed.first() == Some(&self.blocks[low]);
            let end_ok = high == count || parsed.last() == Some(&self.blocks[high - 1]);
            if start_ok && end_ok {
                let new = low..low + parsed.len();
                self.blocks.splice(low..high, parsed);
                self.revision += 1;
                return Some(Update::Replaced { old: low..high, new });
            }
            if !start_ok {
                low = low.saturating_sub(1);
            }
            if !end_ok {
                high = (high + 1).min(count);
            }
        }
        None
    }

    /// Moves block and definition ranges through `edits` in order and returns the edited byte
    /// range in final coordinates, or `None` when an edit touches a reference definition (which
    /// needs a full parse).
    fn shift_for_edits(&mut self, edits: &[Edit]) -> Option<Range<usize>> {
        let mut dirty: Option<Range<usize>> = None;
        for edit in edits {
            let removed_end = edit.position + edit.removed;
            if self
                .refdefs
                .iter()
                .any(|definition| definition.span.start <= removed_end && edit.position <= definition.span.end)
            {
                return None;
            }
            for block in &mut self.blocks {
                block.bytes = shift_range(block.bytes.clone(), edit);
                if block.bytes.start >= edit.position + edit.inserted {
                    block.lines = shift_lines(block.lines.clone(), edit.lines_delta);
                }
            }
            for definition in &mut self.refdefs {
                definition.span = shift_range(definition.span.clone(), edit);
            }
            let touched = edit.position..edit.position + edit.inserted;
            dirty = Some(match dirty {
                None => touched,
                Some(previous) => {
                    let previous = shift_range(previous, edit);
                    previous.start.min(touched.start)..previous.end.max(touched.end)
                }
            });
        }
        dirty
    }
}

fn shift_position(position: usize, edit: &Edit, is_end: bool) -> usize {
    let removed_end = edit.position + edit.removed;
    if position < edit.position || (position == edit.position && !is_end) {
        position
    } else if position >= removed_end {
        position - edit.removed + edit.inserted
    } else {
        edit.position + edit.inserted
    }
}

fn shift_range(range: Range<usize>, edit: &Edit) -> Range<usize> {
    let start = shift_position(range.start, edit, false);
    let end = shift_position(range.end, edit, true).max(start);
    start..end
}

fn shift_lines(lines: Range<usize>, delta: isize) -> Range<usize> {
    let move_line = |line: usize| (line as isize + delta).max(0) as usize;
    move_line(lines.start)..move_line(lines.end)
}
```

Line shifting rule: blocks that start at or after the end of the inserted text move by `lines_delta`; blocks that overlap the edit sit inside the dirty span and are replaced by the reparse, so their stale lines never survive. Sentinel comparison uses full `Block` equality, so a sentinel with a wrong line range fails comparison and widens the slice rather than producing a wrong result.

`shift_lines` clamps at zero, so a large negative `lines_delta` cannot underflow.

- [ ] **Step 3: Run tests**

Run: `cargo test --lib preview::incremental`
Expected: PASS. If `incremental_updates_always_equal_a_full_parse` fails, read the printed text, identify which boundary assumption broke, and add a conservative fallback (widening or full parse) in `apply`; re-run until it passes. Do not reduce the fixture set, seeds, or insert pool.

- [ ] **Step 4: Clippy and commit**

Run: `cargo clippy --all-targets`

```bash
git add src/preview/mod.rs src/preview/incremental.rs
git commit -m "feat: incremental preview reparse verified against full parses"
```

---
### Task 5: Link classification and heading slugs

**Files:**
- Create: `src/preview/links.rs`
- Modify: `src/preview/mod.rs` (add `pub mod links;`)

**Interfaces:**
- Produces:
  - `pub enum LinkAction { External(String), Anchor(String), LocalFile(PathBuf), Ignored }` (`Clone, Debug, PartialEq, Eq`)
  - `pub fn classify_link(dest: &str, document_dir: Option<&Path>) -> LinkAction`
  - `pub fn resolve_image_path(dest: &str, document_dir: Option<&Path>) -> Option<PathBuf>`
  - `pub fn slug(text: &str) -> String`
  - `#[derive(Default)] pub struct SlugSet` with `pub fn unique(&mut self, text: &str) -> String`

- [ ] **Step 1: Write the failing tests**

Create `src/preview/links.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    struct ScratchDir(PathBuf);

    impl ScratchDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("fastpad-links-{name}-{}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn web_and_mail_links_open_externally() {
        for dest in ["https://x.dev/a", "HTTP://X.DEV", "mailto:me@x.dev"] {
            assert_eq!(classify_link(dest, None), LinkAction::External(dest.to_owned()));
        }
    }

    #[test]
    fn fragment_links_are_anchors() {
        assert_eq!(
            classify_link("#Getting-Started", None),
            LinkAction::Anchor("getting-started".into())
        );
    }

    #[test]
    fn existing_local_files_open_in_fastpad() {
        let dir = ScratchDir::new("local");
        std::fs::write(dir.0.join("notes.md"), "x").unwrap();
        let expected = LinkAction::LocalFile(dir.0.join("notes.md"));
        assert_eq!(classify_link("notes.md", Some(&dir.0)), expected);
        assert_eq!(classify_link("./notes.md#part", Some(&dir.0)), LinkAction::LocalFile(dir.0.join(".\\notes.md")));
        assert_eq!(classify_link("missing.md", Some(&dir.0)), LinkAction::Ignored);
        assert_eq!(classify_link("notes.md", None), LinkAction::Ignored);
        let absolute = dir.0.join("notes.md");
        assert_eq!(
            classify_link(absolute.to_str().unwrap(), None),
            LinkAction::LocalFile(absolute)
        );
    }

    #[test]
    fn other_schemes_are_ignored() {
        for dest in ["ftp://x.dev", "javascript:alert(1)", "file:///C:/x", "//host/share", ""] {
            assert_eq!(classify_link(dest, None), LinkAction::Ignored, "{dest}");
        }
    }

    #[test]
    fn image_paths_resolve_locally_and_never_remotely() {
        let dir = PathBuf::from(r"C:\docs");
        assert_eq!(
            resolve_image_path("img/a%20b.png", Some(&dir)),
            Some(PathBuf::from(r"C:\docs\img\a b.png"))
        );
        assert_eq!(resolve_image_path("https://x.dev/a.png", Some(&dir)), None);
        assert_eq!(resolve_image_path("data:image/png;base64,AAAA", Some(&dir)), None);
        assert_eq!(resolve_image_path("a.png", None), None);
    }

    #[test]
    fn slugs_follow_github_rules() {
        assert_eq!(slug("Getting Started!"), "getting-started");
        assert_eq!(slug("C++ & Rust"), "c--rust");
        assert_eq!(slug("Ünïcode_ok-1"), "ünïcode_ok-1");
        let mut slugs = SlugSet::default();
        assert_eq!(slugs.unique("Intro"), "intro");
        assert_eq!(slugs.unique("Intro"), "intro-1");
        assert_eq!(slugs.unique("Intro"), "intro-2");
    }
}
```

Add `pub mod links;` to `src/preview/mod.rs`.

Run: `cargo test --lib preview::links`
Expected: FAIL to compile.

- [ ] **Step 2: Implement**

Above the tests:

```rust
//! What a preview link or image reference points at. Classification never touches the network;
//! local targets resolve against the document's folder.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkAction {
    External(String),
    Anchor(String),
    LocalFile(PathBuf),
    Ignored,
}

pub fn classify_link(dest: &str, document_dir: Option<&Path>) -> LinkAction {
    let dest = dest.trim();
    let lower = dest.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:") {
        return LinkAction::External(dest.to_owned());
    }
    if let Some(anchor) = dest.strip_prefix('#') {
        return LinkAction::Anchor(percent_decode(anchor).to_lowercase());
    }
    match local_path(dest, document_dir) {
        Some(path) if path.is_file() => LinkAction::LocalFile(path),
        _ => LinkAction::Ignored,
    }
}

pub fn resolve_image_path(dest: &str, document_dir: Option<&Path>) -> Option<PathBuf> {
    local_path(dest.trim(), document_dir)
}

fn local_path(dest: &str, document_dir: Option<&Path>) -> Option<PathBuf> {
    if dest.is_empty() || dest.starts_with('#') || dest.starts_with("//") || has_scheme(dest) {
        return None;
    }
    let without_suffix = dest.split(['#', '?']).next().unwrap_or_default();
    if without_suffix.is_empty() {
        return None;
    }
    let path = PathBuf::from(percent_decode(without_suffix).replace('/', "\\"));
    if path.is_absolute() {
        Some(path)
    } else {
        Some(document_dir?.join(path))
    }
}

/// `C:\x` and `C:/x` are drive paths, not a one-letter scheme.
fn has_scheme(dest: &str) -> bool {
    let Some(colon) = dest.find(':') else {
        return false;
    };
    let scheme = &dest[..colon];
    let is_drive = scheme.len() == 1 && scheme.as_bytes()[0].is_ascii_alphabetic();
    !is_drive
        && scheme.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        && scheme
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'.' | b'-'))
}

fn percent_decode(value: &str) -> String {
    fn hex(byte: u8) -> Option<u8> {
        (byte as char).to_digit(16).map(|digit| digit as u8)
    }
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2]))
        {
            decoded.push(high * 16 + low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).unwrap_or_else(|_| value.to_owned())
}

/// GitHub's heading anchor: lowercase, spaces become hyphens, other punctuation is dropped.
pub fn slug(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter_map(|character| match character {
            ' ' => Some('-'),
            '-' | '_' => Some(character),
            _ if character.is_alphanumeric() => Some(character),
            _ => None,
        })
        .collect()
}

#[derive(Debug, Default)]
pub struct SlugSet {
    seen: HashMap<String, usize>,
}

impl SlugSet {
    /// Repeated headings get `-1`, `-2`, ... suffixes, as on GitHub.
    pub fn unique(&mut self, text: &str) -> String {
        let base = slug(text);
        let count = self.seen.entry(base.clone()).or_insert(0);
        let result = if *count == 0 { base } else { format!("{base}-{count}") };
        *count += 1;
        result
    }
}
```

- [ ] **Step 3: Run tests, Clippy, commit**

Run: `cargo test --lib preview::links` then `cargo clippy --all-targets`
Expected: PASS, no warnings.

```bash
git add src/preview/mod.rs src/preview/links.rs
git commit -m "feat: preview link classification and GitHub heading slugs"
```

---

### Task 6: Height index and scroll mapping

**Files:**
- Create: `src/preview/heights.rs`
- Modify: `src/preview/mod.rs` (add `pub mod heights;`)

**Interfaces:**
- Consumes: `model::Block` (Task 3).
- Produces:
  - `#[derive(Debug, Default)] pub struct HeightIndex` with `reset(&mut self, estimates: impl IntoIterator<Item = f32>)`, `splice(&mut self, old: Range<usize>, estimates: &[f32])`, `len(&self) -> usize`, `height(&self, index: usize) -> f32`, `is_measured(&self, index: usize) -> bool`, `set_measured(&mut self, index: usize, height: f32)`, `top(&mut self, index: usize) -> f32`, `total(&mut self) -> f32`, `index_at(&mut self, y: f32) -> usize`, `anchor(&mut self, scroll_y: f32) -> (usize, f32)`, `scroll_for_anchor(&mut self, anchor: (usize, f32)) -> f32`
  - `pub fn estimate_height(line_count: usize, line_height: f32, gap: f32) -> f32`
  - `pub fn offset_for_line(blocks: &[Block], heights: &mut HeightIndex, line: usize) -> f32`
  - `pub fn line_for_offset(blocks: &[Block], heights: &mut HeightIndex, y: f32) -> usize`

- [ ] **Step 1: Write the failing tests**

Create `src/preview/heights.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::model::parse_document;

    #[test]
    fn tops_are_running_sums_and_update_after_measurement() {
        let mut heights = HeightIndex::default();
        heights.reset([10.0, 20.0, 30.0]);
        assert_eq!(heights.top(0), 0.0);
        assert_eq!(heights.top(2), 30.0);
        assert_eq!(heights.total(), 60.0);
        heights.set_measured(0, 15.0);
        assert!(heights.is_measured(0));
        assert_eq!(heights.top(2), 35.0);
        assert_eq!(heights.index_at(34.9), 1);
        assert_eq!(heights.index_at(35.0), 2);
        assert_eq!(heights.index_at(1_000.0), 2);
    }

    #[test]
    fn splice_replaces_estimates_and_forgets_measurements() {
        let mut heights = HeightIndex::default();
        heights.reset([10.0, 10.0, 10.0]);
        heights.set_measured(1, 50.0);
        heights.splice(1..2, &[5.0, 5.0]);
        assert_eq!(heights.len(), 4);
        assert!(!heights.is_measured(1));
        assert_eq!(heights.total(), 30.0);
    }

    #[test]
    fn anchors_keep_the_first_visible_block_in_place() {
        let mut heights = HeightIndex::default();
        heights.reset([10.0, 10.0, 10.0]);
        let anchor = heights.anchor(15.0);
        assert_eq!(anchor, (1, 5.0));
        heights.set_measured(0, 30.0);
        assert_eq!(heights.scroll_for_anchor(anchor), 35.0);
    }

    #[test]
    fn lines_and_offsets_round_trip_inside_blocks() {
        let source = "# A\n\none\ntwo\nthree\n\n- x\n- y\n";
        let (blocks, _) = parse_document(source);
        let mut heights = HeightIndex::default();
        heights.reset(blocks.iter().map(|block| estimate_height(block.lines.len(), 20.0, 16.0)));
        for block in &blocks {
            for line in block.lines.clone() {
                let offset = offset_for_line(&blocks, &mut heights, line);
                assert_eq!(line_for_offset(&blocks, &mut heights, offset), line, "line {line}");
            }
        }
    }

    #[test]
    fn offsets_grow_with_source_lines() {
        let (blocks, _) = parse_document("a\n\nb\n\nc\n");
        let mut heights = HeightIndex::default();
        heights.reset(blocks.iter().map(|_| 36.0));
        assert!(offset_for_line(&blocks, &mut heights, 0) < offset_for_line(&blocks, &mut heights, 2));
        assert!(offset_for_line(&blocks, &mut heights, 2) < offset_for_line(&blocks, &mut heights, 4));
        assert_eq!(offset_for_line(&blocks, &mut heights, 99), heights.total());
    }
}
```

Add `pub mod heights;` to `src/preview/mod.rs`.

Run: `cargo test --lib preview::heights`
Expected: FAIL to compile.

- [ ] **Step 2: Implement**

```rust
//! Block heights for the virtualized preview: estimates until a block is laid out, exact heights
//! afterwards, with running sums rebuilt lazily from the first changed block.

use crate::preview::model::Block;
use std::ops::Range;

#[derive(Debug, Default)]
pub struct HeightIndex {
    heights: Vec<f32>,
    measured: Vec<bool>,
    /// `prefix[i]` is the sum of `heights[..i]`; entries `0..=valid` are current.
    prefix: Vec<f32>,
    valid: usize,
}

impl HeightIndex {
    pub fn reset(&mut self, estimates: impl IntoIterator<Item = f32>) {
        self.heights = estimates.into_iter().collect();
        self.measured = vec![false; self.heights.len()];
        self.prefix = vec![0.0; self.heights.len() + 1];
        self.valid = 0;
    }

    pub fn splice(&mut self, old: Range<usize>, estimates: &[f32]) {
        self.heights.splice(old.clone(), estimates.iter().copied());
        self.measured
            .splice(old.clone(), std::iter::repeat_n(false, estimates.len()));
        self.prefix.resize(self.heights.len() + 1, 0.0);
        self.valid = self.valid.min(old.start);
    }

    pub fn len(&self) -> usize {
        self.heights.len()
    }

    pub fn height(&self, index: usize) -> f32 {
        self.heights.get(index).copied().unwrap_or(0.0)
    }

    pub fn is_measured(&self, index: usize) -> bool {
        self.measured.get(index).copied().unwrap_or(false)
    }

    pub fn set_measured(&mut self, index: usize, height: f32) {
        if index < self.heights.len() {
            self.heights[index] = height;
            self.measured[index] = true;
            self.valid = self.valid.min(index);
        }
    }

    fn ensure_prefix(&mut self, upto: usize) {
        if self.prefix.len() != self.heights.len() + 1 {
            self.prefix.resize(self.heights.len() + 1, 0.0);
        }
        while self.valid < upto {
            self.prefix[self.valid + 1] = self.prefix[self.valid] + self.heights[self.valid];
            self.valid += 1;
        }
    }

    pub fn top(&mut self, index: usize) -> f32 {
        let index = index.min(self.heights.len());
        self.ensure_prefix(index);
        self.prefix[index]
    }

    pub fn total(&mut self) -> f32 {
        self.top(self.heights.len())
    }

    /// The block containing `y`, clamped to the last block.
    pub fn index_at(&mut self, y: f32) -> usize {
        let count = self.heights.len();
        if count == 0 {
            return 0;
        }
        self.ensure_prefix(count);
        self.prefix[..count].partition_point(|top| *top <= y).saturating_sub(1)
    }

    pub fn anchor(&mut self, scroll_y: f32) -> (usize, f32) {
        let index = self.index_at(scroll_y);
        (index, scroll_y - self.top(index))
    }

    pub fn scroll_for_anchor(&mut self, (index, within): (usize, f32)) -> f32 {
        self.top(index) + within.min(self.height(index))
    }
}

pub fn estimate_height(line_count: usize, line_height: f32, gap: f32) -> f32 {
    line_count.max(1) as f32 * line_height + gap
}

pub fn offset_for_line(blocks: &[Block], heights: &mut HeightIndex, line: usize) -> f32 {
    let index = blocks.partition_point(|block| block.lines.end <= line);
    let Some(block) = blocks.get(index) else {
        return heights.total();
    };
    let top = heights.top(index);
    if line < block.lines.start {
        return top;
    }
    let span = block.lines.len().max(1) as f32;
    top + (line - block.lines.start) as f32 / span * heights.height(index)
}

pub fn line_for_offset(blocks: &[Block], heights: &mut HeightIndex, y: f32) -> usize {
    if blocks.is_empty() {
        return 0;
    }
    let index = heights.index_at(y.max(0.0)).min(blocks.len() - 1);
    let block = &blocks[index];
    let height = heights.height(index).max(1.0);
    let fraction = ((y - heights.top(index)) / height).clamp(0.0, 1.0);
    let span = block.lines.len().max(1);
    let offset = ((fraction * span as f32) + 1e-3).floor() as usize;
    block.lines.start + offset.min(span - 1)
}
```

- [ ] **Step 3: Run tests, Clippy, commit**

Run: `cargo test --lib preview::heights` then `cargo clippy --all-targets`
Expected: PASS.

```bash
git add src/preview/mod.rs src/preview/heights.rs
git commit -m "feat: preview height index and source-line scroll mapping"
```

---

### Task 7: Preview colors

**Files:**
- Create: `src/preview/colors.rs`
- Modify: `src/preview/mod.rs` (add `pub mod colors;`)

**Interfaces:**
- Consumes: `crate::platform::theme::Theme`, `crate::window::palette::Palette::for_theme(theme, high_contrast)`, `crate::catppuccin::{self, Flavor}`, `crate::languages::rgb`.
- Produces:
  - `pub enum ColorRole { Background, Text, Muted, Heading, Link, CodeBackground, Border, QuoteBar, TableStripe, Focus }` with `pub const ALL: [ColorRole; 10]` in declaration order (`Clone, Copy, Debug, Eq, PartialEq, Hash`)
  - `pub struct PreviewColors { pub background: u32, pub text: u32, pub muted: u32, pub heading: u32, pub link: u32, pub code_background: u32, pub border: u32, pub quote_bar: u32, pub table_stripe: u32, pub focus: u32 }` with `pub fn get(&self, role: ColorRole) -> u32`
  - `pub fn preview_colors(theme: Theme, high_contrast: bool) -> PreviewColors`

Colors are `COLORREF` values (`0x00BBGGRR`), like every other color in FastPad.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::window::palette::Palette;

    #[test]
    fn preview_background_matches_the_editor_for_every_theme() {
        for theme in Theme::ALL {
            for high_contrast in [false, true] {
                assert_eq!(
                    preview_colors(theme, high_contrast).background,
                    Palette::for_theme(theme, high_contrast).editor_background
                );
            }
        }
    }

    #[test]
    fn text_roles_are_distinguishable_from_the_background() {
        for theme in Theme::ALL {
            for high_contrast in [false, true] {
                let colors = preview_colors(theme, high_contrast);
                for role in [ColorRole::Text, ColorRole::Muted, ColorRole::Heading, ColorRole::Link] {
                    assert_ne!(colors.get(role), colors.background, "{theme:?} {role:?}");
                }
            }
        }
    }

    #[test]
    fn catppuccin_uses_the_style_guide_roles() {
        let mocha = preview_colors(Theme::CatppuccinMocha, false);
        assert_eq!(mocha.link, crate::catppuccin::MOCHA.blue);
        assert_eq!(mocha.border, crate::catppuccin::MOCHA.surface1);
        assert_eq!(mocha.muted, crate::catppuccin::MOCHA.subtext0);
        assert_eq!(mocha.code_background, crate::catppuccin::MOCHA.mantle);
    }

    #[test]
    fn roles_index_their_fields() {
        let colors = preview_colors(Theme::Light, false);
        assert_eq!(colors.get(ColorRole::Link), colors.link);
        assert_eq!(ColorRole::ALL[ColorRole::Focus as usize], ColorRole::Focus);
    }
}
```

Run: `cargo test --lib preview::colors`
Expected: FAIL to compile.

- [ ] **Step 2: Implement**

```rust
//! The preview's color roles per theme. Backgrounds always equal the editor's so Split mode reads
//! as one surface; high contrast takes system colors.

use crate::catppuccin::{self, Flavor};
use crate::languages::rgb;
use crate::platform::theme::Theme;
use crate::window::palette::Palette;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ColorRole {
    Background,
    Text,
    Muted,
    Heading,
    Link,
    CodeBackground,
    Border,
    QuoteBar,
    TableStripe,
    Focus,
}

impl ColorRole {
    pub const ALL: [ColorRole; 10] = [
        Self::Background,
        Self::Text,
        Self::Muted,
        Self::Heading,
        Self::Link,
        Self::CodeBackground,
        Self::Border,
        Self::QuoteBar,
        Self::TableStripe,
        Self::Focus,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreviewColors {
    pub background: u32,
    pub text: u32,
    pub muted: u32,
    pub heading: u32,
    pub link: u32,
    pub code_background: u32,
    pub border: u32,
    pub quote_bar: u32,
    pub table_stripe: u32,
    pub focus: u32,
}

impl PreviewColors {
    pub fn get(&self, role: ColorRole) -> u32 {
        match role {
            ColorRole::Background => self.background,
            ColorRole::Text => self.text,
            ColorRole::Muted => self.muted,
            ColorRole::Heading => self.heading,
            ColorRole::Link => self.link,
            ColorRole::CodeBackground => self.code_background,
            ColorRole::Border => self.border,
            ColorRole::QuoteBar => self.quote_bar,
            ColorRole::TableStripe => self.table_stripe,
            ColorRole::Focus => self.focus,
        }
    }
}

const GITHUB_LIGHT: PreviewColors = PreviewColors {
    background: rgb(255, 255, 255),
    text: rgb(31, 35, 40),
    muted: rgb(89, 99, 110),
    heading: rgb(31, 35, 40),
    link: rgb(9, 105, 218),
    code_background: rgb(246, 248, 250),
    border: rgb(209, 217, 224),
    quote_bar: rgb(209, 217, 224),
    table_stripe: rgb(246, 248, 250),
    focus: rgb(9, 105, 218),
};

const GITHUB_DARK: PreviewColors = PreviewColors {
    background: rgb(30, 30, 30),
    text: rgb(230, 237, 243),
    muted: rgb(145, 152, 161),
    heading: rgb(230, 237, 243),
    link: rgb(68, 147, 248),
    code_background: rgb(45, 45, 45),
    border: rgb(61, 68, 77),
    quote_bar: rgb(61, 68, 77),
    table_stripe: rgb(37, 37, 38),
    focus: rgb(68, 147, 248),
};

const fn catppuccin_colors(flavor: &Flavor) -> PreviewColors {
    PreviewColors {
        background: flavor.base,
        text: flavor.text,
        muted: flavor.subtext0,
        heading: flavor.text,
        link: flavor.blue,
        code_background: flavor.mantle,
        border: flavor.surface1,
        quote_bar: flavor.surface1,
        table_stripe: catppuccin::blend(flavor.surface0, flavor.base, 96),
        focus: flavor.blue,
    }
}

pub fn preview_colors(theme: Theme, high_contrast: bool) -> PreviewColors {
    let palette = Palette::for_theme(theme, high_contrast);
    if high_contrast {
        let link = unsafe {
            windows_sys::Win32::Graphics::Gdi::GetSysColor(
                windows_sys::Win32::Graphics::Gdi::COLOR_HOTLIGHT,
            )
        };
        return PreviewColors {
            background: palette.editor_background,
            text: palette.editor_foreground,
            muted: palette.editor_foreground,
            heading: palette.editor_foreground,
            link,
            code_background: palette.editor_background,
            border: palette.editor_foreground,
            quote_bar: palette.editor_foreground,
            table_stripe: palette.editor_background,
            focus: palette.selection_background,
        };
    }
    let colors = match theme {
        Theme::Light => GITHUB_LIGHT,
        Theme::Dark => GITHUB_DARK,
        Theme::CatppuccinLatte => catppuccin_colors(&catppuccin::LATTE),
        Theme::CatppuccinFrappe => catppuccin_colors(&catppuccin::FRAPPE),
        Theme::CatppuccinMacchiato => catppuccin_colors(&catppuccin::MACCHIATO),
        Theme::CatppuccinMocha => catppuccin_colors(&catppuccin::MOCHA),
    };
    PreviewColors { background: palette.editor_background, ..colors }
}
```

If `COLOR_HOTLIGHT` equals the high-contrast background on the test machine and the distinguishability test fails, fall back to `palette.editor_foreground` when the two are equal. If `Palette::for_theme` is not `pub(crate)`, make it `pub(crate)`.

- [ ] **Step 3: Run tests, Clippy, commit**

Run: `cargo test --lib preview::colors` then `cargo clippy --all-targets`
Expected: PASS.

```bash
git add src/preview/mod.rs src/preview/colors.rs src/window/palette.rs
git commit -m "feat: preview color roles for every theme"
```

---

### Task 8: Lazy Direct2D and DirectWrite factories

**Files:**
- Create: `src/preview/dwrite.rs`
- Modify: `src/preview/mod.rs` (add `pub mod dwrite;`)

**Interfaces:**
- Consumes: `crate::platform::{OwnedModule, last_error, wide_null}`, `crate::FastPadError::Win32`.
- Produces:
  - `pub struct Graphics { pub d2d: ID2D1Factory, pub dwrite: IDWriteFactory, .. }`
  - `impl Graphics { pub fn load() -> Result<Self>; pub fn text_format(&self, family: &str, size: f32, weight: DWRITE_FONT_WEIGHT, style: DWRITE_FONT_STYLE) -> Result<IDWriteTextFormat> }`
  - `pub fn hresult_error(error: windows::core::Error) -> FastPadError`

- [ ] **Step 1: Write the failing test**

Create `src/preview/dwrite.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::DirectWrite::{
        DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_METRICS,
    };

    #[test]
    fn graphics_loads_both_factories_and_measures_text() {
        let graphics = Graphics::load().unwrap();
        let format = graphics
            .text_format("Segoe UI", 16.0, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_STYLE_NORMAL)
            .unwrap();
        let text = "Hello".encode_utf16().collect::<Vec<_>>();
        let layout = unsafe { graphics.dwrite.CreateTextLayout(&text, &format, 500.0, 100.0) }
            .unwrap();
        let mut metrics = DWRITE_TEXT_METRICS::default();
        unsafe { layout.GetMetrics(&mut metrics) }.unwrap();
        assert!(metrics.width > 0.0 && metrics.height > 0.0);
    }
}
```

Run: `cargo test --lib preview::dwrite`
Expected: FAIL to compile.

- [ ] **Step 2: Implement**

```rust
//! Direct2D and DirectWrite, loaded on the first preview. Both factory functions are resolved with
//! GetProcAddress so neither DLL enters FastPad.exe's import table; the `markdown_preview`
//! integration test enforces that. Never call the `windows` crate's `D2D1CreateFactory` or
//! `DWriteCreateFactory` wrappers: they add static imports.

use crate::platform::{OwnedModule, last_error, wide_null};
use crate::{FastPadError, Result};
use std::ffi::{CStr, c_void};
use windows::Win32::Graphics::Direct2D::{
    D2D1_FACTORY_OPTIONS, D2D1_FACTORY_TYPE, D2D1_FACTORY_TYPE_SINGLE_THREADED, ID2D1Factory,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL,
    DWRITE_FONT_STYLE, DWRITE_FONT_WEIGHT, IDWriteFactory, IDWriteTextFormat,
};
use windows::core::{GUID, HRESULT, Interface, PCWSTR};
use windows_sys::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};

type D2D1CreateFactoryFn = unsafe extern "system" fn(
    D2D1_FACTORY_TYPE,
    *const GUID,
    *const D2D1_FACTORY_OPTIONS,
    *mut *mut c_void,
) -> HRESULT;
type DWriteCreateFactoryFn =
    unsafe extern "system" fn(DWRITE_FACTORY_TYPE, *const GUID, *mut *mut c_void) -> HRESULT;

/// Field order matters: the factories drop before the modules that implement them.
pub struct Graphics {
    pub d2d: ID2D1Factory,
    pub dwrite: IDWriteFactory,
    _d2d_module: OwnedModule,
    _dwrite_module: OwnedModule,
}

impl std::fmt::Debug for Graphics {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Graphics")
    }
}

pub fn hresult_error(error: windows::core::Error) -> FastPadError {
    FastPadError::Win32(error.code().0 as u32)
}

fn load_system_library(name: &str) -> Result<OwnedModule> {
    let wide = wide_null(name);
    let raw = unsafe {
        LoadLibraryExW(wide.as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32)
    };
    unsafe { OwnedModule::from_raw_owned(raw) }
}

/// # Safety
/// `F` must be the exact `extern "system"` signature of the export `name`.
unsafe fn resolve<F: Copy>(module: &OwnedModule, name: &CStr) -> Result<F> {
    assert_eq!(std::mem::size_of::<F>(), std::mem::size_of::<usize>());
    let proc = unsafe { GetProcAddress(module.as_raw(), name.as_ptr().cast()) };
    let Some(proc) = proc else {
        return Err(last_error());
    };
    Ok(unsafe { std::mem::transmute_copy::<unsafe extern "system" fn() -> isize, F>(&proc) })
}

impl Graphics {
    pub fn load() -> Result<Self> {
        let d2d_module = load_system_library("d2d1.dll")?;
        let dwrite_module = load_system_library("dwrite.dll")?;
        let d2d = unsafe {
            let create: D2D1CreateFactoryFn = resolve(&d2d_module, c"D2D1CreateFactory")?;
            let mut raw = std::ptr::null_mut();
            create(
                D2D1_FACTORY_TYPE_SINGLE_THREADED,
                &ID2D1Factory::IID,
                std::ptr::null(),
                &mut raw,
            )
            .ok()
            .map_err(hresult_error)?;
            ID2D1Factory::from_raw(raw)
        };
        let dwrite = unsafe {
            let create: DWriteCreateFactoryFn = resolve(&dwrite_module, c"DWriteCreateFactory")?;
            let mut raw = std::ptr::null_mut();
            create(DWRITE_FACTORY_TYPE_SHARED, &IDWriteFactory::IID, &mut raw)
                .ok()
                .map_err(hresult_error)?;
            IDWriteFactory::from_raw(raw)
        };
        Ok(Self { d2d, dwrite, _d2d_module: d2d_module, _dwrite_module: dwrite_module })
    }

    pub fn text_format(
        &self,
        family: &str,
        size: f32,
        weight: DWRITE_FONT_WEIGHT,
        style: DWRITE_FONT_STYLE,
    ) -> Result<IDWriteTextFormat> {
        let family = wide_null(family);
        let locale = wide_null("");
        unsafe {
            self.dwrite.CreateTextFormat(
                PCWSTR(family.as_ptr()),
                None,
                weight,
                style,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                PCWSTR(locale.as_ptr()),
            )
        }
        .map_err(hresult_error)
    }
}
```

If `None` does not satisfy the font-collection parameter's `Param` bound, pass `None::<&windows::Win32::Graphics::DirectWrite::IDWriteFontCollection>`.

- [ ] **Step 3: Run tests, Clippy, import guard, commit**

Run: `cargo test --lib preview::dwrite`, `cargo clippy --all-targets`, `cargo build --release`, `cargo test --test markdown_preview binary_does_not_statically_import -- --test-threads=1`
Expected: all PASS; the guard proves `Graphics::load` added no static import.

```bash
git add src/preview/mod.rs src/preview/dwrite.rs
git commit -m "feat: load Direct2D and DirectWrite on demand without static imports"
```

---

### Task 9: Brushes and block layout

**Files:**
- Create: `src/preview/render.rs` (brushes, rectangles, render target creation, test window)
- Create: `src/preview/layout.rs`
- Modify: `src/preview/mod.rs` (add `pub mod layout; pub mod render;`)

**Interfaces:**
- Consumes: `Graphics`, `hresult_error` (Task 8); `ColorRole`, `PreviewColors`, `preview_colors` (Task 7); `model::*` (Task 3); `links::resolve_image_path` (Task 5).
- Produces in `render.rs`:
  - `#[derive(Clone, Copy, Debug, Default, PartialEq)] pub struct RectF { pub left: f32, pub top: f32, pub right: f32, pub bottom: f32 }` with `new`, `width`, `height`, `contains(x, y)`, `offset(dx, dy) -> RectF`, `inflate(by) -> RectF`, `to_d2d() -> D2D_RECT_F`
  - `pub struct Brushes` with `pub fn create(target: &ID2D1RenderTarget, colors: &PreviewColors) -> Result<Self>` and `pub fn get(&self, role: ColorRole) -> &ID2D1SolidColorBrush`
  - `pub fn create_hwnd_target(graphics: &Graphics, hwnd: HWND, width: u32, height: u32, dpi: u32) -> Result<ID2D1HwndRenderTarget>` (`HWND` is `windows_sys::Win32::Foundation::HWND`)
  - `#[cfg(test)] pub struct TestWindow(pub HWND)` with `new(width, height)` and `Drop`
- Produces in `layout.rs`:
  - `#[derive(Clone, Debug, PartialEq)] pub struct PreviewFonts { pub body_family: String, pub code_family: String, pub body_size: f32 }` with `pub fn from_settings(font_face: &str, font_size_points: u16) -> Self` and `pub fn unit(&self) -> f32`
  - `pub struct LayoutContext<'a>` with `pub fn new(graphics: &'a Graphics, brushes: &'a Brushes, fonts: &'a PreviewFonts, document_dir: Option<&'a Path>, image_size: &'a dyn Fn(&Path) -> Option<(u32, u32)>) -> Result<Self>` and `pub fn line_height(&self) -> f32`
  - `pub enum DrawOp { Text { layout: IDWriteTextLayout, x: f32, y: f32, role: ColorRole }, Fill { rect: RectF, role: ColorRole }, RoundedFill { rect: RectF, radius: f32, role: ColorRole }, Stroke { rect: RectF, role: ColorRole }, Checkbox { rect: RectF, checked: bool }, Image { slot: usize }, Scrollable { clip: RectF, content_width: f32, ops: Vec<DrawOp> } }`
  - `pub struct LinkHit { pub text: String, pub dest: String, pub rects: Vec<RectF>, pub layout: IDWriteTextLayout, pub range: DWRITE_TEXT_RANGE, pub scrolls: bool }`
  - `pub struct ImageSlot { pub path: Option<PathBuf>, pub rect: RectF, pub alt: IDWriteTextLayout }`
  - `pub struct LaidBlock { pub height: f32, pub ops: Vec<DrawOp>, pub links: Vec<LinkHit>, pub images: Vec<ImageSlot>, pub scroll_width: f32, pub heading: Option<String> }`
  - `pub fn layout_block(ctx: &LayoutContext<'_>, kind: &BlockKind, width: f32) -> Result<LaidBlock>`

All layout coordinates are DIPs relative to the block's top-left corner at x = 0.

- [ ] **Step 1: Write `render.rs` (no behavior to test in isolation beyond compiling)**

```rust
//! Render-target and brush plumbing shared by layout and painting.

use crate::preview::colors::{ColorRole, PreviewColors};
use crate::preview::dwrite::{Graphics, hresult_error};
use crate::Result;
use windows::Win32::Foundation::HWND as WinHwnd;
use windows::Win32::Graphics::Direct2D::Common::{D2D_RECT_F, D2D_SIZE_U, D2D1_COLOR_F};
use windows::Win32::Graphics::Direct2D::{
    D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_PRESENT_OPTIONS_NONE,
    D2D1_RENDER_TARGET_PROPERTIES, ID2D1HwndRenderTarget, ID2D1RenderTarget,
    ID2D1SolidColorBrush,
};
use windows_sys::Win32::Foundation::HWND;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RectF {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl RectF {
    pub const fn new(left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self { left, top, right, bottom }
    }

    pub fn width(&self) -> f32 {
        self.right - self.left
    }

    pub fn height(&self) -> f32 {
        self.bottom - self.top
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }

    pub fn offset(&self, dx: f32, dy: f32) -> Self {
        Self::new(self.left + dx, self.top + dy, self.right + dx, self.bottom + dy)
    }

    pub fn inflate(&self, by: f32) -> Self {
        Self::new(self.left - by, self.top - by, self.right + by, self.bottom + by)
    }

    pub fn to_d2d(self) -> D2D_RECT_F {
        D2D_RECT_F { left: self.left, top: self.top, right: self.right, bottom: self.bottom }
    }
}

pub fn color_f(colorref: u32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: (colorref & 0xFF) as f32 / 255.0,
        g: ((colorref >> 8) & 0xFF) as f32 / 255.0,
        b: ((colorref >> 16) & 0xFF) as f32 / 255.0,
        a: 1.0,
    }
}

pub struct Brushes {
    brushes: Vec<ID2D1SolidColorBrush>,
}

impl Brushes {
    pub fn create(target: &ID2D1RenderTarget, colors: &PreviewColors) -> Result<Self> {
        let brushes = ColorRole::ALL
            .iter()
            .map(|role| unsafe {
                target.CreateSolidColorBrush(&color_f(colors.get(*role)), None)
            })
            .collect::<windows::core::Result<Vec<_>>>()
            .map_err(hresult_error)?;
        Ok(Self { brushes })
    }

    pub fn get(&self, role: ColorRole) -> &ID2D1SolidColorBrush {
        &self.brushes[role as usize]
    }
}

pub fn create_hwnd_target(
    graphics: &Graphics,
    hwnd: HWND,
    width: u32,
    height: u32,
    dpi: u32,
) -> Result<ID2D1HwndRenderTarget> {
    let properties = D2D1_RENDER_TARGET_PROPERTIES {
        dpiX: dpi as f32,
        dpiY: dpi as f32,
        ..Default::default()
    };
    let hwnd_properties = D2D1_HWND_RENDER_TARGET_PROPERTIES {
        hwnd: WinHwnd(hwnd),
        pixelSize: D2D_SIZE_U { width: width.max(1), height: height.max(1) },
        presentOptions: D2D1_PRESENT_OPTIONS_NONE,
    };
    unsafe { graphics.d2d.CreateHwndRenderTarget(&properties, &hwnd_properties) }
        .map_err(hresult_error)
}

#[cfg(test)]
pub struct TestWindow(pub HWND);

#[cfg(test)]
impl TestWindow {
    pub fn new(width: i32, height: i32) -> Self {
        use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, WS_POPUP};
        let class = crate::platform::wide_null("STATIC");
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                std::ptr::null(),
                WS_POPUP,
                0,
                0,
                width,
                height,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        assert!(!hwnd.is_null());
        Self(hwnd)
    }
}

#[cfg(test)]
impl Drop for TestWindow {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(self.0);
        }
    }
}
```

If `WinHwnd(hwnd)` does not type-check, construct it as `WinHwnd(hwnd.cast())`.

- [ ] **Step 2: Write the failing layout tests**

Create `src/preview/layout.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::theme::Theme;
    use crate::preview::colors::preview_colors;
    use crate::preview::model::parse_document;
    use crate::preview::render::{TestWindow, create_hwnd_target};

    fn with_context(
        image_size: &dyn Fn(&Path) -> Option<(u32, u32)>,
        document_dir: Option<&Path>,
        test: impl FnOnce(&LayoutContext<'_>),
    ) {
        let graphics = Graphics::load().unwrap();
        let window = TestWindow::new(400, 300);
        let target = create_hwnd_target(&graphics, window.0, 400, 300, 96).unwrap();
        let brushes = Brushes::create(&target, &preview_colors(Theme::Light, false)).unwrap();
        let fonts = PreviewFonts::from_settings("Consolas", 12);
        let context =
            LayoutContext::new(&graphics, &brushes, &fonts, document_dir, image_size).unwrap();
        test(&context);
    }

    fn no_images(_: &Path) -> Option<(u32, u32)> {
        None
    }

    fn laid(context: &LayoutContext<'_>, source: &str, width: f32) -> LaidBlock {
        let (blocks, _) = parse_document(source);
        layout_block(context, &blocks[0].kind, width).unwrap()
    }

    fn flatten(ops: &[DrawOp]) -> Vec<&DrawOp> {
        ops.iter()
            .flat_map(|op| match op {
                DrawOp::Scrollable { ops, .. } => {
                    let mut nested = vec![op];
                    nested.extend(flatten(ops));
                    nested
                }
                _ => vec![op],
            })
            .collect()
    }

    #[test]
    fn headings_are_taller_than_paragraphs_with_the_same_text() {
        with_context(&no_images, None, |context| {
            let heading = laid(context, "# Same\n", 400.0);
            let paragraph = laid(context, "Same\n", 400.0);
            assert!(heading.height > paragraph.height);
            assert_eq!(heading.heading.as_deref(), Some("Same"));
        });
    }

    #[test]
    fn narrow_widths_wrap_paragraphs() {
        with_context(&no_images, None, |context| {
            let text = "word ".repeat(60) + "\n";
            assert!(laid(context, &text, 100.0).height > laid(context, &text, 1000.0).height);
        });
    }

    #[test]
    fn links_get_hit_rectangles() {
        with_context(&no_images, None, |context| {
            let block = laid(context, "go [here](https://x.dev)\n", 400.0);
            assert_eq!(block.links.len(), 1);
            assert_eq!(block.links[0].dest, "https://x.dev");
            assert_eq!(block.links[0].text, "here");
            let rect = block.links[0].rects[0];
            assert!(rect.left > 0.0 && rect.width() > 0.0 && rect.height() > 0.0);
            assert!(!block.links[0].scrolls);
        });
    }

    #[test]
    fn wide_tables_and_long_code_lines_scroll_horizontally() {
        with_context(&no_images, None, |context| {
            let wide = "wide ".repeat(30);
            let table = laid(context, &format!("| {wide} | {wide} |\n|---|---|\n| a | b |\n"), 200.0);
            assert!(table.scroll_width > 200.0);
            assert!(flatten(&table.ops).iter().any(|op| matches!(op, DrawOp::Scrollable { .. })));
            let code = laid(context, &format!("```\n{wide}{wide}\n```\n"), 200.0);
            assert!(code.scroll_width > 200.0);
        });
    }

    #[test]
    fn task_items_draw_checkboxes() {
        with_context(&no_images, None, |context| {
            let block = laid(context, "- [x] done\n- [ ] todo\n", 400.0);
            let checks = flatten(&block.ops)
                .into_iter()
                .filter_map(|op| match op {
                    DrawOp::Checkbox { checked, .. } => Some(*checked),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(checks, vec![true, false]);
        });
    }

    #[test]
    fn images_scale_down_to_width_and_use_placeholders_when_unknown() {
        let dir = PathBuf::from(r"C:\docs");
        let known = |_: &Path| Some((800, 400));
        with_context(&known, Some(&dir), |context| {
            let block = laid(context, "![a](a.png)\n", 400.0);
            assert_eq!(block.images[0].path.as_deref(), Some(Path::new(r"C:\docs\a.png")));
            assert_eq!(block.images[0].rect.width(), 400.0);
            assert_eq!(block.images[0].rect.height(), 200.0);
        });
        with_context(&no_images, Some(&dir), |context| {
            let block = laid(context, "![a](a.png)\n", 400.0);
            assert!(block.images[0].rect.height() > 0.0);
            assert!(block.images[0].rect.width() <= 400.0);
        });
    }

    #[test]
    fn quotes_draw_a_bar_and_indent_their_content() {
        with_context(&no_images, None, |context| {
            let block = laid(context, "> quoted\n", 400.0);
            assert!(block.ops.iter().any(|op| matches!(op, DrawOp::Fill { role: ColorRole::QuoteBar, .. })));
            let text_x = block.ops.iter().find_map(|op| match op {
                DrawOp::Text { x, .. } => Some(*x),
                _ => None,
            });
            assert!(text_x.unwrap() > 0.0);
        });
    }
}
```

Add `pub mod layout; pub mod render;` to `src/preview/mod.rs`.

Run: `cargo test --lib preview::layout -- --test-threads=1`
Expected: FAIL to compile.

- [ ] **Step 3: Implement `layout.rs`**

Above the tests:

```rust
//! Blocks to positioned drawing operations using DirectWrite text layouts. A `LaidBlock` is valid
//! for one content width, one `PreviewFonts`, and one `Brushes` (links and muted spans use brush
//! drawing effects), so the view discards layouts when any of those change.

use crate::Result;
use crate::preview::colors::ColorRole;
use crate::preview::dwrite::{Graphics, hresult_error};
use crate::preview::links::resolve_image_path;
use crate::preview::model::{BlockKind, CellAlign, ImageRef, InlineStyle, ListItem, RichText};
use crate::preview::render::{Brushes, RectF};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FONT_STYLE_ITALIC, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_HIT_TEST_METRICS,
    DWRITE_TEXT_ALIGNMENT, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
    DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_TEXT_METRICS, DWRITE_TEXT_RANGE,
    DWRITE_WORD_WRAPPING_NO_WRAP, IDWriteTextFormat, IDWriteTextLayout,
};
use windows::core::PCWSTR;

const HEADING_SCALE: [f32; 6] = [2.0, 1.5, 1.25, 1.0, 0.875, 0.875];
const BULLETS: [&str; 3] = ["\u{2022}", "\u{25E6}", "\u{25AA}"];
const CODE_SCALE: f32 = 0.85;

#[derive(Clone, Debug, PartialEq)]
pub struct PreviewFonts {
    pub body_family: String,
    pub code_family: String,
    /// Body text size in DIPs.
    pub body_size: f32,
}

impl PreviewFonts {
    pub fn from_settings(font_face: &str, font_size_points: u16) -> Self {
        Self {
            body_family: "Segoe UI".to_owned(),
            code_family: font_face.to_owned(),
            body_size: f32::from(font_size_points) * 96.0 / 72.0,
        }
    }

    /// GitHub's spacing is expressed for 16 px body text; everything scales with the body size.
    pub fn unit(&self) -> f32 {
        self.body_size / 16.0
    }
}

pub enum DrawOp {
    Text { layout: IDWriteTextLayout, x: f32, y: f32, role: ColorRole },
    Fill { rect: RectF, role: ColorRole },
    RoundedFill { rect: RectF, radius: f32, role: ColorRole },
    Stroke { rect: RectF, role: ColorRole },
    Checkbox { rect: RectF, checked: bool },
    Image { slot: usize },
    /// Content wider than the pane: drawn clipped to `clip`, shifted by the block's horizontal
    /// scroll offset.
    Scrollable { clip: RectF, content_width: f32, ops: Vec<DrawOp> },
}

pub struct LinkHit {
    /// The visible link text, used as the accessible name.
    pub text: String,
    pub dest: String,
    pub rects: Vec<RectF>,
    pub layout: IDWriteTextLayout,
    pub range: DWRITE_TEXT_RANGE,
    /// Whether the rects move with the block's horizontal scroll offset.
    pub scrolls: bool,
}

pub struct ImageSlot {
    pub path: Option<PathBuf>,
    pub rect: RectF,
    pub alt: IDWriteTextLayout,
}

pub struct LaidBlock {
    pub height: f32,
    pub ops: Vec<DrawOp>,
    pub links: Vec<LinkHit>,
    pub images: Vec<ImageSlot>,
    /// The widest horizontally scrollable content, or 0 when nothing scrolls.
    pub scroll_width: f32,
    pub heading: Option<String>,
}

pub struct LayoutContext<'a> {
    graphics: &'a Graphics,
    brushes: &'a Brushes,
    fonts: &'a PreviewFonts,
    document_dir: Option<&'a Path>,
    image_size: &'a dyn Fn(&Path) -> Option<(u32, u32)>,
    formats: RefCell<HashMap<(bool, u32, i32), IDWriteTextFormat>>,
    line_height: f32,
}

impl<'a> LayoutContext<'a> {
    pub fn new(
        graphics: &'a Graphics,
        brushes: &'a Brushes,
        fonts: &'a PreviewFonts,
        document_dir: Option<&'a Path>,
        image_size: &'a dyn Fn(&Path) -> Option<(u32, u32)>,
    ) -> Result<Self> {
        let mut context = Self {
            graphics,
            brushes,
            fonts,
            document_dir,
            image_size,
            formats: RefCell::new(HashMap::new()),
            line_height: 0.0,
        };
        let sample = context.plain_layout("Ag", false, fonts.body_size, DWRITE_FONT_WEIGHT_NORMAL, 1000.0)?;
        context.line_height = metrics(&sample)?.height;
        Ok(context)
    }

    pub fn line_height(&self) -> f32 {
        self.line_height
    }

    fn format(&self, code: bool, size: f32, weight: DWRITE_FONT_WEIGHT) -> Result<IDWriteTextFormat> {
        let key = (code, size.to_bits(), weight.0);
        if let Some(format) = self.formats.borrow().get(&key) {
            return Ok(format.clone());
        }
        let family = if code { &self.fonts.code_family } else { &self.fonts.body_family };
        let format = self.graphics.text_format(family, size, weight, DWRITE_FONT_STYLE_NORMAL)?;
        self.formats.borrow_mut().insert(key, format.clone());
        Ok(format)
    }

    fn plain_layout(
        &self,
        text: &str,
        code: bool,
        size: f32,
        weight: DWRITE_FONT_WEIGHT,
        width: f32,
    ) -> Result<IDWriteTextLayout> {
        let format = self.format(code, size, weight)?;
        let wide = text.encode_utf16().collect::<Vec<_>>();
        unsafe { self.graphics.dwrite.CreateTextLayout(&wide, &format, width.max(1.0), f32::MAX) }
            .map_err(hresult_error)
    }

    fn rich_layout(
        &self,
        rich: &RichText,
        size: f32,
        weight: DWRITE_FONT_WEIGHT,
        width: f32,
    ) -> Result<IDWriteTextLayout> {
        let layout = self.plain_layout(&rich.text, false, size, weight, width)?;
        let code_family = crate::platform::wide_null(&self.fonts.code_family);
        for span in &rich.spans {
            let range = text_range(span.range.start, span.range.end);
            unsafe {
                match &span.style {
                    InlineStyle::Strong => layout.SetFontWeight(DWRITE_FONT_WEIGHT_SEMI_BOLD, range),
                    InlineStyle::Emphasis => layout.SetFontStyle(DWRITE_FONT_STYLE_ITALIC, range),
                    InlineStyle::Strikethrough => layout.SetStrikethrough(true, range),
                    InlineStyle::Code => layout
                        .SetFontFamilyName(PCWSTR(code_family.as_ptr()), range)
                        .and_then(|()| layout.SetFontSize(size * CODE_SCALE, range)),
                    InlineStyle::Link(_) => {
                        layout.SetDrawingEffect(self.brushes.get(ColorRole::Link), range)
                    }
                    InlineStyle::ImageAlt => layout
                        .SetFontStyle(DWRITE_FONT_STYLE_ITALIC, range)
                        .and_then(|()| {
                            layout.SetDrawingEffect(self.brushes.get(ColorRole::Muted), range)
                        }),
                }
            }
            .map_err(hresult_error)?;
        }
        Ok(layout)
    }
}

#[derive(Default)]
struct Output {
    ops: Vec<DrawOp>,
    links: Vec<LinkHit>,
    images: Vec<ImageSlot>,
    scroll_width: f32,
}

/// A laid-out block's own height and the space it wants below it.
#[derive(Clone, Copy)]
struct Extent {
    content: f32,
    margin: f32,
}

#[derive(Clone, Copy)]
struct Style {
    role: ColorRole,
    list_depth: usize,
    nested: bool,
}

pub fn layout_block(context: &LayoutContext<'_>, kind: &BlockKind, width: f32) -> Result<LaidBlock> {
    let mut output = Output::default();
    let style = Style { role: ColorRole::Text, list_depth: 0, nested: false };
    let extent = layout_kind(context, kind, 0.0, 0.0, width, style, &mut output)?;
    let heading = match kind {
        BlockKind::Heading { text, .. } => Some(text.plain_text().to_owned()),
        _ => None,
    };
    Ok(LaidBlock {
        height: extent.content + extent.margin,
        ops: output.ops,
        links: output.links,
        images: output.images,
        scroll_width: output.scroll_width,
        heading,
    })
}

fn layout_kind(
    context: &LayoutContext<'_>,
    kind: &BlockKind,
    x: f32,
    y: f32,
    width: f32,
    style: Style,
    output: &mut Output,
) -> Result<Extent> {
    let unit = context.fonts.unit();
    let margin = if style.nested { 4.0 * unit } else { 16.0 * unit };
    match kind {
        BlockKind::Paragraph(text) => {
            let height = push_rich_text(context, text, context.fonts.body_size, DWRITE_FONT_WEIGHT_NORMAL, x, y, width, style.role, false, output)?;
            Ok(Extent { content: height, margin })
        }
        BlockKind::Heading { level, text } => {
            let size = context.fonts.body_size * HEADING_SCALE[usize::from(*level).clamp(1, 6) - 1];
            let role = match (style.role, *level) {
                (ColorRole::Text, 5 | 6) => ColorRole::Muted,
                (ColorRole::Text, _) => ColorRole::Heading,
                (role, _) => role,
            };
            let top = y + 8.0 * unit;
            let mut bottom = top
                + push_rich_text(context, text, size, DWRITE_FONT_WEIGHT_SEMI_BOLD, x, top, width, role, false, output)?;
            if *level <= 2 {
                bottom += 0.3 * size;
                output.ops.push(DrawOp::Fill { rect: RectF::new(x, bottom, x + width, bottom + 1.0), role: ColorRole::Border });
                bottom += 1.0;
            }
            Ok(Extent { content: bottom - y, margin })
        }
        BlockKind::Code { text, .. } => push_code(context, text, x, y, width, ColorRole::Text, true, output, margin),
        BlockKind::Html(html) => push_code(context, html, x, y, width, ColorRole::Muted, false, output, margin),
        BlockKind::Rule => {
            output.ops.push(DrawOp::Fill { rect: RectF::new(x, y + 8.0 * unit, x + width, y + 8.0 * unit + 1.0), role: ColorRole::Border });
            Ok(Extent { content: 16.0 * unit + 1.0, margin: 8.0 * unit })
        }
        BlockKind::Quote(blocks) => {
            let bar_index = output.ops.len();
            let indent = 16.0 * unit;
            let inner = Style { role: ColorRole::Muted, ..style };
            let content = layout_children(context, blocks, x + indent, y, width - indent, inner, output)?;
            output.ops.insert(bar_index, DrawOp::Fill { rect: RectF::new(x, y, x + 4.0 * unit, y + content), role: ColorRole::QuoteBar });
            Ok(Extent { content, margin })
        }
        BlockKind::List { start, items } => push_list(context, *start, items, x, y, width, style, output, margin),
        BlockKind::Table { alignments, head, rows } => push_table(context, alignments, head, rows, x, y, width, style.role, output, margin),
        BlockKind::Images(images) => push_images(context, images, x, y, width, output, margin),
    }
}

fn layout_children(
    context: &LayoutContext<'_>,
    blocks: &[BlockKind],
    x: f32,
    y: f32,
    width: f32,
    style: Style,
    output: &mut Output,
) -> Result<f32> {
    let mut cursor = y;
    let mut last_margin = 0.0;
    for block in blocks {
        let extent = layout_kind(context, block, x, cursor, width, style, output)?;
        cursor += extent.content + extent.margin;
        last_margin = extent.margin;
    }
    Ok((cursor - y - last_margin).max(0.0))
}

#[allow(clippy::too_many_arguments)]
fn push_rich_text(
    context: &LayoutContext<'_>,
    text: &RichText,
    size: f32,
    weight: DWRITE_FONT_WEIGHT,
    x: f32,
    y: f32,
    width: f32,
    role: ColorRole,
    scrolls: bool,
    output: &mut Output,
) -> Result<f32> {
    let layout = context.rich_layout(text, size, weight, width)?;
    push_laid_text(context, text, layout, x, y, role, scrolls, output)
}

#[allow(clippy::too_many_arguments)]
fn push_laid_text(
    context: &LayoutContext<'_>,
    text: &RichText,
    layout: IDWriteTextLayout,
    x: f32,
    y: f32,
    role: ColorRole,
    scrolls: bool,
    output: &mut Output,
) -> Result<f32> {
    let unit = context.fonts.unit();
    for span in &text.spans {
        match &span.style {
            InlineStyle::Code => {
                for rect in range_rects(&layout, span.range.start, span.range.end, x, y)? {
                    output.ops.push(DrawOp::RoundedFill { rect: rect.inflate(2.0 * unit), radius: 4.0 * unit, role: ColorRole::CodeBackground });
                }
            }
            InlineStyle::Link(dest) => output.links.push(LinkHit {
                text: String::from_utf16_lossy(
                    &text.text.encode_utf16().collect::<Vec<_>>()
                        [span.range.start as usize..span.range.end as usize],
                ),
                dest: dest.clone(),
                rects: range_rects(&layout, span.range.start, span.range.end, x, y)?,
                layout: layout.clone(),
                range: text_range(span.range.start, span.range.end),
                scrolls,
            }),
            _ => {}
        }
    }
    let height = metrics(&layout)?.height;
    output.ops.push(DrawOp::Text { layout, x, y, role });
    Ok(height)
}

#[allow(clippy::too_many_arguments)]
fn push_code(
    context: &LayoutContext<'_>,
    text: &str,
    x: f32,
    y: f32,
    width: f32,
    role: ColorRole,
    boxed: bool,
    output: &mut Output,
    margin: f32,
) -> Result<Extent> {
    let unit = context.fonts.unit();
    let padding = if boxed { 16.0 * unit } else { 0.0 };
    let layout = context.plain_layout(text, true, context.fonts.body_size * CODE_SCALE, DWRITE_FONT_WEIGHT_NORMAL, width)?;
    unsafe { layout.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP) }.map_err(hresult_error)?;
    let text_metrics = metrics(&layout)?;
    let height = text_metrics.height.max(context.line_height) + 2.0 * padding;
    let rect = RectF::new(x, y, x + width, y + height);
    if boxed {
        output.ops.push(DrawOp::RoundedFill { rect, radius: 6.0 * unit, role: ColorRole::CodeBackground });
    }
    let content_width = text_metrics.widthIncludingTrailingWhitespace + 2.0 * padding;
    let text_op = DrawOp::Text { layout, x: x + padding, y: y + padding, role };
    if content_width > width {
        output.scroll_width = output.scroll_width.max(content_width);
        output.ops.push(DrawOp::Scrollable { clip: rect, content_width, ops: vec![text_op] });
    } else {
        output.ops.push(text_op);
    }
    Ok(Extent { content: height, margin })
}

#[allow(clippy::too_many_arguments)]
fn push_list(
    context: &LayoutContext<'_>,
    start: Option<u64>,
    items: &[ListItem],
    x: f32,
    y: f32,
    width: f32,
    style: Style,
    output: &mut Output,
    margin: f32,
) -> Result<Extent> {
    let unit = context.fonts.unit();
    let indent = 32.0 * unit;
    let item_gap = 4.0 * unit;
    let mut cursor = y;
    for (index, item) in items.iter().enumerate() {
        if let Some(checked) = item.task {
            let size = 14.0 * unit;
            let top = cursor + (context.line_height - size) / 2.0;
            output.ops.push(DrawOp::Checkbox { rect: RectF::new(x + indent - size - 8.0 * unit, top, x + indent - 8.0 * unit, top + size), checked });
        } else {
            let marker = match start {
                Some(first) => format!("{}.", first + index as u64),
                None => BULLETS[style.list_depth % BULLETS.len()].to_owned(),
            };
            let layout = context.plain_layout(&marker, false, context.fonts.body_size, DWRITE_FONT_WEIGHT_NORMAL, indent - 8.0 * unit)?;
            unsafe { layout.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING) }.map_err(hresult_error)?;
            output.ops.push(DrawOp::Text { layout, x, y: cursor, role: style.role });
        }
        let inner = Style { role: style.role, list_depth: style.list_depth + 1, nested: true };
        let content = layout_children(context, &item.blocks, x + indent, cursor, width - indent, inner, output)?;
        cursor += content.max(context.line_height) + item_gap;
    }
    Ok(Extent { content: (cursor - y - item_gap).max(0.0), margin: if style.nested { 0.0 } else { margin } })
}

#[allow(clippy::too_many_arguments)]
fn push_table(
    context: &LayoutContext<'_>,
    alignments: &[CellAlign],
    head: &[RichText],
    rows: &[Vec<RichText>],
    x: f32,
    y: f32,
    width: f32,
    role: ColorRole,
    output: &mut Output,
    margin: f32,
) -> Result<Extent> {
    let unit = context.fonts.unit();
    let (pad_x, pad_y) = (13.0 * unit, 6.0 * unit);
    let columns = alignments.len().max(head.len()).max(rows.iter().map(Vec::len).max().unwrap_or(0));
    let all_rows = std::iter::once(head).chain(rows.iter().map(Vec::as_slice)).collect::<Vec<_>>();
    let mut layouts = Vec::with_capacity(all_rows.len());
    let mut column_widths = vec![0.0_f32; columns];
    for (row_index, row) in all_rows.iter().enumerate() {
        let weight = if row_index == 0 { DWRITE_FONT_WEIGHT_SEMI_BOLD } else { DWRITE_FONT_WEIGHT_NORMAL };
        let mut row_layouts = Vec::with_capacity(columns);
        for column in 0..columns {
            let empty = RichText::default();
            let cell = row.get(column).unwrap_or(&empty);
            let layout = context.rich_layout(cell, context.fonts.body_size, weight, 100_000.0)?;
            let natural = metrics(&layout)?.widthIncludingTrailingWhitespace;
            column_widths[column] = column_widths[column].max(natural + 2.0 * pad_x);
            row_layouts.push(layout);
        }
        layouts.push(row_layouts);
    }
    let table_width: f32 = column_widths.iter().sum();
    let scrolls = table_width > width;
    let mut table_output = Output::default();
    let mut cursor = y;
    for (row_index, row_layouts) in layouts.into_iter().enumerate() {
        let mut row_height = context.line_height;
        for (column, layout) in row_layouts.iter().enumerate() {
            let inner = column_widths[column] - 2.0 * pad_x;
            unsafe {
                layout.SetMaxWidth(inner.max(1.0)).map_err(hresult_error)?;
                layout.SetTextAlignment(alignment(alignments.get(column).copied().unwrap_or(CellAlign::None))).map_err(hresult_error)?;
            }
            row_height = row_height.max(metrics(layout)?.height);
        }
        row_height += 2.0 * pad_y;
        if row_index > 0 && row_index % 2 == 0 {
            table_output.ops.push(DrawOp::Fill { rect: RectF::new(x, cursor, x + table_width, cursor + row_height), role: ColorRole::TableStripe });
        }
        let mut cell_x = x;
        for (column, layout) in row_layouts.into_iter().enumerate() {
            let cell_rect = RectF::new(cell_x, cursor, cell_x + column_widths[column], cursor + row_height);
            table_output.ops.push(DrawOp::Stroke { rect: cell_rect, role: ColorRole::Border });
            let empty = RichText::default();
            let cell = all_rows[row_index].get(column).unwrap_or(&empty);
            push_laid_text(context, cell, layout, cell_x + pad_x, cursor + pad_y, role, scrolls, &mut table_output)?;
            cell_x += column_widths[column];
        }
        cursor += row_height;
    }
    let clip = RectF::new(x, y, x + width, cursor);
    output.links.append(&mut table_output.links);
    if scrolls {
        output.scroll_width = output.scroll_width.max(table_width);
        output.ops.push(DrawOp::Scrollable { clip, content_width: table_width, ops: table_output.ops });
    } else {
        output.ops.append(&mut table_output.ops);
    }
    Ok(Extent { content: cursor - y, margin })
}

fn push_images(
    context: &LayoutContext<'_>,
    images: &[ImageRef],
    x: f32,
    y: f32,
    width: f32,
    output: &mut Output,
    margin: f32,
) -> Result<Extent> {
    let unit = context.fonts.unit();
    let gap = 8.0 * unit;
    let mut cursor = y;
    for image in images {
        let path = resolve_image_path(&image.dest, context.document_dir);
        let size = path.as_deref().and_then(|path| (context.image_size)(path));
        let rect = match size {
            Some((image_width, image_height)) if image_width > 0 => {
                let scale = (width / image_width as f32).min(1.0);
                RectF::new(x, cursor, x + image_width as f32 * scale, cursor + image_height as f32 * scale)
            }
            _ => RectF::new(x, cursor, x + (240.0 * unit).min(width), cursor + 48.0 * unit),
        };
        let alt_text = if image.alt.is_empty() { "image" } else { &image.alt };
        let alt = context.plain_layout(alt_text, false, context.fonts.body_size, DWRITE_FONT_WEIGHT_NORMAL, (rect.width() - 16.0 * unit).max(1.0))?;
        output.images.push(ImageSlot { path, rect, alt });
        output.ops.push(DrawOp::Image { slot: output.images.len() - 1 });
        cursor = rect.bottom + gap;
    }
    Ok(Extent { content: (cursor - y - gap).max(0.0), margin })
}

fn alignment(align: CellAlign) -> DWRITE_TEXT_ALIGNMENT {
    match align {
        CellAlign::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
        CellAlign::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
        CellAlign::None | CellAlign::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
    }
}

fn text_range(start: u32, end: u32) -> DWRITE_TEXT_RANGE {
    DWRITE_TEXT_RANGE { startPosition: start, length: end.saturating_sub(start) }
}

fn metrics(layout: &IDWriteTextLayout) -> Result<DWRITE_TEXT_METRICS> {
    let mut value = DWRITE_TEXT_METRICS::default();
    unsafe { layout.GetMetrics(&mut value) }.map_err(hresult_error)?;
    Ok(value)
}

fn range_rects(layout: &IDWriteTextLayout, start: u32, end: u32, x: f32, y: f32) -> Result<Vec<RectF>> {
    let length = end.saturating_sub(start);
    if length == 0 {
        return Ok(Vec::new());
    }
    let mut count = 0_u32;
    // The first call only reports how many rectangles the range needs.
    let _ = unsafe { layout.HitTestTextRange(start, length, x, y, None, &mut count) };
    let mut hits = vec![DWRITE_HIT_TEST_METRICS::default(); count as usize];
    unsafe { layout.HitTestTextRange(start, length, x, y, Some(&mut hits), &mut count) }
        .map_err(hresult_error)?;
    Ok(hits
        .iter()
        .take(count as usize)
        .map(|hit| RectF::new(hit.left, hit.top, hit.left + hit.width, hit.top + hit.height))
        .collect())
}
```

If `SetDrawingEffect` rejects `&ID2D1SolidColorBrush` for its `Param<IUnknown>` bound, pass `&self.brushes.get(role).cast::<windows::core::IUnknown>().map_err(hresult_error)?` instead. Run `cargo fmt` after pasting; the long lines above are formatted by rustfmt.

- [ ] **Step 4: Run tests, Clippy, commit**

Run: `cargo test --lib preview::layout -- --test-threads=1` then `cargo clippy --all-targets`
Expected: PASS.

```bash
git add src/preview/mod.rs src/preview/render.rs src/preview/layout.rs
git commit -m "feat: DirectWrite block layout for the Markdown preview"
```

---

### Task 10: Image decoding off the UI thread

**Files:**
- Create: `src/preview/images.rs`
- Modify: `src/preview/mod.rs` (add `pub mod images;`)

**Interfaces:**
- Consumes: `hresult_error` (Task 8).
- Produces:
  - `pub const MAX_IMAGE_PIXELS: u64 = 64_000_000;`
  - `pub struct DecodedImage { pub width: u32, pub height: u32, pub pixels: Vec<u8> }` (premultiplied BGRA, stride `width * 4`)
  - `pub fn decode_image(path: &Path, max_width: u32) -> Result<DecodedImage>` (`max_width == 0` means no limit)
  - `pub struct ImageCache` with `new(notify: HWND, message: u32) -> Self`, `request(&mut self, path: &Path, max_width: u32)`, `drain(&mut self) -> bool`, `size(&self, path: &Path) -> Option<(u32, u32)>`, `is_failed(&self, path: &Path) -> bool`, `bitmap(&mut self, target: &ID2D1RenderTarget, path: &Path) -> Option<ID2D1Bitmap>`, `release_bitmaps(&mut self)`, `clear(&mut self)`

- [ ] **Step 1: Write the failing tests**

Create `src/preview/images.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// A 2x2 opaque red PNG.
    const PNG_2X2: [u8; 125] = [
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x06, 0x00, 0x00, 0x00, 0x72,
        0xb6, 0x0d, 0x24, 0x00, 0x00, 0x00, 0x01, 0x73, 0x52, 0x47, 0x42, 0x00, 0xae, 0xce, 0x1c,
        0xe9, 0x00, 0x00, 0x00, 0x04, 0x67, 0x41, 0x4d, 0x41, 0x00, 0x00, 0xb1, 0x8f, 0x0b, 0xfc,
        0x61, 0x05, 0x00, 0x00, 0x00, 0x09, 0x70, 0x48, 0x59, 0x73, 0x00, 0x00, 0x0e, 0xc3, 0x00,
        0x00, 0x0e, 0xc3, 0x01, 0xc7, 0x6f, 0xa8, 0x64, 0x00, 0x00, 0x00, 0x12, 0x49, 0x44, 0x41,
        0x54, 0x18, 0x57, 0x63, 0xf8, 0xcf, 0xc0, 0xf0, 0x1f, 0x84, 0xa1, 0x0c, 0x86, 0xff, 0x00,
        0x47, 0xca, 0x07, 0xf9, 0xac, 0x78, 0x42, 0xbc, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e,
        0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    fn write_png(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("fastpad-{name}-{}.png", std::process::id()));
        std::fs::write(&path, PNG_2X2).unwrap();
        path
    }

    #[test]
    fn decodes_png_to_premultiplied_bgra() {
        let path = write_png("decode");
        let image = decode_image(&path, 0).unwrap();
        assert_eq!((image.width, image.height), (2, 2));
        assert_eq!(image.pixels.len(), 16);
        assert_eq!(&image.pixels[..4], &[0, 0, 255, 255]);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn scales_down_to_the_maximum_width() {
        let path = write_png("scale");
        let image = decode_image(&path, 1).unwrap();
        assert_eq!((image.width, image.height), (1, 1));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn missing_files_fail() {
        assert!(decode_image(Path::new(r"C:\definitely\missing.png"), 0).is_err());
    }

    #[test]
    fn the_cache_decodes_on_a_worker_and_reports_sizes() {
        let path = write_png("cache");
        let missing = PathBuf::from(r"C:\definitely\missing.png");
        let mut cache = ImageCache::new(std::ptr::null_mut(), 0);
        cache.request(&path, 0);
        cache.request(&missing, 0);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while cache.size(&path).is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
            cache.drain();
        }
        assert_eq!(cache.size(&path), Some((2, 2)));
        assert!(cache.is_failed(&missing));
        let _ = std::fs::remove_file(path);
    }
}
```

Run: `cargo test --lib preview::images -- --test-threads=1`
Expected: FAIL to compile.

- [ ] **Step 2: Implement**

```rust
//! Image decoding off the UI thread. A short-lived worker decodes queued files with WIC into
//! premultiplied BGRA pixels and posts `message` to `notify`; the UI thread drains results and
//! creates Direct2D bitmaps on demand, keeping the pixels so bitmaps can be rebuilt after device
//! loss.

use crate::preview::dwrite::hresult_error;
use crate::{FastPadError, Result};
use std::collections::{HashMap, VecDeque};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use windows::Win32::Foundation::GENERIC_READ;
use windows::Win32::Graphics::Direct2D::Common::{D2D_SIZE_U, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT};
use windows::Win32::Graphics::Direct2D::{D2D1_BITMAP_PROPERTIES, ID2D1Bitmap, ID2D1RenderTarget};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICBitmapSource,
    IWICImagingFactory, WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant,
    WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::core::{Interface, PCWSTR};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

pub const MAX_IMAGE_PIXELS: u64 = 64_000_000;

pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

struct ComScope(bool);

impl ComScope {
    fn enter() -> Self {
        Self(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
    }
}

impl Drop for ComScope {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

pub fn decode_image(path: &Path, max_width: u32) -> Result<DecodedImage> {
    let _com = ComScope::enter();
    let wide = path.as_os_str().encode_wide().chain(Some(0)).collect::<Vec<u16>>();
    unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).map_err(hresult_error)?;
        let decoder = factory
            .CreateDecoderFromFilename(PCWSTR(wide.as_ptr()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand)
            .map_err(hresult_error)?;
        let frame = decoder.GetFrame(0).map_err(hresult_error)?;
        let (mut width, mut height) = (0_u32, 0_u32);
        frame.GetSize(&mut width, &mut height).map_err(hresult_error)?;
        if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS {
            return Err(FastPadError::Invariant("image is empty or larger than 64 megapixels"));
        }
        let mut source: IWICBitmapSource = frame.cast().map_err(hresult_error)?;
        if max_width > 0 && width > max_width {
            let scaled_height = ((u64::from(height) * u64::from(max_width)) / u64::from(width)).max(1) as u32;
            let scaler = factory.CreateBitmapScaler().map_err(hresult_error)?;
            scaler.Initialize(&source, max_width, scaled_height, WICBitmapInterpolationModeFant).map_err(hresult_error)?;
            source = scaler.cast().map_err(hresult_error)?;
            (width, height) = (max_width, scaled_height);
        }
        let converter = factory.CreateFormatConverter().map_err(hresult_error)?;
        converter
            .Initialize(&source, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)
            .map_err(hresult_error)?;
        let mut pixels = vec![0_u8; width as usize * height as usize * 4];
        converter.CopyPixels(std::ptr::null(), width * 4, &mut pixels).map_err(hresult_error)?;
        Ok(DecodedImage { width, height, pixels })
    }
}

enum EntryState {
    Pending,
    Ready(DecodedImage),
    Failed,
}

struct Entry {
    modified: Option<SystemTime>,
    state: EntryState,
    bitmap: Option<ID2D1Bitmap>,
}

#[derive(Default)]
struct Shared {
    queue: VecDeque<(PathBuf, u32)>,
    done: Vec<(PathBuf, Result<DecodedImage>)>,
    worker_running: bool,
}

pub struct ImageCache {
    entries: HashMap<PathBuf, Entry>,
    shared: Arc<Mutex<Shared>>,
    /// `HWND` as an integer so the worker closure is `Send`.
    notify: isize,
    message: u32,
}

impl ImageCache {
    pub fn new(notify: HWND, message: u32) -> Self {
        Self {
            entries: HashMap::new(),
            shared: Arc::new(Mutex::new(Shared::default())),
            notify: notify as isize,
            message,
        }
    }

    pub fn request(&mut self, path: &Path, max_width: u32) {
        let modified = std::fs::metadata(path).and_then(|metadata| metadata.modified()).ok();
        if let Some(entry) = self.entries.get(path)
            && entry.modified == modified
        {
            return;
        }
        if modified.is_none() {
            self.entries.insert(path.to_owned(), Entry { modified, state: EntryState::Failed, bitmap: None });
            return;
        }
        self.entries.insert(path.to_owned(), Entry { modified, state: EntryState::Pending, bitmap: None });
        let spawn = {
            let mut shared = self.shared.lock().unwrap_or_else(|error| error.into_inner());
            shared.queue.push_back((path.to_owned(), max_width));
            !std::mem::replace(&mut shared.worker_running, true)
        };
        if spawn {
            let shared = Arc::clone(&self.shared);
            let (notify, message) = (self.notify, self.message);
            std::thread::spawn(move || loop {
                let job = {
                    let mut state = shared.lock().unwrap_or_else(|error| error.into_inner());
                    let job = state.queue.pop_front();
                    if job.is_none() {
                        state.worker_running = false;
                    }
                    job
                };
                let Some((path, max_width)) = job else {
                    return;
                };
                let result = decode_image(&path, max_width);
                shared.lock().unwrap_or_else(|error| error.into_inner()).done.push((path, result));
                unsafe { PostMessageW(notify as HWND, message, 0, 0) };
            });
        }
    }

    /// Moves finished decodes into the cache; true when anything changed.
    pub fn drain(&mut self) -> bool {
        let done = std::mem::take(&mut self.shared.lock().unwrap_or_else(|error| error.into_inner()).done);
        let changed = !done.is_empty();
        for (path, result) in done {
            if let Some(entry) = self.entries.get_mut(&path) {
                entry.state = match result {
                    Ok(image) => EntryState::Ready(image),
                    Err(_) => EntryState::Failed,
                };
                entry.bitmap = None;
            }
        }
        changed
    }

    pub fn size(&self, path: &Path) -> Option<(u32, u32)> {
        match &self.entries.get(path)?.state {
            EntryState::Ready(image) => Some((image.width, image.height)),
            _ => None,
        }
    }

    pub fn is_failed(&self, path: &Path) -> bool {
        matches!(self.entries.get(path).map(|entry| &entry.state), Some(EntryState::Failed))
    }

    pub fn bitmap(&mut self, target: &ID2D1RenderTarget, path: &Path) -> Option<ID2D1Bitmap> {
        let entry = self.entries.get_mut(path)?;
        let EntryState::Ready(image) = &entry.state else {
            return None;
        };
        if entry.bitmap.is_none() {
            let properties = D2D1_BITMAP_PROPERTIES {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
            };
            entry.bitmap = unsafe {
                target.CreateBitmap(
                    D2D_SIZE_U { width: image.width, height: image.height },
                    Some(image.pixels.as_ptr().cast()),
                    image.width * 4,
                    &properties,
                )
            }
            .ok();
        }
        entry.bitmap.clone()
    }

    /// Device loss: bitmaps belong to the lost device; pixels stay for rebuilding.
    pub fn release_bitmaps(&mut self) {
        for entry in self.entries.values_mut() {
            entry.bitmap = None;
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}
```

- [ ] **Step 3: Run tests, Clippy, import guard, commit**

Run: `cargo test --lib preview::images -- --test-threads=1`, `cargo clippy --all-targets`, `cargo build --release`, `cargo test --test markdown_preview binary_does_not_statically_import -- --test-threads=1`
Expected: PASS. WIC is created through `CoCreateInstance`, so `windowscodecs.dll` must not appear in imports.

```bash
git add src/preview/mod.rs src/preview/images.rs
git commit -m "feat: decode preview images with WIC on a worker thread"
```

---
### Task 11: The preview window

**Files:**
- Modify: `src/window/messages.rs` (preview messages)
- Modify: `src/preview/render.rs` (append `draw_ops`)
- Create: `src/preview/view.rs`
- Modify: `src/preview/mod.rs` (add `pub mod view;`)

**Interfaces:**
- Consumes: everything in `src/preview/` from Tasks 3–10.
- Produces in `crate::window::messages` (re-exported from `crate::window` like the existing messages):
  - `WM_FASTPAD_DIAGNOSTIC_PREVIEW = WM_APP + 0x41`
  - `WM_FASTPAD_PREVIEW_SCROLLED = WM_APP + 0x50` (wparam: top source line after a user scroll)
  - `WM_FASTPAD_PREVIEW_LINK = WM_APP + 0x51` (lparam: `Box<String>` raw pointer; receiver frees with `Box::from_raw`)
  - `WM_FASTPAD_PREVIEW_HOVER = WM_APP + 0x52` (lparam: `Box<Option<String>>` raw pointer; receiver frees)
  - `WM_FASTPAD_PREVIEW_REFRESH = WM_APP + 0x53`
  - `WM_FASTPAD_PREVIEW_ESCAPE = WM_APP + 0x54`
  - `WM_FASTPAD_PREVIEW_IMAGE = WM_APP + 0x55` (preview window internal)
  - `WM_FASTPAD_PREVIEW_PARSED = WM_APP + 0x56` (worker parse result; lparam: `Box<ParsedPreview>`)
  - `WM_FASTPAD_PREVIEW_ACTIVATE = WM_APP + 0x57` (to the preview window; wparam: index into its accessible link snapshot)
- Produces in `render.rs`: `pub fn draw_ops(target: &ID2D1RenderTarget, brushes: &Brushes, ops: &[DrawOp], slots: &[ImageSlot], cache: &mut ImageCache, x: f32, y: f32, h_offset: f32)`
- Produces in `view.rs`:
  - `#[derive(Clone, Copy, Debug, Default)] pub struct PreviewStats { pub block_count: usize, pub revision: u64, pub first_frame_micros: u64, pub last_update_micros: u64 }`
  - `pub fn content_frame(view_width: f32, centered: bool) -> (f32, f32)` (left, width)
  - `pub struct PreviewView` with:
    - `pub fn create(parent: HWND, graphics: Rc<Graphics>, colors: PreviewColors, fonts: PreviewFonts) -> Result<Self>`
    - `pub fn hwnd(&self) -> HWND`
    - `pub fn destroy(self)`
    - `pub fn replace_document(&self, document: PreviewDocument, document_dir: Option<PathBuf>, started: Instant)`
    - `pub fn apply_edits<S: SourceText + ?Sized>(&self, source: &S, edits: &[Edit], started: Instant, allow_full_parse: bool) -> Option<Update>` (`None` only when `allow_full_parse` is false and a full parse is needed)
    - `pub fn reparse<S: SourceText + ?Sized>(&self, source: &S, started: Instant) -> Update`
    - `pub fn set_appearance(&self, colors: PreviewColors, fonts: PreviewFonts, dark_scrollbar: bool)`
    - `pub fn set_document_dir(&self, document_dir: Option<PathBuf>)`
    - `pub fn set_centered(&self, centered: bool)`
    - `pub fn set_paused(&self, paused: bool)`
    - `pub fn set_live_resize(&self, live: bool)`
    - `pub fn mark_opened(&self, at: Instant)`
    - `pub fn scroll_to_line(&self, line: usize)`
    - `pub fn top_line(&self) -> usize`
    - `pub fn scroll_to_anchor(&self, anchor: &str) -> bool`
    - `pub fn stats(&self) -> PreviewStats`
    - `#[derive(Clone, Debug, PartialEq, Eq)] pub struct VisibleLink { pub text: String, pub dest: String, pub rect: RECT }` (client pixels)
    - `pub fn visible_links(&self) -> Vec<VisibleLink>` (links currently on screen, in document order)
    - `pub fn accessible_links(&self) -> Arc<RwLock<Vec<VisibleLink>>>` (snapshot refreshed after every paint; safe to read from any thread, used by Task 17)

- [ ] **Step 1: Add the messages**

Append to `src/window/messages.rs` after `WM_FASTPAD_DIAGNOSTIC_JSON_COUNT`:

```rust
// Not part of the deferred chain: answers only under --diagnostic; wparam selects a preview value.
pub const WM_FASTPAD_DIAGNOSTIC_PREVIEW: u32 = WM_APP + 0x41;
// Preview window to main window. Payload-carrying messages pass a `Box` the receiver frees.
pub const WM_FASTPAD_PREVIEW_SCROLLED: u32 = WM_APP + 0x50;
pub const WM_FASTPAD_PREVIEW_LINK: u32 = WM_APP + 0x51;
pub const WM_FASTPAD_PREVIEW_HOVER: u32 = WM_APP + 0x52;
pub const WM_FASTPAD_PREVIEW_REFRESH: u32 = WM_APP + 0x53;
pub const WM_FASTPAD_PREVIEW_ESCAPE: u32 = WM_APP + 0x54;
pub const WM_FASTPAD_PREVIEW_IMAGE: u32 = WM_APP + 0x55;
pub const WM_FASTPAD_PREVIEW_PARSED: u32 = WM_APP + 0x56;
pub const WM_FASTPAD_PREVIEW_ACTIVATE: u32 = WM_APP + 0x57;
```

If `src/window/mod.rs` re-exports messages by name rather than with a glob, add these names to that re-export.

- [ ] **Step 2: Append `draw_ops` to `render.rs`**

```rust
use crate::preview::images::ImageCache;
use crate::preview::layout::{DrawOp, ImageSlot};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_ALIASED, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
    D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ROUNDED_RECT,
};
use windows_numerics::Vector2;

/// Paints `ops` with their block origin at (`x`, `y`). Scrollable groups draw clipped and shifted
/// left by `h_offset`.
#[allow(clippy::too_many_arguments)]
pub fn draw_ops(
    target: &ID2D1RenderTarget,
    brushes: &Brushes,
    ops: &[DrawOp],
    slots: &[ImageSlot],
    cache: &mut ImageCache,
    x: f32,
    y: f32,
    h_offset: f32,
) {
    for op in ops {
        unsafe {
            match op {
                DrawOp::Text { layout, x: left, y: top, role } => target.DrawTextLayout(
                    Vector2 { X: x + left, Y: y + top },
                    layout,
                    brushes.get(*role),
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                ),
                DrawOp::Fill { rect, role } => {
                    target.FillRectangle(&rect.offset(x, y).to_d2d(), brushes.get(*role))
                }
                DrawOp::RoundedFill { rect, radius, role } => target.FillRoundedRectangle(
                    &D2D1_ROUNDED_RECT { rect: rect.offset(x, y).to_d2d(), radiusX: *radius, radiusY: *radius },
                    brushes.get(*role),
                ),
                DrawOp::Stroke { rect, role } => target.DrawRectangle(
                    &rect.offset(x, y).inflate(-0.5).to_d2d(),
                    brushes.get(*role),
                    1.0,
                    None,
                ),
                DrawOp::Checkbox { rect, checked } => {
                    let rect = rect.offset(x, y);
                    target.DrawRectangle(&rect.inflate(-0.5).to_d2d(), brushes.get(ColorRole::Border), 1.0, None);
                    if *checked {
                        target.FillRectangle(&rect.inflate(-3.0).to_d2d(), brushes.get(ColorRole::Link));
                    }
                }
                DrawOp::Image { slot } => {
                    let Some(slot) = slots.get(*slot) else { continue };
                    let rect = slot.rect.offset(x, y);
                    match slot.path.as_deref().and_then(|path| cache.bitmap(target, path)) {
                        Some(bitmap) => target.DrawBitmap(
                            &bitmap,
                            Some(&rect.to_d2d()),
                            1.0,
                            D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                            None,
                        ),
                        None => {
                            target.DrawRectangle(&rect.inflate(-0.5).to_d2d(), brushes.get(ColorRole::Border), 1.0, None);
                            target.DrawTextLayout(
                                Vector2 { X: rect.left + 8.0, Y: rect.top + 8.0 },
                                &slot.alt,
                                brushes.get(ColorRole::Muted),
                                D2D1_DRAW_TEXT_OPTIONS_NONE,
                            );
                        }
                    }
                }
                DrawOp::Scrollable { clip, ops, .. } => {
                    target.PushAxisAlignedClip(&clip.offset(x, y).to_d2d(), D2D1_ANTIALIAS_MODE_ALIASED);
                    draw_ops(target, brushes, ops, slots, cache, x - h_offset, y, 0.0);
                    target.PopAxisAlignedClip();
                }
            }
        }
    }
}
```

- [ ] **Step 3: Write the failing view tests**

Create `src/preview/view.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::theme::Theme;
    use crate::preview::colors::preview_colors;
    use crate::preview::render::TestWindow;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_END, VK_ESCAPE};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MSG, MoveWindow, PM_REMOVE, PeekMessageW, SendMessageW, WM_KEYDOWN, WM_LBUTTONDOWN,
        WM_LBUTTONUP, WM_PAINT,
    };

    fn view_with(parent: &TestWindow, source: &str) -> PreviewView {
        let graphics = Rc::new(Graphics::load().unwrap());
        let view = PreviewView::create(
            parent.0,
            graphics,
            preview_colors(Theme::Light, false),
            PreviewFonts::from_settings("Consolas", 11),
        )
        .unwrap();
        unsafe {
            use windows_sys::Win32::UI::WindowsAndMessaging::{SW_SHOWNOACTIVATE, ShowWindow};
            ShowWindow(parent.0, SW_SHOWNOACTIVATE);
            MoveWindow(view.hwnd(), 0, 0, 400, 300, 0);
            ShowWindow(view.hwnd(), SW_SHOWNOACTIVATE);
        }
        view.mark_opened(Instant::now());
        view.replace_document(PreviewDocument::parse(source), None, Instant::now());
        // The view paints without BeginPaint, so a direct WM_PAINT renders synchronously.
        unsafe { SendMessageW(view.hwnd(), WM_PAINT, 0, 0) };
        view
    }

    fn take_posted(parent: &TestWindow, message: u32) -> Option<MSG> {
        let mut msg = MSG::default();
        (unsafe { PeekMessageW(&mut msg, parent.0, message, message, PM_REMOVE) } != 0).then_some(msg)
    }

    #[test]
    fn content_is_padded_and_capped_when_centered() {
        assert_eq!(content_frame(400.0, false), (16.0, 368.0));
        assert_eq!(content_frame(2000.0, true), (510.0, 980.0));
        assert_eq!(content_frame(500.0, true), (16.0, 468.0));
    }

    #[test]
    fn painting_a_document_records_stats() {
        let parent = TestWindow::new(800, 600);
        let view = view_with(&parent, "# A\n\ntext\n");
        let stats = view.stats();
        assert_eq!(stats.block_count, 2);
        assert!(stats.revision > 0);
        assert!(stats.first_frame_micros > 0);
        view.destroy();
    }

    #[test]
    fn scrolling_to_a_line_reports_the_same_top_line() {
        let parent = TestWindow::new(800, 600);
        let source = (0..200).map(|index| format!("para {index}\n\n")).collect::<String>();
        let view = view_with(&parent, &source);
        view.scroll_to_line(100);
        assert_eq!(view.top_line(), 100);
        view.destroy();
    }

    #[test]
    fn end_key_scrolls_to_the_bottom_and_reports_the_scroll() {
        let parent = TestWindow::new(800, 600);
        let source = (0..200).map(|index| format!("para {index}\n\n")).collect::<String>();
        let view = view_with(&parent, &source);
        unsafe { SendMessageW(view.hwnd(), WM_KEYDOWN, VK_END as usize, 0) };
        assert!(view.top_line() > 0);
        assert!(take_posted(&parent, WM_FASTPAD_PREVIEW_SCROLLED).is_some());
        view.destroy();
    }

    #[test]
    fn escape_is_forwarded_to_the_parent() {
        let parent = TestWindow::new(800, 600);
        let view = view_with(&parent, "text\n");
        unsafe { SendMessageW(view.hwnd(), WM_KEYDOWN, VK_ESCAPE as usize, 0) };
        assert!(take_posted(&parent, WM_FASTPAD_PREVIEW_ESCAPE).is_some());
        view.destroy();
    }

    #[test]
    fn clicking_a_link_posts_its_destination() {
        let parent = TestWindow::new(800, 600);
        let view = view_with(&parent, "[site](https://x.dev)\n");
        let rect = view.visible_links().into_iter().next().expect("a laid-out link").rect;
        assert_eq!(view.accessible_links().read().unwrap()[0].text, "site");
        let point = (((rect.top + rect.bottom) / 2) << 16 | ((rect.left + rect.right) / 2)) as isize;
        unsafe {
            SendMessageW(view.hwnd(), WM_LBUTTONDOWN, 0, point);
            SendMessageW(view.hwnd(), WM_LBUTTONUP, 0, point);
        }
        let message = take_posted(&parent, WM_FASTPAD_PREVIEW_LINK).expect("link message");
        let dest = unsafe { Box::from_raw(message.lParam as *mut String) };
        assert_eq!(*dest, "https://x.dev");
        view.destroy();
    }

    #[test]
    fn anchors_scroll_to_their_heading() {
        let parent = TestWindow::new(800, 600);
        let mut source = (0..100).map(|index| format!("para {index}\n\n")).collect::<String>();
        source.push_str("## Deep Heading\n");
        let view = view_with(&parent, &source);
        assert!(view.scroll_to_anchor("deep-heading"));
        assert!(view.top_line() >= 150);
        assert!(!view.scroll_to_anchor("missing"));
        view.destroy();
    }
}
```

Add `pub mod view;` to `src/preview/mod.rs`.

Run: `cargo test --lib preview::view -- --test-threads=1`
Expected: FAIL to compile.

- [ ] **Step 4: Implement `view.rs`**

Above the tests:

```rust
//! The `FastPadPreview` child window. It owns the preview model, lays out only the blocks that
//! scroll into view, and paints them with Direct2D. User actions are posted to the parent (never
//! sent), so the parent can call back into the view without re-entering a borrowed state.

use crate::Result;
use crate::platform::{last_error, wide_null};
use crate::preview::colors::{ColorRole, PreviewColors};
use crate::preview::dwrite::Graphics;
use crate::preview::heights::{HeightIndex, estimate_height, line_for_offset, offset_for_line};
use crate::preview::images::ImageCache;
use crate::preview::incremental::{Edit, PreviewDocument, SourceText, Update};
use crate::preview::layout::{LaidBlock, LayoutContext, PreviewFonts, layout_block};
use crate::preview::links::SlugSet;
use crate::preview::model::BlockKind;
use crate::preview::render::{Brushes, color_f, create_hwnd_target, draw_ops};
use crate::window::messages::{
    WM_FASTPAD_PREVIEW_ACTIVATE, WM_FASTPAD_PREVIEW_ESCAPE, WM_FASTPAD_PREVIEW_HOVER,
    WM_FASTPAD_PREVIEW_IMAGE, WM_FASTPAD_PREVIEW_LINK, WM_FASTPAD_PREVIEW_REFRESH,
    WM_FASTPAD_PREVIEW_SCROLLED,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, RwLock};
use std::time::Instant;
use windows::Win32::Foundation::D2DERR_RECREATE_TARGET;
use windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget;
use windows::Win32::Graphics::Direct2D::Common::D2D_SIZE_U;
use windows::Win32::Graphics::DirectWrite::{DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL};
use windows_numerics::{Matrix3x2, Vector2};
use windows_sys::Win32::Foundation::{
    ERROR_CLASS_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, RECT, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{InvalidateRect, ValidateRect};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::{SetWindowTheme, WM_MOUSELEAVE};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_DOWN, VK_END,
    VK_ESCAPE, VK_HOME, VK_NEXT, VK_PRIOR, VK_RETURN, VK_SHIFT, VK_TAB, VK_UP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DLGC_WANTALLKEYS, DefWindowProcW, DestroyWindow, GWLP_USERDATA,
    GetClientRect, GetParent, GetWindowLongPtrW, HTCLIENT, IDC_ARROW, IDC_HAND, LoadCursorW,
    PostMessageW, RegisterClassW, SB_BOTTOM, SB_LINEDOWN, SB_LINEUP, SB_PAGEDOWN,
    SB_PAGEUP, SB_THUMBTRACK, SB_TOP, SB_VERT, SCROLLINFO, SIF_ALL, SIF_TRACKPOS, SetCursor,
    SetScrollInfo, GetScrollInfo, SetWindowLongPtrW, WM_ERASEBKGND, WM_GETDLGCODE, WM_KEYDOWN,
    WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL,
    WM_NCDESTROY, WM_PAINT, WM_SETCURSOR, WM_SETFOCUS, WM_SIZE, WM_VSCROLL, WNDCLASSW, WS_CHILD,
    WS_CLIPSIBLINGS, WS_TABSTOP, WS_VSCROLL,
};

const CLASS_NAME: &str = "FastPadPreview";
/// `MK_SHIFT` from WinUser.h; its windows-sys home needs a feature FastPad does not enable.
const MK_SHIFT: u32 = 0x0004;
const PADDING: f32 = 16.0;
const MAX_CENTERED_WIDTH: f32 = 980.0;
const PAUSED_BAR_HEIGHT: f32 = 32.0;
const PAUSED_TEXT: &str =
    "Live preview is paused for this large file. Click here to refresh the preview.";

#[derive(Clone, Copy, Debug, Default)]
pub struct PreviewStats {
    pub block_count: usize,
    pub revision: u64,
    pub first_frame_micros: u64,
    pub last_update_micros: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisibleLink {
    pub text: String,
    pub dest: String,
    /// Client pixels of the link's first line.
    pub rect: RECT,
}

pub fn content_frame(view_width: f32, centered: bool) -> (f32, f32) {
    let available = (view_width - 2.0 * PADDING).max(1.0);
    if centered && available > MAX_CENTERED_WIDTH {
        (PADDING + (available - MAX_CENTERED_WIDTH) / 2.0, MAX_CENTERED_WIDTH)
    } else {
        (PADDING, available)
    }
}

struct ViewState {
    hwnd: HWND,
    graphics: Rc<Graphics>,
    target: Option<ID2D1HwndRenderTarget>,
    brushes: Option<Brushes>,
    colors: PreviewColors,
    fonts: PreviewFonts,
    document: PreviewDocument,
    document_dir: Option<PathBuf>,
    layouts: Vec<Option<LaidBlock>>,
    heights: HeightIndex,
    layout_width: f32,
    scroll_y: f32,
    h_scroll: HashMap<usize, f32>,
    hover: Option<(usize, usize)>,
    pressed: Option<(usize, usize)>,
    focus: Option<(usize, usize)>,
    tracking_mouse: bool,
    images: ImageCache,
    centered: bool,
    paused: bool,
    live_resize: bool,
    anchors: Vec<(String, usize)>,
    stats: PreviewStats,
    opened_at: Option<Instant>,
    update_started: Option<Instant>,
    /// On-screen links as of the last paint, shared with the accessibility provider.
    accessible: Arc<RwLock<Vec<VisibleLink>>>,
}

/// A handle to a preview window; copies are cheap and all refer to the same window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreviewView {
    hwnd: HWND,
}

impl PreviewView {
    pub fn create(
        parent: HWND,
        graphics: Rc<Graphics>,
        colors: PreviewColors,
        fonts: PreviewFonts,
    ) -> Result<Self> {
        register_class()?;
        let class = wide_null(CLASS_NAME);
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                std::ptr::null(),
                WS_CHILD | WS_CLIPSIBLINGS | WS_TABSTOP | WS_VSCROLL,
                0,
                0,
                0,
                0,
                parent,
                std::ptr::null_mut(),
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null(),
            )
        };
        if hwnd.is_null() {
            return Err(last_error());
        }
        let state = Box::new(ViewState {
            hwnd,
            graphics,
            target: None,
            brushes: None,
            colors,
            fonts,
            document: PreviewDocument::default(),
            document_dir: None,
            layouts: Vec::new(),
            heights: HeightIndex::default(),
            layout_width: 0.0,
            scroll_y: 0.0,
            h_scroll: HashMap::new(),
            hover: None,
            pressed: None,
            focus: None,
            tracking_mouse: false,
            images: ImageCache::new(hwnd, WM_FASTPAD_PREVIEW_IMAGE),
            centered: false,
            paused: false,
            live_resize: false,
            anchors: Vec::new(),
            stats: PreviewStats::default(),
            opened_at: None,
            update_started: None,
            accessible: Arc::new(RwLock::new(Vec::new())),
        });
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize) };
        Ok(Self { hwnd })
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    pub fn destroy(self) {
        unsafe { DestroyWindow(self.hwnd) };
    }

    fn with<R>(&self, action: impl FnOnce(&mut ViewState) -> R) -> Option<R> {
        with_state(self.hwnd, action)
    }

    pub fn replace_document(&self, document: PreviewDocument, document_dir: Option<PathBuf>, started: Instant) {
        self.with(|state| {
            state.document = document;
            state.document_dir = document_dir;
            state.scroll_y = 0.0;
            state.update_started = Some(started);
            accept_update(state, Update::Full);
        });
    }

    pub fn apply_edits<S: SourceText + ?Sized>(
        &self,
        source: &S,
        edits: &[Edit],
        started: Instant,
        allow_full_parse: bool,
    ) -> Option<Update> {
        self.with(|state| {
            let update = if allow_full_parse {
                Some(state.document.apply(source, edits))
            } else {
                state.document.try_apply(source, edits)
            }?;
            state.update_started = Some(started);
            accept_update(state, update.clone());
            Some(update)
        })
        .flatten()
    }

    pub fn reparse<S: SourceText + ?Sized>(&self, source: &S, started: Instant) -> Update {
        self.with(|state| {
            let update = state.document.reparse(source);
            state.update_started = Some(started);
            accept_update(state, update.clone());
            update
        })
        .unwrap_or(Update::Unchanged)
    }

    pub fn set_appearance(&self, colors: PreviewColors, fonts: PreviewFonts, dark_scrollbar: bool) {
        self.with(|state| {
            state.colors = colors;
            state.fonts = fonts;
            state.brushes = None;
            reset_layouts(state);
        });
        let theme = wide_null(if dark_scrollbar { "DarkMode_Explorer" } else { "Explorer" });
        unsafe { SetWindowTheme(self.hwnd, theme.as_ptr(), std::ptr::null()) };
        invalidate(self.hwnd);
    }

    pub fn set_document_dir(&self, document_dir: Option<PathBuf>) {
        self.with(|state| {
            if state.document_dir != document_dir {
                state.document_dir = document_dir;
                reset_layouts(state);
            }
        });
        invalidate(self.hwnd);
    }

    pub fn set_centered(&self, centered: bool) {
        self.with(|state| state.centered = centered);
        invalidate(self.hwnd);
    }

    pub fn set_paused(&self, paused: bool) {
        self.with(|state| state.paused = paused);
        invalidate(self.hwnd);
    }

    pub fn set_live_resize(&self, live: bool) {
        self.with(|state| state.live_resize = live);
        invalidate(self.hwnd);
    }

    pub fn mark_opened(&self, at: Instant) {
        self.with(|state| state.opened_at = Some(at));
    }

    pub fn scroll_to_line(&self, line: usize) {
        self.with(|state| {
            let ViewState { document, heights, .. } = state;
            let offset = offset_for_line(&document.blocks, heights, line);
            set_scroll(state, offset, false);
        });
    }

    pub fn top_line(&self) -> usize {
        self.with(|state| {
            let ViewState { document, heights, scroll_y, .. } = state;
            line_for_offset(&document.blocks, heights, *scroll_y)
        })
        .unwrap_or(0)
    }

    pub fn scroll_to_anchor(&self, anchor: &str) -> bool {
        self.with(|state| {
            let Some(index) = state.anchors.iter().find(|(slug, _)| slug == anchor).map(|(_, index)| *index) else {
                return false;
            };
            let top = state.heights.top(index);
            set_scroll(state, top, true);
            true
        })
        .unwrap_or(false)
    }

    pub fn stats(&self) -> PreviewStats {
        self.with(|state| state.stats).unwrap_or_default()
    }

    pub fn visible_links(&self) -> Vec<VisibleLink> {
        self.with(visible_links).unwrap_or_default()
    }

    pub fn accessible_links(&self) -> Arc<RwLock<Vec<VisibleLink>>> {
        self.with(|state| Arc::clone(&state.accessible)).unwrap_or_default()
    }
}

fn register_class() -> Result<()> {
    let class = wide_null(CLASS_NAME);
    let window_class = WNDCLASSW {
        lpfnWndProc: Some(preview_proc),
        hInstance: unsafe { GetModuleHandleW(std::ptr::null()) },
        hCursor: unsafe { LoadCursorW(std::ptr::null_mut(), IDC_ARROW) },
        lpszClassName: class.as_ptr(),
        ..Default::default()
    };
    if unsafe { RegisterClassW(&window_class) } == 0
        && unsafe { GetLastError() } != ERROR_CLASS_ALREADY_EXISTS
    {
        return Err(last_error());
    }
    Ok(())
}

fn with_state<R>(hwnd: HWND, action: impl FnOnce(&mut ViewState) -> R) -> Option<R> {
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut ViewState;
    // SAFETY: the pointer is owned by this window until WM_NCDESTROY, and every entry point runs on
    // the window's thread without sending messages while the borrow is live.
    (!pointer.is_null()).then(|| action(unsafe { &mut *pointer }))
}

fn invalidate(hwnd: HWND) {
    unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
}

fn dpi_scale(hwnd: HWND) -> f32 {
    unsafe { GetDpiForWindow(hwnd) }.max(96) as f32 / 96.0
}

fn view_size(hwnd: HWND) -> (f32, f32) {
    let mut rect = RECT::default();
    unsafe { GetClientRect(hwnd, &mut rect) };
    let scale = dpi_scale(hwnd);
    ((rect.right - rect.left) as f32 / scale, (rect.bottom - rect.top) as f32 / scale)
}

fn bar_height(state: &ViewState) -> f32 {
    if state.paused { PAUSED_BAR_HEIGHT } else { 0.0 }
}

fn estimates(state: &ViewState) -> Vec<f32> {
    let line_height = state.fonts.body_size * 1.5;
    let gap = 16.0 * state.fonts.unit();
    state.document.blocks.iter().map(|block| estimate_height(block.lines.len(), line_height, gap)).collect()
}

fn reset_layouts(state: &mut ViewState) {
    state.layouts = (0..state.document.blocks.len()).map(|_| None).collect();
    let estimates = estimates(state);
    state.heights.reset(estimates);
    state.hover = None;
    state.pressed = None;
    state.focus = None;
}

fn accept_update(state: &mut ViewState, update: Update) {
    match update {
        Update::Unchanged => return,
        Update::Full => {
            reset_layouts(state);
            state.h_scroll.clear();
        }
        Update::Replaced { old, new } => {
            state.layouts.splice(old.clone(), new.clone().map(|_| None));
            let all = estimates(state);
            state.heights.splice(old.clone(), &all[new.clone()]);
            let delta = new.len() as isize - old.len() as isize;
            state.h_scroll = state
                .h_scroll
                .drain()
                .filter_map(|(index, offset)| {
                    if index < old.start {
                        Some((index, offset))
                    } else if index >= old.end {
                        Some(((index as isize + delta) as usize, offset))
                    } else {
                        None
                    }
                })
                .collect();
            state.hover = None;
            state.pressed = None;
            state.focus = None;
        }
    }
    let mut slugs = SlugSet::default();
    state.anchors = state
        .document
        .blocks
        .iter()
        .enumerate()
        .filter_map(|(index, block)| match &block.kind {
            BlockKind::Heading { text, .. } => Some((slugs.unique(text.plain_text()), index)),
            _ => None,
        })
        .collect();
    state.stats.block_count = state.document.blocks.len();
    state.stats.revision = state.document.revision;
    invalidate(state.hwnd);
}

/// Lays out `indices` that have no layout yet; true when any height changed.
fn ensure_layouts(state: &mut ViewState, indices: &[usize]) -> Result<bool> {
    let ViewState { graphics, brushes, fonts, document, document_dir, images, layouts, heights, layout_width, .. } = state;
    let Some(brushes) = brushes.as_ref() else {
        return Ok(false);
    };
    let mut requests = Vec::new();
    let mut changed = false;
    {
        let sizes = |path: &Path| images.size(path);
        let context = LayoutContext::new(graphics, brushes, fonts, document_dir.as_deref(), &sizes)?;
        for &index in indices {
            if index >= document.blocks.len() || layouts[index].is_some() {
                continue;
            }
            let laid = layout_block(&context, &document.blocks[index].kind, *layout_width)?;
            for slot in &laid.images {
                if let Some(path) = &slot.path
                    && images.size(path).is_none()
                    && !images.is_failed(path)
                {
                    requests.push(path.clone());
                }
            }
            heights.set_measured(index, laid.height);
            layouts[index] = Some(laid);
            changed = true;
        }
    }
    for path in requests {
        images.request(&path, *layout_width as u32);
    }
    Ok(changed)
}

fn visible_indices(state: &mut ViewState, view_height: f32) -> Vec<usize> {
    let bottom = state.scroll_y + view_height;
    let mut index = state.heights.index_at(state.scroll_y);
    let mut indices = Vec::new();
    while index < state.document.blocks.len() && state.heights.top(index) < bottom {
        indices.push(index);
        index += 1;
    }
    indices
}

fn max_scroll(state: &mut ViewState, view_height: f32) -> f32 {
    (state.heights.total() - (view_height - bar_height(state))).max(0.0)
}

fn set_scroll(state: &mut ViewState, y: f32, user: bool) {
    let (_, view_height) = view_size(state.hwnd);
    let clamped = y.clamp(0.0, max_scroll(state, view_height));
    if (clamped - state.scroll_y).abs() < 0.01 {
        return;
    }
    state.scroll_y = clamped;
    invalidate(state.hwnd);
    if user {
        let ViewState { document, heights, scroll_y, hwnd, .. } = state;
        let line = line_for_offset(&document.blocks, heights, *scroll_y);
        unsafe { PostMessageW(GetParent(*hwnd), WM_FASTPAD_PREVIEW_SCROLLED, line, 0) };
    }
}

fn paint(state: &mut ViewState) -> Result<()> {
    let hwnd = state.hwnd;
    let mut client = RECT::default();
    unsafe { GetClientRect(hwnd, &mut client) };
    let (pixel_width, pixel_height) = ((client.right - client.left).max(1) as u32, (client.bottom - client.top).max(1) as u32);
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    if state.target.is_none() {
        let target = create_hwnd_target(&state.graphics, hwnd, pixel_width, pixel_height, dpi)?;
        state.brushes = None;
        state.target = Some(target);
    }
    if state.brushes.is_none() {
        let target = state.target.as_ref().expect("created above");
        state.brushes = Some(Brushes::create(target, &state.colors)?);
        reset_layouts(state);
    }
    let (view_width, view_height) = view_size(hwnd);
    let (content_left, content_width) = content_frame(view_width, state.centered);
    if (content_width - state.layout_width).abs() > 0.5 && (!state.live_resize || state.layout_width == 0.0) {
        state.layout_width = content_width;
        reset_layouts(state);
    }
    let bar = bar_height(state);
    for _ in 0..3 {
        let anchor = state.heights.anchor(state.scroll_y);
        let visible = visible_indices(state, view_height - bar);
        if !ensure_layouts(state, &visible)? {
            break;
        }
        state.scroll_y = state.heights.scroll_for_anchor(anchor);
    }
    state.scroll_y = state.scroll_y.clamp(0.0, max_scroll(state, view_height));

    let visible = visible_indices(state, view_height - bar);
    let tops = visible.iter().map(|index| state.heights.top(*index)).collect::<Vec<_>>();
    let ViewState { target, brushes, layouts, images, h_scroll, focus, scroll_y, colors, graphics, fonts, paused, .. } = state;
    let target = target.as_ref().expect("created above");
    let brushes = brushes.as_ref().expect("created above");
    let result = unsafe {
        target.BeginDraw();
        target.SetTransform(&Matrix3x2::identity());
        target.Clear(Some(&color_f(colors.background)));
        for (index, top) in visible.iter().zip(tops) {
            let Some(laid) = layouts[*index].as_ref() else { continue };
            let y = top - *scroll_y + bar;
            let offset = h_scroll.get(index).copied().unwrap_or(0.0);
            draw_ops(target, brushes, &laid.ops, &laid.images, images, content_left, y, offset);
            if let Some((focus_block, focus_link)) = *focus
                && focus_block == *index
                && let Some(link) = laid.links.get(focus_link)
            {
                let shift = if link.scrolls { offset } else { 0.0 };
                for rect in &link.rects {
                    target.DrawRectangle(&rect.offset(content_left - shift, y).inflate(2.0).to_d2d(), brushes.get(ColorRole::Focus), 2.0, None);
                }
            }
        }
        if *paused {
            let bar_rect = crate::preview::render::RectF::new(0.0, 0.0, view_width, PAUSED_BAR_HEIGHT);
            target.FillRectangle(&bar_rect.to_d2d(), brushes.get(ColorRole::CodeBackground));
            if let Ok(format) = graphics.text_format(&fonts.body_family, fonts.body_size, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_STYLE_NORMAL) {
                let text = PAUSED_TEXT.encode_utf16().collect::<Vec<_>>();
                if let Ok(layout) = graphics.dwrite.CreateTextLayout(&text, &format, view_width - 2.0 * PADDING, PAUSED_BAR_HEIGHT) {
                    target.DrawTextLayout(Vector2 { X: PADDING, Y: 6.0 }, &layout, brushes.get(ColorRole::Text), windows::Win32::Graphics::Direct2D::D2D1_DRAW_TEXT_OPTIONS_NONE);
                }
            }
        }
        target.EndDraw(None, None)
    };
    match result {
        Err(error) if error.code() == D2DERR_RECREATE_TARGET => {
            state.target = None;
            state.brushes = None;
            state.images.release_bitmaps();
            invalidate(hwnd);
        }
        Err(error) => return Err(crate::preview::dwrite::hresult_error(error)),
        Ok(()) => {
            if let Some(opened) = state.opened_at.take() {
                state.stats.first_frame_micros = opened.elapsed().as_micros().max(1) as u64;
            }
            if let Some(started) = state.update_started.take() {
                state.stats.last_update_micros = started.elapsed().as_micros().max(1) as u64;
            }
            let links = visible_links(state);
            *state.accessible.write().unwrap_or_else(|error| error.into_inner()) = links;
        }
    }
    update_scrollbar(state, view_height);
    Ok(())
}

fn update_scrollbar(state: &mut ViewState, view_height: f32) {
    let scale = dpi_scale(state.hwnd);
    let info = SCROLLINFO {
        cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
        fMask: SIF_ALL,
        nMin: 0,
        nMax: (state.heights.total() * scale) as i32,
        nPage: ((view_height - bar_height(state)) * scale).max(0.0) as u32,
        nPos: (state.scroll_y * scale) as i32,
        nTrackPos: 0,
    };
    unsafe { SetScrollInfo(state.hwnd, SB_VERT as i32, &info, 1) };
}

fn client_point(lparam: LPARAM) -> (i32, i32) {
    ((lparam & 0xFFFF) as u16 as i16 as i32, ((lparam >> 16) & 0xFFFF) as u16 as i16 as i32)
}

fn link_at(state: &mut ViewState, x: i32, y: i32) -> Option<(usize, usize)> {
    let scale = dpi_scale(state.hwnd);
    let (view_width, _) = view_size(state.hwnd);
    let (content_left, _) = content_frame(view_width, state.centered);
    let document_y = y as f32 / scale + state.scroll_y - bar_height(state);
    let index = state.heights.index_at(document_y);
    let top = state.heights.top(index);
    let laid = state.layouts.get(index)?.as_ref()?;
    let block_x = x as f32 / scale - content_left;
    let block_y = document_y - top;
    let offset = state.h_scroll.get(&index).copied().unwrap_or(0.0);
    laid.links.iter().position(|link| {
        let hit_x = if link.scrolls { block_x + offset } else { block_x };
        link.rects.iter().any(|rect| rect.contains(hit_x, block_y))
    })
    .map(|link| (index, link))
}

fn link_dest(state: &ViewState, (block, link): (usize, usize)) -> Option<String> {
    Some(state.layouts.get(block)?.as_ref()?.links.get(link)?.dest.clone())
}

fn set_underline(state: &ViewState, target: Option<(usize, usize)>, underline: bool) {
    if let Some((block, link)) = target
        && let Some(Some(laid)) = state.layouts.get(block)
        && let Some(link) = laid.links.get(link)
    {
        let _ = unsafe { link.layout.SetUnderline(underline, link.range) };
    }
}

fn post_link(hwnd: HWND, dest: String) {
    let payload = Box::into_raw(Box::new(dest));
    if unsafe { PostMessageW(GetParent(hwnd), WM_FASTPAD_PREVIEW_LINK, 0, payload as isize) } == 0 {
        drop(unsafe { Box::from_raw(payload) });
    }
}

fn post_hover(hwnd: HWND, dest: Option<String>) {
    let payload = Box::into_raw(Box::new(dest));
    if unsafe { PostMessageW(GetParent(hwnd), WM_FASTPAD_PREVIEW_HOVER, 0, payload as isize) } == 0 {
        drop(unsafe { Box::from_raw(payload) });
    }
}

fn visible_links(state: &mut ViewState) -> Vec<VisibleLink> {
    let scale = dpi_scale(state.hwnd);
    let (view_width, view_height) = view_size(state.hwnd);
    let (content_left, _) = content_frame(view_width, state.centered);
    let bar = bar_height(state);
    let mut links = Vec::new();
    for index in visible_indices(state, view_height - bar) {
        let top = state.heights.top(index) - state.scroll_y + bar;
        let offset = state.h_scroll.get(&index).copied().unwrap_or(0.0);
        let Some(Some(laid)) = state.layouts.get(index) else { continue };
        for link in &laid.links {
            let Some(rect) = link.rects.first() else { continue };
            let shift = if link.scrolls { offset } else { 0.0 };
            let rect = rect.offset(content_left - shift, top);
            links.push(VisibleLink {
                text: link.text.clone(),
                dest: link.dest.clone(),
                rect: RECT {
                    left: (rect.left * scale) as i32,
                    top: (rect.top * scale) as i32,
                    right: (rect.right * scale) as i32,
                    bottom: (rect.bottom * scale) as i32,
                },
            });
        }
    }
    links
}

/// Moves keyboard focus to the next (or previous) link in document order, laying out blocks on
/// the way, and scrolls it into view.
fn move_focus(state: &mut ViewState, forward: bool) {
    let count = state.document.blocks.len();
    if count == 0 {
        return;
    }
    let (mut block, mut link) = match state.focus {
        Some((block, link)) => (block, Some(link)),
        None => (state.heights.index_at(state.scroll_y), None),
    };
    for _ in 0..count {
        if ensure_layouts(state, &[block]).is_err() {
            return;
        }
        let links = state.layouts[block].as_ref().map_or(0, |laid| laid.links.len());
        let next = match (link, forward) {
            (None, true) if links > 0 => Some(0),
            (None, false) if links > 0 => Some(links - 1),
            (Some(current), true) if current + 1 < links => Some(current + 1),
            (Some(current), false) if current > 0 => Some(current - 1),
            _ => None,
        };
        if let Some(next) = next {
            state.focus = Some((block, next));
            let top = state.heights.top(block);
            let rect = state.layouts[block].as_ref().and_then(|laid| laid.links[next].rects.first().copied()).unwrap_or_default();
            let (_, view_height) = view_size(state.hwnd);
            let visible_height = view_height - bar_height(state);
            if top + rect.top < state.scroll_y || top + rect.bottom > state.scroll_y + visible_height {
                set_scroll(state, top + rect.top - visible_height / 3.0, true);
            }
            invalidate(state.hwnd);
            return;
        }
        link = None;
        block = if forward { (block + 1) % count } else { (block + count - 1) % count };
    }
}

unsafe extern "system" fn preview_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        WM_NCDESTROY => {
            let pointer = unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) } as *mut ViewState;
            if !pointer.is_null() {
                drop(unsafe { Box::from_raw(pointer) });
            }
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let painted = with_state(hwnd, |state| paint(state).is_ok()).unwrap_or(false);
            if !painted {
                with_state(hwnd, |state| {
                    state.target = None;
                    state.brushes = None;
                });
            }
            unsafe { ValidateRect(hwnd, std::ptr::null()) };
            0
        }
        WM_SIZE => {
            let width = (lparam & 0xFFFF) as u32;
            let height = ((lparam >> 16) & 0xFFFF) as u32;
            with_state(hwnd, |state| {
                if let Some(target) = &state.target
                    && unsafe { target.Resize(&D2D_SIZE_U { width: width.max(1), height: height.max(1) }) }.is_err()
                {
                    state.target = None;
                    state.brushes = None;
                }
            });
            invalidate(hwnd);
            0
        }
        WM_GETDLGCODE => DLGC_WANTALLKEYS as LRESULT,
        WM_SETFOCUS | WM_KILLFOCUS => {
            invalidate(hwnd);
            0
        }
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            let delta = ((wparam >> 16) & 0xFFFF) as u16 as i16 as f32;
            let keys = (wparam & 0xFFFF) as u32;
            with_state(hwnd, |state| {
                let step = state.fonts.body_size * 1.5 * 3.0 * delta / 120.0;
                let horizontal = message == WM_MOUSEHWHEEL || keys & MK_SHIFT != 0;
                if horizontal {
                    let mut point = windows_sys::Win32::Foundation::POINT {
                        x: (lparam & 0xFFFF) as u16 as i16 as i32,
                        y: ((lparam >> 16) & 0xFFFF) as u16 as i16 as i32,
                    };
                    unsafe { windows_sys::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut point) };
                    let document_y = point.y as f32 / dpi_scale(hwnd) + state.scroll_y - bar_height(state);
                    let index = state.heights.index_at(document_y);
                    let (view_width, _) = view_size(hwnd);
                    let (_, content_width) = content_frame(view_width, state.centered);
                    if let Some(Some(laid)) = state.layouts.get(index)
                        && laid.scroll_width > content_width
                    {
                        let limit = laid.scroll_width - content_width;
                        let sign = if message == WM_MOUSEHWHEEL { 1.0 } else { -1.0 };
                        let offset = state.h_scroll.entry(index).or_insert(0.0);
                        *offset = (*offset + sign * step).clamp(0.0, limit);
                        invalidate(hwnd);
                    }
                } else {
                    let target = state.scroll_y - step;
                    set_scroll(state, target, true);
                }
            });
            0
        }
        WM_VSCROLL => {
            with_state(hwnd, |state| {
                let (_, view_height) = view_size(hwnd);
                let line = state.fonts.body_size * 1.5;
                let page = (view_height - bar_height(state) - line).max(line);
                let target = match (wparam & 0xFFFF) as i32 {
                    SB_LINEUP => state.scroll_y - line,
                    SB_LINEDOWN => state.scroll_y + line,
                    SB_PAGEUP => state.scroll_y - page,
                    SB_PAGEDOWN => state.scroll_y + page,
                    SB_TOP => 0.0,
                    SB_BOTTOM => f32::MAX,
                    SB_THUMBTRACK => {
                        let mut info = SCROLLINFO { cbSize: std::mem::size_of::<SCROLLINFO>() as u32, fMask: SIF_TRACKPOS, ..Default::default() };
                        unsafe { GetScrollInfo(hwnd, SB_VERT as i32, &mut info) };
                        info.nTrackPos as f32 / dpi_scale(hwnd)
                    }
                    _ => return,
                };
                set_scroll(state, target, true);
            });
            0
        }
        WM_KEYDOWN => {
            let key = wparam as u16;
            if key == VK_ESCAPE {
                unsafe { PostMessageW(GetParent(hwnd), WM_FASTPAD_PREVIEW_ESCAPE, 0, 0) };
                return 0;
            }
            with_state(hwnd, |state| {
                let (_, view_height) = view_size(hwnd);
                let line = state.fonts.body_size * 1.5;
                let page = (view_height - bar_height(state) - line).max(line);
                match key {
                    VK_UP => set_scroll(state, state.scroll_y - 2.0 * line, true),
                    VK_DOWN => set_scroll(state, state.scroll_y + 2.0 * line, true),
                    VK_PRIOR => set_scroll(state, state.scroll_y - page, true),
                    VK_NEXT => set_scroll(state, state.scroll_y + page, true),
                    VK_HOME => set_scroll(state, 0.0, true),
                    VK_END => set_scroll(state, f32::MAX, true),
                    VK_TAB => {
                        let backward = unsafe { GetKeyState(VK_SHIFT as i32) } < 0;
                        move_focus(state, !backward);
                    }
                    VK_RETURN => {
                        if let Some(dest) = state.focus.and_then(|focus| link_dest(state, focus)) {
                            post_link(hwnd, dest);
                        }
                    }
                    _ => {}
                }
            });
            0
        }
        WM_MOUSEMOVE => {
            let (x, y) = client_point(lparam);
            with_state(hwnd, |state| {
                if !state.tracking_mouse {
                    let mut track = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                    state.tracking_mouse = unsafe { TrackMouseEvent(&mut track) } != 0;
                }
                let hovered = link_at(state, x, y);
                if hovered != state.hover {
                    set_underline(state, state.hover, false);
                    set_underline(state, hovered, true);
                    state.hover = hovered;
                    post_hover(hwnd, hovered.and_then(|link| link_dest(state, link)));
                    invalidate(hwnd);
                }
            });
            0
        }
        WM_MOUSELEAVE => {
            with_state(hwnd, |state| {
                state.tracking_mouse = false;
                if state.hover.is_some() {
                    set_underline(state, state.hover, false);
                    state.hover = None;
                    post_hover(hwnd, None);
                    invalidate(hwnd);
                }
            });
            0
        }
        WM_SETCURSOR if (lparam & 0xFFFF) as u32 == HTCLIENT => {
            let over_link = with_state(hwnd, |state| state.hover.is_some()).unwrap_or(false);
            unsafe { SetCursor(LoadCursorW(std::ptr::null_mut(), if over_link { IDC_HAND } else { IDC_ARROW })) };
            1
        }
        WM_LBUTTONDOWN => {
            unsafe { SetFocus(hwnd) };
            let (x, y) = client_point(lparam);
            with_state(hwnd, |state| state.pressed = link_at(state, x, y));
            0
        }
        WM_LBUTTONUP => {
            let (x, y) = client_point(lparam);
            with_state(hwnd, |state| {
                if state.paused && (y as f32) < PAUSED_BAR_HEIGHT * dpi_scale(hwnd) {
                    unsafe { PostMessageW(GetParent(hwnd), WM_FASTPAD_PREVIEW_REFRESH, 0, 0) };
                    return;
                }
                let released = link_at(state, x, y);
                if released.is_some() && released == state.pressed.take()
                    && let Some(dest) = released.and_then(|link| link_dest(state, link))
                {
                    post_link(hwnd, dest);
                }
            });
            0
        }
        WM_FASTPAD_PREVIEW_ACTIVATE => {
            let dest = with_state(hwnd, |state| {
                state
                    .accessible
                    .read()
                    .unwrap_or_else(|error| error.into_inner())
                    .get(wparam)
                    .map(|link| link.dest.clone())
            })
            .flatten();
            if let Some(dest) = dest {
                post_link(hwnd, dest);
            }
            0
        }
        WM_FASTPAD_PREVIEW_IMAGE => {
            with_state(hwnd, |state| {
                if state.images.drain() {
                    for layout in &mut state.layouts {
                        if layout.as_ref().is_some_and(|laid| !laid.images.is_empty()) {
                            *layout = None;
                        }
                    }
                    invalidate(hwnd);
                }
            });
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}
```

Notes for the implementer:
- `ensure_layouts` measures a block the first time it is laid out; after image decodes clear a block's layout, the block is re-laid out and re-measured, and the anchor keeps the reading position.
- `SB_*` constants are `i32` in windows-sys; if a match arm type mismatches, cast with `as i32` on the constant.
- If a bare `None` does not infer for the stroke-style argument of `DrawRectangle`, write `None::<&windows::Win32::Graphics::Direct2D::ID2D1StrokeStyle>` (same in `render.rs`).
- `with_state` must never be nested: none of the functions called inside it send messages (they only post), and `set_scroll`, `move_focus`, and `ensure_layouts` take the already-borrowed state.
- Run `cargo fmt` after pasting.

- [ ] **Step 5: Run tests, Clippy, commit**

Run: `cargo test --lib preview::view -- --test-threads=1` then `cargo clippy --all-targets`
Expected: PASS.

```bash
git add src/window/messages.rs src/window/mod.rs src/preview/mod.rs src/preview/render.rs src/preview/view.rs
git commit -m "feat: FastPadPreview window with virtualized Direct2D painting"
```

---

### Task 12: Commands, shortcut, menu, and palette entries

**Files:**
- Modify: `src/window/commands.rs`
- Modify: `src/window/menus.rs`
- Modify: `src/window/command_palette.rs`

**Interfaces:**
- Produces: `CommandId::MarkdownPreviewCycle = 152`, `MarkdownPreviewSide = 153`, `MarkdownPreviewFull = 154`, `MarkdownPreviewClose = 155`; `impl CommandId { pub const fn is_markdown_preview(self) -> bool }`; `menus::set_markdown_preview_enabled(menu: HMENU, enabled: bool)`.

- [ ] **Step 1: Write failing tests**

Add to the tests in `src/window/commands.rs`:

```rust
    #[test]
    fn markdown_preview_commands_have_stable_values() {
        assert_eq!(CommandId::try_from(152), Ok(CommandId::MarkdownPreviewCycle));
        assert_eq!(CommandId::try_from(153), Ok(CommandId::MarkdownPreviewSide));
        assert_eq!(CommandId::try_from(154), Ok(CommandId::MarkdownPreviewFull));
        assert_eq!(CommandId::try_from(155), Ok(CommandId::MarkdownPreviewClose));
        assert!(CommandId::MarkdownPreviewSide.needs_document());
        assert!(CommandId::MarkdownPreviewClose.is_markdown_preview());
        assert!(!CommandId::Save.is_markdown_preview());
    }
```

Add to the tests in `src/window/command_palette.rs`, next to the existing `shortcut_text` assertions:

```rust
    #[test]
    fn markdown_preview_cycles_with_ctrl_shift_v_and_lists_three_palette_entries() {
        assert_eq!(
            shortcut_text(CommandId::MarkdownPreviewCycle).as_deref(),
            Some("Ctrl+Shift+V")
        );
        let labels = filter_entries("markdown preview", |_| true)
            .into_iter()
            .map(|entry| entry.command)
            .collect::<Vec<_>>();
        for command in [
            CommandId::MarkdownPreviewSide,
            CommandId::MarkdownPreviewFull,
            CommandId::MarkdownPreviewClose,
        ] {
            assert!(labels.contains(&command), "{command:?}");
        }
        assert!(
            filter_entries("markdown preview", |command| !command.is_markdown_preview()).is_empty()
        );
    }
```

If `filter_entries` or entry fields are named differently in that module, use the existing names shown by the module's other tests.

Run: `cargo test --lib window::commands window::command_palette`
Expected: FAIL to compile.

- [ ] **Step 2: Implement**

In `src/window/commands.rs`:
- Append after `ThemeCatppuccinMocha,`: `MarkdownPreviewCycle, MarkdownPreviewSide, MarkdownPreviewFull, MarkdownPreviewClose,`
- Change `const COMMANDS: [CommandId; 52]` to `56` and append the four variants in the same order.
- Add inside `impl CommandId`:

```rust
    pub const fn is_markdown_preview(self) -> bool {
        matches!(
            self,
            Self::MarkdownPreviewCycle
                | Self::MarkdownPreviewSide
                | Self::MarkdownPreviewFull
                | Self::MarkdownPreviewClose
        )
    }
```

In `src/window/menus.rs`:
- Change `accelerator_specs() -> [AcceleratorSpec; 41]` to `42` and append `accelerator(FCONTROL | FSHIFT, b'V', CommandId::MarkdownPreviewCycle),` after the Ctrl+Shift+P entry.
- In the View popup, insert before the separator that precedes `Command &palette...`:

```rust
                MenuEntry::Separator,
                MenuEntry::command("Markdown preview &side by side", CommandId::MarkdownPreviewSide),
                MenuEntry::command("Markdown preview f&ull", CommandId::MarkdownPreviewFull),
                MenuEntry::command("Close Markdown pre&view", CommandId::MarkdownPreviewClose),
```

Also in `src/window/menus.rs`, add (importing `EnableMenuItem`, `MF_BYCOMMAND`, `MF_ENABLED`, `MF_GRAYED` from `WindowsAndMessaging`):

```rust
/// Grays the View menu's preview entries while the active tab is not Markdown.
pub(crate) fn set_markdown_preview_enabled(menu: HMENU, enabled: bool) {
    let state = MF_BYCOMMAND | if enabled { MF_ENABLED } else { MF_GRAYED };
    for command in [
        CommandId::MarkdownPreviewSide,
        CommandId::MarkdownPreviewFull,
        CommandId::MarkdownPreviewClose,
    ] {
        unsafe { EnableMenuItem(menu, command as u32, state) };
    }
}
```

and a test next to the existing menu tests:

```rust
    #[test]
    fn preview_entries_gray_out_and_re_enable() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetMenuState, MF_BYCOMMAND, MF_GRAYED};
        let bar = MenuBar::create().unwrap();
        let view = bar.dropdown(3);
        set_markdown_preview_enabled(view, false);
        let state = unsafe { GetMenuState(view, CommandId::MarkdownPreviewSide as u32, MF_BYCOMMAND) };
        assert_ne!(state & MF_GRAYED, 0);
        set_markdown_preview_enabled(view, true);
        let state = unsafe { GetMenuState(view, CommandId::MarkdownPreviewSide as u32, MF_BYCOMMAND) };
        assert_eq!(state & MF_GRAYED, 0);
    }
```

In `src/window/command_palette.rs` `ENTRIES`, after `entry("Language: Markdown", CommandId::LanguageMarkdown),` add:

```rust
    entry("Markdown Preview: Side by Side", CommandId::MarkdownPreviewSide),
    entry("Markdown Preview: Full", CommandId::MarkdownPreviewFull),
    entry("Markdown Preview: Close", CommandId::MarkdownPreviewClose),
```

Any test that asserts the exact accelerator count or palette catalog must be updated to include these entries.

- [ ] **Step 3: Run tests, Clippy, commit**

Task 14 wires the palette filter and the menu graying into the main window.

Run: `cargo test --lib window::commands window::command_palette window::menus` then `cargo clippy --all-targets`
Expected: PASS. Pressing Ctrl+Shift+V does nothing yet (the `_ =>` arm in `execute_command` ignores unknown commands); Task 14 wires it.

```bash
git add src/window/commands.rs src/window/menus.rs src/window/command_palette.rs
git commit -m "feat: Markdown preview commands, Ctrl+Shift+V, menu and palette entries"
```

---

### Task 13: Title-strip preview buttons

**Files:**
- Modify: `src/window/titlebar.rs`
- Modify: `src/window/tabs.rs`
- Modify: `src/window/accessibility.rs`
- Modify: `src/window/main_window.rs` (call-site parameters only)

**Interfaces:**
- Consumes: `crate::preview::PreviewMode` (Task 1).
- Produces:
  - `HitTarget::PreviewSide`, `HitTarget::PreviewFull`
  - `TitleBarLayout { pub preview_side: Option<Rect>, pub preview_full: Option<Rect>, .. }`
  - `TitleBarLayout::calculate_with_preview(client: Size, dpi: u32, tab_count: usize, scroll: i32, preview_buttons: bool) -> Self` (`calculate_scrolled` delegates with `false`)
  - `layout_for_window(hwnd, tab_count, scroll, preview_buttons: bool)`
  - `TitlePaint { pub preview: Option<PreviewMode>, .. }` (`Some` exactly when the buttons are shown)
  - `nonclient_hit_test` gains a trailing `preview_buttons: bool` parameter
  - `TabViewSnapshot { pub(crate) preview_buttons: bool, .. }`, `TabView::set_preview_buttons(&self, bool)`, `Tabs::set_preview_buttons(&self, bool)`
  - `accessible_children(tab_titles: &[&str], preview_buttons: bool)`
  - `main_window::preview_buttons_visible(hwnd) -> bool` (temporary: returns `false` until Task 14 replaces its body)

- [ ] **Step 1: Write failing layout tests**

In `src/window/titlebar.rs` tests:

```rust
    #[test]
    fn preview_buttons_leave_the_layout_unchanged_when_absent() {
        let client = Size::new(1200, 800);
        assert_eq!(
            TitleBarLayout::calculate_scrolled(client, 96, 3, 0),
            TitleBarLayout::calculate_with_preview(client, 96, 3, 0, false)
        );
        assert!(TitleBarLayout::calculate_scrolled(client, 96, 3, 0).preview_side.is_none());
    }

    #[test]
    fn preview_buttons_sit_left_of_the_overflow_button_without_overlap() {
        let layout = TitleBarLayout::calculate_with_preview(Size::new(1200, 800), 144, 20, 0, true);
        let side = layout.preview_side.unwrap();
        let full = layout.preview_full.unwrap();
        assert_eq!(side.right, full.left);
        assert_eq!(full.right, layout.overflow.left);
        assert!(layout.tabs.right <= side.left);
        assert_eq!(layout.drag_region.right, side.left);
        assert_eq!(layout.hit_test(side.center()), HitTarget::PreviewSide);
        assert_eq!(layout.hit_test(full.center()), HitTarget::PreviewFull);
        let without = TitleBarLayout::calculate_with_preview(Size::new(1200, 800), 144, 20, 0, false);
        assert!(layout.tabs.right < without.tabs.right);
    }
```

In `src/window/accessibility.rs` tests, change both existing `accessible_children(&["Untitled"])` calls to `accessible_children(&["Untitled"], false)` and add:

```rust
    #[test]
    fn preview_buttons_are_appended_after_the_caption_buttons() {
        let children = accessible_children(&["Untitled"], true);
        let names = children.iter().filter_map(AccessibleChild::button_name).collect::<Vec<_>>();
        assert_eq!(
            names,
            vec!["Overflow", "Minimize", "Maximize", "Close", "Open Preview to the Side", "Open Preview"]
        );
        assert_eq!(
            accessible_default_action(&children, 6),
            Some(AccessibleDefaultAction::Click(crate::window::titlebar::HitTarget::PreviewSide))
        );
        assert_eq!(
            accessible_default_action(&children, 7),
            Some(AccessibleDefaultAction::Click(crate::window::titlebar::HitTarget::PreviewFull))
        );
    }
```

Import `AccessibleDefaultAction` into that test module.

Run: `cargo test --lib window::titlebar window::accessibility`
Expected: FAIL to compile.

- [ ] **Step 2: Implement the layout and hit targets**

In `src/window/titlebar.rs`:

1. Add `PreviewSide, PreviewFull,` to `HitTarget` after `Overflow`, add both to `is_interactive`, and add both to the `HTCLIENT` arm of `nonclient_hit_test`.
2. Add fields to `TitleBarLayout` after `overflow`:

```rust
    /// "Open Preview to the Side" and "Open Preview", present only for Markdown tabs.
    pub preview_side: Option<Rect>,
    pub preview_full: Option<Rect>,
```

3. Rename the body of `calculate_scrolled` into:

```rust
    pub fn calculate_scrolled(client: Size, dpi: u32, tab_count: usize, scroll: i32) -> Self {
        Self::calculate_with_preview(client, dpi, tab_count, scroll, false)
    }

    pub fn calculate_with_preview(
        client: Size,
        dpi: u32,
        tab_count: usize,
        scroll: i32,
        preview_buttons: bool,
    ) -> Self {
```

and inside it, directly after `let overflow = Rect::new(overflow_left, 0, actions_right, height);`:

```rust
        let (preview_side, preview_full, buttons_left) = if preview_buttons {
            let button = scale(40, dpi);
            let full_left = (overflow_left - button).max(0);
            let side_left = (full_left - button).max(0);
            (
                Some(Rect::new(side_left, 0, full_left, height)),
                Some(Rect::new(full_left, 0, overflow_left, height)),
                side_left,
            )
        } else {
            (None, None, overflow_left)
        };
```

then replace `overflow_left` with `buttons_left` in the `tabs_right` computation and as the `drag_region` right edge, and add `preview_side, preview_full,` to the returned struct.

4. In `hit_test`, after the overflow check:

```rust
        if self.preview_side.is_some_and(|rect| rect.contains(point)) {
            return HitTarget::PreviewSide;
        }
        if self.preview_full.is_some_and(|rect| rect.contains(point)) {
            return HitTarget::PreviewFull;
        }
```

5. `layout_for_window(hwnd, tab_count, scroll, preview_buttons: bool)` calls `calculate_with_preview`. Update callers: `invalidate_strip` passes `false`; `paint` passes `input.preview.is_some()`; `nonclient_hit_test` passes its new `preview_buttons` parameter; `main_window::title_layout` passes `preview_buttons_visible(hwnd)`; the `WM_NCHITTEST` call in `main_window.rs` passes `preview_buttons_visible(hwnd)`.

6. Add to `main_window.rs` near `title_layout`:

```rust
/// Whether the title strip shows the Markdown preview buttons (the active tab is Markdown).
fn preview_buttons_visible(_hwnd: HWND) -> bool {
    false
}
```

- [ ] **Step 3: Paint the buttons**

Add glyph constants beside `GLYPH_MORE`:

```rust
const GLYPH_PREVIEW_SIDE: &str = "\u{E90D}";
const GLYPH_PREVIEW_FULL: &str = "\u{E8FF}";
```

Add to `TitlePaint`:

```rust
    /// The current preview mode while the preview buttons are shown; `None` hides them.
    pub preview: Option<crate::preview::PreviewMode>,
```

In the strip painter, directly after the overflow glyph is drawn (same `select_font(dc, input.fonts.glyph)` context):

```rust
    if let Some(mode) = input.preview {
        for (target, rect, glyph, active) in [
            (HitTarget::PreviewSide, layout.preview_side, GLYPH_PREVIEW_SIDE, mode == crate::preview::PreviewMode::Split),
            (HitTarget::PreviewFull, layout.preview_full, GLYPH_PREVIEW_FULL, mode == crate::preview::PreviewMode::Full),
        ] {
            let Some(rect) = rect else { continue };
            let hovered = pointer.hovered == Some(target);
            let background = if hovered && pointer.is_pressed(target) {
                Some(palette.pressed_background)
            } else if active {
                Some(palette.pressed_background)
            } else if hovered {
                Some(palette.hover_background)
            } else {
                None
            };
            unsafe {
                if let Some(background) = background {
                    fill(dc, rect.centered_square(scale(32, dpi)), background);
                }
                SetTextColor(dc, if hovered || active { palette.hover_foreground } else { palette.muted_foreground });
                draw_text(dc, glyph, rect, centered);
            }
        }
    }
```

In `main_window.rs` `WM_PAINT`, add `preview: None,` to the `TitlePaint` literal. Task 14 replaces it with the live mode once `App::preview` exists.

- [ ] **Step 4: Tab view flag and accessibility**

In `src/window/tabs.rs`: add `preview_buttons: bool` to `TabViewState` (initialized `false`) and to `TabViewSnapshot`; copy it in `snapshot()`; add:

```rust
impl TabView {
    pub(crate) fn set_preview_buttons(&self, visible: bool) {
        self.state.write().unwrap_or_else(|error| error.into_inner()).preview_buttons = visible;
    }
}

impl Tabs {
    pub(crate) fn set_preview_buttons(&self, visible: bool) {
        self.view.set_preview_buttons(visible);
    }
}
```

In `src/window/accessibility.rs`:

1. `pub fn accessible_children(tab_titles: &[&str], preview_buttons: bool)`: after pushing the four caption buttons, add

```rust
    if preview_buttons {
        children.extend([
            AccessibleChild::Button("Open Preview to the Side"),
            AccessibleChild::Button("Open Preview"),
        ]);
    }
```

2. `children_from_view` passes `view.preview_buttons`.
3. Replace the `Overflow` variant of `AccessibleDefaultAction` with `Click(crate::window::titlebar::HitTarget)`; in `accessible_default_action` map button `0 => Click(HitTarget::Overflow)`, `4 => Click(HitTarget::PreviewSide)`, `5 => Click(HitTarget::PreviewFull)`.
4. In `accessible_do_default_action`, replace the `Overflow` arm with:

```rust
        Some(AccessibleDefaultAction::Click(target)) => {
            let layout = native_layout(item, tabs);
            let rect = match target {
                crate::window::titlebar::HitTarget::Overflow => Some(layout.overflow),
                crate::window::titlebar::HitTarget::PreviewSide => layout.preview_side,
                crate::window::titlebar::HitTarget::PreviewFull => layout.preview_full,
                _ => None,
            };
            let Some(rect) = rect else {
                return E_INVALIDARG;
            };
            let center = rect.center();
            let packed = (center.x as u16 as u32 | ((center.y as u16 as u32) << 16)) as isize;
            unsafe { PostMessageW(item.hwnd, WM_LBUTTONUP, 0, packed) }
        }
```

5. `native_layout` calls `TitleBarLayout::calculate_with_preview(.., item.view.snapshot().preview_buttons)`.
6. `child_screen_rect`: add `5 => layout.preview_side?,` and `6 => layout.preview_full?,`.
7. `accessible_hit_test`: map `PreviewSide => Some(tab_count as i32 + 5)`, `PreviewFull => Some(tab_count as i32 + 6)`.

- [ ] **Step 5: Run tests, Clippy, commit**

Run: `cargo test --lib window::titlebar window::accessibility window::tabs` then `cargo clippy --all-targets`
Expected: PASS; with `preview_buttons_visible` returning `false` the app looks unchanged.

```bash
git add src/window/titlebar.rs src/window/tabs.rs src/window/accessibility.rs src/window/main_window.rs
git commit -m "feat: title-strip Markdown preview buttons and their accessibility"
```

---
### Task 14: Preview host — modes, split layout, divider, buttons, focus

**Files:**
- Create: `src/window/preview_host.rs`
- Modify: `src/window/mod.rs` (add `pub(crate) mod preview_host;`)
- Modify: `src/app.rs` (field `preview`)
- Modify: `src/window/main_window.rs` (visibility of helpers, hooks listed in Step 4)
- Modify: `src/window/titlebar.rs` (`TitlePaint::divider`)
- Modify: `tests/windows/markdown_preview.rs` (in-process harness and mode tests)

**Interfaces:**
- Consumes: `PreviewView` (Task 11), `Graphics` (Task 8), `PreviewDocument`, `EditLog`, `SourceText` (Task 4), `preview_colors` (Task 7), `PreviewFonts` (Task 9), commands (Task 12), `HitTarget::PreviewSide/PreviewFull` and `Tabs::set_preview_buttons` (Task 13), `Editor::{range_bytes, line_from_position, first_visible_line, doc_line_from_visible, length, text}` (Task 2 and existing).
- Produces in `crate::window::preview_host`:
  - `pub(crate) const PREVIEW_TIMER_ID: usize = 0x4650_5056;`
  - `#[derive(Debug)] pub(crate) struct PreviewHost` (`Default`), with `pub(crate) mode: PreviewMode`, `pub(crate) sync_count: u64`, and `pub(crate) fn status_hint(&self) -> Option<String>`
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub(crate) struct ContentRects { pub editor: Option<RECT>, pub divider: Option<RECT>, pub preview: Option<RECT> }`
  - `pub(crate) fn content_rects(area: RECT, mode: PreviewMode, shown: bool, ratio: f32, dpi: u32) -> ContentRects`
  - `pub(crate) fn ratio_for_x(area: RECT, x: i32, dpi: u32) -> f32`
  - `pub(crate) struct ScintillaSource<'a>(pub &'a Editor)` implementing `SourceText`
  - hwnd-based: `mode`, `view`, `buttons_visible`, `preview_shown`, `run_command`, `click_button`, `escape`, `set_mode`, `sync_visibility`, `layout`, `divider_rect`, `begin_divider_drag`, `drag_divider`, `end_divider_drag`, `cursor_over_divider`, `full_view_hwnd`, `button_hover`, `refresh_appearance`, `load_active_document`
- Produces in `App`: `pub(crate) preview: crate::window::preview_host::PreviewHost`

- [ ] **Step 1: Write failing pure layout tests**

Create `src/window/preview_host.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const AREA: RECT = RECT { left: 0, top: 40, right: 1004, bottom: 700 };

    #[test]
    fn off_or_hidden_previews_give_the_editor_the_whole_area() {
        let whole = ContentRects { editor: Some(AREA), divider: None, preview: None };
        assert_eq!(content_rects(AREA, PreviewMode::Off, true, 0.5, 96), whole);
        assert_eq!(content_rects(AREA, PreviewMode::Split, false, 0.5, 96), whole);
    }

    #[test]
    fn split_places_a_divider_between_editor_and_preview() {
        let rects = content_rects(AREA, PreviewMode::Split, true, 0.5, 96);
        assert_eq!(rects.editor, Some(RECT { right: 500, ..AREA }));
        assert_eq!(rects.divider, Some(RECT { left: 500, right: 504, ..AREA }));
        assert_eq!(rects.preview, Some(RECT { left: 504, ..AREA }));
    }

    #[test]
    fn split_ratio_is_clamped() {
        let wide = content_rects(AREA, PreviewMode::Split, true, 0.95, 96);
        assert_eq!(wide.editor.unwrap().right, 800);
        let narrow = content_rects(AREA, PreviewMode::Split, true, 0.01, 96);
        assert_eq!(narrow.editor.unwrap().right, 200);
    }

    #[test]
    fn full_mode_hides_the_editor() {
        let rects = content_rects(AREA, PreviewMode::Full, true, 0.5, 96);
        assert_eq!(rects, ContentRects { editor: None, divider: None, preview: Some(AREA) });
    }

    #[test]
    fn dragging_maps_the_pointer_to_a_clamped_ratio() {
        assert_eq!(ratio_for_x(AREA, 502, 96), 0.5);
        assert_eq!(ratio_for_x(AREA, 0, 96), 0.2);
        assert_eq!(ratio_for_x(AREA, 5000, 96), 0.8);
    }
}
```

Add `pub(crate) mod preview_host;` to `src/window/mod.rs`.

Run: `cargo test --lib window::preview_host`
Expected: FAIL to compile.

- [ ] **Step 2: Implement `preview_host.rs`**

Above the tests:

```rust
//! The main window's half of the Markdown preview: the mode, the split layout and its divider,
//! loading documents into the preview window, and (in later tasks) the debounced update pipeline,
//! scroll sync, and link actions. Every entry point takes the main window handle. No function holds
//! an `App` borrow while calling Win32 APIs that can send messages back to the main window
//! (`SetFocus`, `ShowWindow`, `MoveWindow`, `ShellExecuteW`).

use crate::Result;
use crate::document::{DocumentId, Language};
use crate::editor::Editor;
use crate::preview::colors::{PreviewColors, preview_colors};
use crate::preview::dwrite::Graphics;
use crate::preview::incremental::{EditLog, PreviewDocument, SourceText};
use crate::preview::layout::PreviewFonts;
use crate::preview::view::PreviewView;
use crate::preview::{LIVE_UPDATE_LIMIT, PreviewMode, WORKER_PARSE_THRESHOLD};
use crate::window::commands::CommandId;
use crate::window::main_window as host_window;
use crate::window::palette::Palette;
use crate::window::panel::scale;
use crate::window::titlebar::HitTarget;
use std::borrow::Cow;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;
use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetDoubleClickTime, GetFocus, ReleaseCapture, SetCapture, SetFocus,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetMessageTime, KillTimer, MoveWindow, SW_HIDE, SW_SHOWNA, ShowWindow,
};

pub(crate) const PREVIEW_TIMER_ID: usize = 0x4650_5056;
const DIVIDER_WIDTH_AT_96_DPI: i32 = 4;
const MIN_RATIO: f32 = 0.2;
const MAX_RATIO: f32 = 0.8;
const NOT_MARKDOWN_NOTICE: &str =
    "Markdown preview is available for Markdown documents. Choose View > Markdown to treat this \
     tab as Markdown.";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ScrollOrigin {
    #[default]
    None,
    Preview,
}

#[derive(Debug)]
pub(crate) struct PreviewHost {
    pub(crate) mode: PreviewMode,
    pub(crate) view: Option<PreviewView>,
    graphics: Option<Rc<Graphics>>,
    ratio: f32,
    pub(crate) edits: EditLog,
    /// The document the preview currently shows; `None` forces a reload when next shown.
    pub(crate) document: Option<DocumentId>,
    pub(crate) parse_generation: u64,
    dragging: bool,
    last_divider_click: Option<u32>,
    area: Option<RECT>,
    divider: Option<RECT>,
    pub(crate) scroll_origin: ScrollOrigin,
    pub(crate) sync_count: u64,
    pub(crate) hover_text: Option<String>,
    button_hint: Option<&'static str>,
}

impl Default for PreviewHost {
    fn default() -> Self {
        Self {
            mode: PreviewMode::Off,
            view: None,
            graphics: None,
            ratio: 0.5,
            edits: EditLog::default(),
            document: None,
            parse_generation: 0,
            dragging: false,
            last_divider_click: None,
            area: None,
            divider: None,
            scroll_origin: ScrollOrigin::None,
            sync_count: 0,
            hover_text: None,
            button_hint: None,
        }
    }
}

impl PreviewHost {
    pub(crate) fn status_hint(&self) -> Option<String> {
        self.button_hint.map(str::to_owned).or_else(|| self.hover_text.clone())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ContentRects {
    pub editor: Option<RECT>,
    pub divider: Option<RECT>,
    pub preview: Option<RECT>,
}

pub(crate) fn content_rects(area: RECT, mode: PreviewMode, shown: bool, ratio: f32, dpi: u32) -> ContentRects {
    match (mode, shown) {
        (PreviewMode::Full, true) => ContentRects { editor: None, divider: None, preview: Some(area) },
        (PreviewMode::Split, true) => {
            let divider = scale(DIVIDER_WIDTH_AT_96_DPI, dpi);
            let usable = (area.right - area.left - divider).max(0);
            let editor_right =
                area.left + (usable as f32 * ratio.clamp(MIN_RATIO, MAX_RATIO)).round() as i32;
            ContentRects {
                editor: Some(RECT { right: editor_right, ..area }),
                divider: Some(RECT { left: editor_right, right: editor_right + divider, ..area }),
                preview: Some(RECT { left: editor_right + divider, ..area }),
            }
        }
        _ => ContentRects { editor: Some(area), divider: None, preview: None },
    }
}

pub(crate) fn ratio_for_x(area: RECT, x: i32, dpi: u32) -> f32 {
    let divider = scale(DIVIDER_WIDTH_AT_96_DPI, dpi);
    let usable = (area.right - area.left - divider).max(1);
    ((x - area.left - divider / 2) as f32 / usable as f32).clamp(MIN_RATIO, MAX_RATIO)
}

/// Document text read straight from Scintilla's buffer without copying.
pub(crate) struct ScintillaSource<'a>(pub &'a Editor);

impl SourceText for ScintillaSource<'_> {
    fn len(&self) -> usize {
        self.0.length().unwrap_or(0)
    }

    fn slice(&self, range: Range<usize>) -> Cow<'_, str> {
        match self.0.range_bytes(range) {
            Ok(bytes) => String::from_utf8_lossy(bytes),
            Err(_) => Cow::Borrowed(""),
        }
    }

    fn line_of(&self, byte: usize) -> usize {
        self.0.line_from_position(byte).unwrap_or(0)
    }
}

pub(crate) fn with_host<R>(hwnd: HWND, action: impl FnOnce(&mut PreviewHost) -> R) -> Option<R> {
    unsafe { host_window::app_ptr(hwnd) }.map(|mut app| action(&mut unsafe { app.as_mut() }.preview))
}

pub(crate) fn mode(hwnd: HWND) -> PreviewMode {
    with_host(hwnd, |host| host.mode).unwrap_or_default()
}

pub(crate) fn view(hwnd: HWND) -> Option<PreviewView> {
    with_host(hwnd, |host| host.view).flatten()
}

pub(crate) fn editor(hwnd: HWND) -> Option<Editor> {
    unsafe { host_window::app_ptr(hwnd) }.and_then(|app| unsafe { app.as_ref() }.editor.clone())
}

/// The active tab's id, language, and folder.
pub(crate) fn active_document(hwnd: HWND) -> Option<(DocumentId, Language, Option<PathBuf>)> {
    let app = unsafe { host_window::app_ptr(hwnd) }?;
    let document = unsafe { app.as_ref() }.tabs.active()?;
    let folder = document.path.as_ref().and_then(|path| path.parent().map(PathBuf::from));
    Some((document.id, document.language, folder))
}

pub(crate) fn buttons_visible(hwnd: HWND) -> bool {
    active_document(hwnd).is_some_and(|(_, language, _)| language == Language::Markdown)
}

pub(crate) fn preview_shown(hwnd: HWND) -> bool {
    mode(hwnd) != PreviewMode::Off && buttons_visible(hwnd) && view(hwnd).is_some()
}

pub(crate) fn full_view_hwnd(hwnd: HWND) -> Option<HWND> {
    (mode(hwnd) == PreviewMode::Full && preview_shown(hwnd)).then(|| view(hwnd).map(|view| view.hwnd())).flatten()
}

pub(crate) fn run_command(hwnd: HWND, command: CommandId) {
    let next = match command {
        CommandId::MarkdownPreviewCycle => match mode(hwnd) {
            PreviewMode::Off => PreviewMode::Split,
            PreviewMode::Split => PreviewMode::Full,
            PreviewMode::Full => PreviewMode::Off,
        },
        CommandId::MarkdownPreviewSide => PreviewMode::Split,
        CommandId::MarkdownPreviewFull => PreviewMode::Full,
        CommandId::MarkdownPreviewClose => PreviewMode::Off,
        _ => return,
    };
    set_mode(hwnd, next);
}

/// Title-strip buttons toggle: the pressed button turns the preview off.
pub(crate) fn click_button(hwnd: HWND, target: HitTarget) {
    let button_mode = match target {
        HitTarget::PreviewSide => PreviewMode::Split,
        HitTarget::PreviewFull => PreviewMode::Full,
        _ => return,
    };
    set_mode(hwnd, if mode(hwnd) == button_mode { PreviewMode::Off } else { button_mode });
}

pub(crate) fn escape(hwnd: HWND) {
    if mode(hwnd) == PreviewMode::Full {
        set_mode(hwnd, PreviewMode::Split);
    }
}

pub(crate) fn set_mode(hwnd: HWND, next: PreviewMode) {
    if next != PreviewMode::Off && !buttons_visible(hwnd) {
        host_window::push_notice(hwnd, NOT_MARKDOWN_NOTICE.to_owned());
        return;
    }
    let previous = mode(hwnd);
    with_host(hwnd, |host| host.mode = next);
    if next == PreviewMode::Off {
        close_view(hwnd);
    }
    sync_visibility(hwnd);
    let focus = match (previous, next) {
        (_, PreviewMode::Full) => view(hwnd).map(|view| view.hwnd()),
        (PreviewMode::Full, _) => unsafe { host_window::editor_hwnd(hwnd) },
        _ => None,
    };
    if let Some(target) = focus {
        unsafe { SetFocus(target) };
    }
}

fn close_view(hwnd: HWND) {
    let closed = with_host(hwnd, |host| {
        host.edits = EditLog::default();
        host.document = None;
        host.divider = None;
        host.hover_text = None;
        host.view.take()
    })
    .flatten();
    unsafe { KillTimer(hwnd, PREVIEW_TIMER_ID) };
    if let Some(view) = closed {
        let had_focus = unsafe { GetFocus() } == view.hwnd();
        view.destroy();
        if had_focus && let Some(editor) = unsafe { host_window::editor_hwnd(hwnd) } {
            unsafe { SetFocus(editor) };
        }
    }
}

fn appearance(hwnd: HWND) -> (PreviewColors, PreviewFonts, bool) {
    let theme = host_window::effective_theme(hwnd);
    unsafe { host_window::app_ptr(hwnd) }
        .map(|app| {
            let app = unsafe { app.as_ref() };
            let high_contrast = app.theme.is_some_and(|system| system.high_contrast);
            let dark = Palette::for_cached_theme(app.theme, app.settings.theme).dark_frame;
            (
                preview_colors(theme, high_contrast),
                PreviewFonts::from_settings(&app.settings.font_face, app.settings.font_size),
                dark,
            )
        })
        .unwrap_or_else(|| (preview_colors(theme, false), PreviewFonts::from_settings("Consolas", 11), false))
}

pub(crate) fn refresh_appearance(hwnd: HWND) {
    if let Some(view) = view(hwnd) {
        let (colors, fonts, dark) = appearance(hwnd);
        view.set_appearance(colors, fonts, dark);
    }
}

fn ensure_view(hwnd: HWND) -> Result<PreviewView> {
    if let Some(view) = view(hwnd) {
        return Ok(view);
    }
    let started = Instant::now();
    let graphics = match with_host(hwnd, |host| host.graphics.clone()).flatten() {
        Some(graphics) => graphics,
        None => {
            let graphics = Rc::new(Graphics::load()?);
            with_host(hwnd, |host| host.graphics = Some(Rc::clone(&graphics)));
            graphics
        }
    };
    let (colors, fonts, dark) = appearance(hwnd);
    let view = PreviewView::create(hwnd, graphics, colors, fonts.clone())?;
    view.set_appearance(colors, fonts, dark);
    view.mark_opened(started);
    with_host(hwnd, |host| {
        host.view = Some(view);
        host.document = None;
    });
    Ok(view)
}

/// Shows or hides the preview and editor for the current mode and active tab, loading the active
/// document when the preview shows something else. Called after mode changes, tab activation or
/// closing, language changes, and file loads.
pub(crate) fn sync_visibility(hwnd: HWND) {
    let markdown = buttons_visible(hwnd);
    if let Some(app) = unsafe { host_window::app_ptr(hwnd) } {
        unsafe { app.as_ref() }.tabs.set_preview_buttons(markdown);
    }
    let wanted = mode(hwnd);
    let mut view = view(hwnd);
    if wanted != PreviewMode::Off && markdown && view.is_none() {
        match ensure_view(hwnd) {
            Ok(created) => view = Some(created),
            Err(error) => {
                with_host(hwnd, |host| host.mode = PreviewMode::Off);
                host_window::push_notice(hwnd, format!("FastPad could not open the Markdown preview: {error}"));
            }
        }
    }
    let shown = wanted != PreviewMode::Off && markdown && view.is_some();
    if let Some(view) = view {
        view.set_centered(wanted == PreviewMode::Full);
        if !shown {
            with_host(hwnd, |host| {
                host.document = None;
                host.edits = EditLog::default();
            });
        }
    }
    if shown {
        let active = active_document(hwnd).map(|(id, ..)| id);
        if with_host(hwnd, |host| host.document).flatten() != active {
            load_active_document(hwnd, false);
        }
    }
    let editor_hwnd = unsafe { host_window::editor_hwnd(hwnd) };
    let hide_editor = shown && wanted == PreviewMode::Full;
    if let (Some(editor), Some(view)) = (editor_hwnd, view)
        && hide_editor
        && unsafe { GetFocus() } == editor
    {
        unsafe { SetFocus(view.hwnd()) };
    }
    if let Some(view) = view {
        unsafe { ShowWindow(view.hwnd(), if shown { SW_SHOWNA } else { SW_HIDE }) };
    }
    if let Some(editor) = editor_hwnd
        && host_window::tab_count(hwnd) > 0
    {
        unsafe { ShowWindow(editor, if hide_editor { SW_HIDE } else { SW_SHOWNA }) };
    }
    host_window::layout_editor_and_find_bar(hwnd);
    host_window::invalidate_title_strip(hwnd);
}

pub(crate) fn load_active_document(hwnd: HWND, force: bool) {
    let (Some(view), Some(editor), Some((id, _, folder))) = (view(hwnd), editor(hwnd), active_document(hwnd)) else {
        return;
    };
    let started = Instant::now();
    unsafe { KillTimer(hwnd, PREVIEW_TIMER_ID) };
    with_host(hwnd, |host| {
        host.document = Some(id);
        host.edits = EditLog::default();
    });
    let length = editor.length().unwrap_or(0);
    let top_line = editor.first_visible_line().and_then(|line| editor.doc_line_from_visible(line)).unwrap_or(0);
    if length > LIVE_UPDATE_LIMIT && !force {
        view.set_paused(true);
        view.replace_document(PreviewDocument::default(), folder, started);
    } else if length > WORKER_PARSE_THRESHOLD {
        view.set_paused(length > LIVE_UPDATE_LIMIT);
        view.set_document_dir(folder);
        spawn_parse(hwnd, &editor, id, started);
    } else {
        view.set_paused(false);
        view.replace_document(PreviewDocument::default(), folder, started);
        view.reparse(&ScintillaSource(&editor), started);
        view.scroll_to_line(top_line);
    }
}

/// Task 15 replaces this body with the worker parse; until then large documents parse inline.
fn spawn_parse(_hwnd: HWND, editor: &Editor, _document: DocumentId, started: Instant) {
    if let Some(view) = view(_hwnd) {
        view.reparse(&ScintillaSource(editor), started);
    }
}

/// Positions the preview for `area` and returns where the editor goes.
pub(crate) fn layout(hwnd: HWND, area: RECT, dpi: u32) -> ContentRects {
    let (mode, ratio) = with_host(hwnd, |host| (host.mode, host.ratio)).unwrap_or((PreviewMode::Off, 0.5));
    let rects = content_rects(area, mode, preview_shown(hwnd), ratio, dpi);
    with_host(hwnd, |host| {
        host.area = Some(area);
        host.divider = rects.divider;
    });
    if let (Some(view), Some(rect)) = (view(hwnd), rects.preview) {
        unsafe { MoveWindow(view.hwnd(), rect.left, rect.top, rect.right - rect.left, rect.bottom - rect.top, 1) };
    }
    rects
}

pub(crate) fn divider_rect(hwnd: HWND) -> Option<RECT> {
    with_host(hwnd, |host| host.divider).flatten()
}

fn over_divider(hwnd: HWND, x: i32, y: i32) -> bool {
    divider_rect(hwnd).is_some_and(|rect| x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom)
}

pub(crate) fn begin_divider_drag(hwnd: HWND, x: i32, y: i32) -> bool {
    if !over_divider(hwnd, x, y) {
        return false;
    }
    let now = unsafe { GetMessageTime() } as u32;
    let double_click_time = unsafe { GetDoubleClickTime() };
    let double = with_host(hwnd, |host| {
        let double = host.last_divider_click.is_some_and(|last| now.wrapping_sub(last) <= double_click_time);
        host.last_divider_click = if double { None } else { Some(now) };
        if double {
            host.ratio = 0.5;
        } else {
            host.dragging = true;
        }
        double
    })
    .unwrap_or(false);
    if double {
        host_window::layout_editor_and_find_bar(hwnd);
        return true;
    }
    if let Some(view) = view(hwnd) {
        view.set_live_resize(true);
    }
    unsafe { SetCapture(hwnd) };
    true
}

pub(crate) fn drag_divider(hwnd: HWND, x: i32) -> bool {
    let Some(area) = with_host(hwnd, |host| host.dragging.then_some(host.area).flatten()).flatten() else {
        return false;
    };
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    with_host(hwnd, |host| host.ratio = ratio_for_x(area, x, dpi));
    host_window::layout_editor_and_find_bar(hwnd);
    true
}

pub(crate) fn end_divider_drag(hwnd: HWND) -> bool {
    if !with_host(hwnd, |host| std::mem::replace(&mut host.dragging, false)).unwrap_or(false) {
        return false;
    }
    unsafe { ReleaseCapture() };
    if let Some(view) = view(hwnd) {
        view.set_live_resize(false);
    }
    true
}

pub(crate) fn cursor_over_divider(hwnd: HWND) -> bool {
    let mut point = POINT::default();
    if unsafe { GetCursorPos(&mut point) } == 0 || unsafe { ScreenToClient(hwnd, &mut point) } == 0 {
        return false;
    }
    with_host(hwnd, |host| host.dragging).unwrap_or(false) || over_divider(hwnd, point.x, point.y)
}

pub(crate) fn button_hover(hwnd: HWND, target: Option<HitTarget>) {
    let hint = match target {
        Some(HitTarget::PreviewSide) => Some("Open Preview to the Side (Ctrl+Shift+V cycles preview modes)"),
        Some(HitTarget::PreviewFull) => Some("Open Preview (Ctrl+Shift+V cycles preview modes)"),
        _ => None,
    };
    let changed = with_host(hwnd, |host| std::mem::replace(&mut host.button_hint, hint) != hint).unwrap_or(false);
    if changed {
        host_window::invalidate_status_bar(hwnd);
    }
}
```

- [ ] **Step 3: Add the `App` field and titlebar divider paint**

In `src/app.rs`: add `pub(crate) preview: crate::window::preview_host::PreviewHost,` to `App` (after `command_palette`) and `preview: Default::default(),` in `App::new`.

In `src/window/titlebar.rs`, add to `TitlePaint`:

```rust
    /// The Markdown preview divider, painted between the editor and the preview.
    pub divider: Option<RECT>,
```

and in `paint`, after the strip is painted and before `EndPaint`:

```rust
    if let Some(divider) = input.divider {
        unsafe { fill(dc, divider, input.palette.hover_background) };
    }
```

(use the module's existing `fill` helper; if it takes the crate `Rect` type, convert with the module's existing `RECT`/`Rect` conversion).

- [ ] **Step 4: Hook the main window**

In `src/window/main_window.rs`:

1. Make these `pub(crate)` (keep `unsafe` where present): `app_ptr`, `push_notice`, `layout_editor_and_find_bar`, `editor_hwnd`, `invalidate_title_strip`, `invalidate_status_bar`, `effective_theme`, `input_pending`, `report_open_failure`, `tab_count`, `title_layout`.
2. Replace the body of `preview_buttons_visible` with `crate::window::preview_host::buttons_visible(hwnd)`.
3. In `WM_PAINT`'s `TitlePaint` literal: `preview: preview_buttons_visible(hwnd).then(|| crate::window::preview_host::mode(hwnd)),` and `divider: crate::window::preview_host::divider_rect(hwnd),`.
4. In `layout_editor_and_find_bar`, replace the final `MoveWindow(editor_hwnd, ...)` block with:

```rust
    let area = RECT {
        left: 0,
        top: content_top,
        right: width,
        bottom: (rect.bottom - rect.top - status_height).max(content_top),
    };
    let rects = crate::window::preview_host::layout(hwnd, area, dpi);
    if let Some(editor_rect) = rects.editor {
        unsafe {
            MoveWindow(
                editor_hwnd,
                editor_rect.left,
                editor_rect.top,
                editor_rect.right - editor_rect.left,
                editor_rect.bottom - editor_rect.top,
                1,
            );
        }
    }
```

5. In `execute_command`, before the `_ =>` arm:

```rust
        CommandId::MarkdownPreviewCycle
        | CommandId::MarkdownPreviewSide
        | CommandId::MarkdownPreviewFull
        | CommandId::MarkdownPreviewClose => {
            crate::window::preview_host::run_command(hwnd, command)
        }
```

6. In the `WM_LBUTTONUP` title-target `match`, add:

```rust
                target @ (crate::window::titlebar::HitTarget::PreviewSide
                | crate::window::titlebar::HitTarget::PreviewFull) => {
                    crate::window::preview_host::click_button(hwnd, target)
                }
```

and at the very start of the `WM_LBUTTONUP` arm: `if crate::window::preview_host::end_divider_drag(hwnd) { return 0; }`.
7. At the start of the `WM_LBUTTONDOWN` arm (decode the point the same way that arm already does): `if crate::window::preview_host::begin_divider_drag(hwnd, x, y) { return 0; }`. At the start of the client `WM_MOUSEMOVE` arm: `if crate::window::preview_host::drag_divider(hwnd, x) { return 0; }`; after that arm updates the title pointer, call `crate::window::preview_host::button_hover(hwnd, Some(title_layout(hwnd).hit_test(point)))`; in `WM_MOUSELEAVE`/`WM_NCMOUSELEAVE` handling call `button_hover(hwnd, None)`.
8. Add an arm before the default arm:

```rust
        WM_SETCURSOR if crate::window::preview_host::cursor_over_divider(hwnd) => {
            unsafe { SetCursor(LoadCursorW(std::ptr::null_mut(), IDC_SIZEWE)) };
            1
        }
```

9. At the end of `refresh_tabs` (after `InvalidateRect`): `crate::window::preview_host::sync_visibility(hwnd);`
10. In `apply_language`'s `Some(Ok(()))` branch, after `apply_editor_settings(hwnd);`: `crate::window::preview_host::sync_visibility(hwnd);`
11. In `WM_SETFOCUS`, before the editor branch:

```rust
            if let Some(preview) = crate::window::preview_host::full_view_hwnd(hwnd) {
                unsafe { SetFocus(preview) };
                return 0;
            }
```

12. In the default arm next to the diagnostic JSON check:

```rust
            if message == crate::window::WM_FASTPAD_PREVIEW_ESCAPE {
                crate::window::preview_host::escape(hwnd);
                return 0;
            }
```

13. In `current_status_bar`, after computing the bar and before returning it: if `app.notifications.pending().is_empty()` and `app.preview.status_hint()` is `Some(hint)`, set `left = hint`.
14. In `refilter_command_palette`, compute `let markdown = crate::window::preview_host::buttons_visible(hwnd);` before `filter_entries` and change its predicate to `|command| (has_tabs || !command.needs_document()) && (markdown || !command.is_markdown_preview())`.
15. In `open_menu`, right after `bar.dropdown(index)` yields `menu` and before the dropdown is tracked, when `index == 3` (View): `crate::window::menus::set_markdown_preview_enabled(menu, crate::window::preview_host::buttons_visible(hwnd));`

- [ ] **Step 5: In-process integration harness and mode tests**

Rewrite the top of `tests/windows/markdown_preview.rs` to build the crate in-process like `json_commands.rs` (keep the import-guard test):

```rust
#![cfg(windows)]
mod support;

// Build the crate in-process with cfg(test), as json_commands.rs does, so tests can reach App.
include!("../../src/lib.rs");

use crate::document::Language;
use crate::preview::PreviewMode;
use crate::window::commands::CommandId;
use std::time::{Duration, Instant};
use support::acceptance::AcceptanceHarness;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetFocus, VK_ESCAPE};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, IsWindowVisible, MSG, PM_REMOVE, PeekMessageW, SendMessageW, TranslateMessage,
    WM_COMMAND, WM_KEYDOWN,
};

#[test]
fn binary_does_not_statically_import_preview_graphics_libraries() {
    AcceptanceHarness::new().assert_no_preview_imports();
}

struct TestMain {
    hwnd: HWND,
    editor: HWND,
    identity: app::WindowIdentity,
    _class: window::MainWindowClass,
}

impl TestMain {
    fn new() -> Self {
        let app = app::App::new(launch::LaunchOptions::default(), perf::StartupMetrics::begin().unwrap());
        let identity = app.window_identity();
        let instance = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) };
        let class = window::MainWindowClass::register(instance).unwrap();
        let mut context = window::WindowCreateContext::new(Box::new(app));
        let hwnd = class.create(&mut context).unwrap();
        let editor = unsafe { window::initialize_editor_with(hwnd, &identity, editor::Editor::create).unwrap() };
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::ShowWindow(hwnd, windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOW);
        }
        Self { hwnd, editor, identity, _class: class }
    }

    fn with_app<R>(&self, run: impl FnOnce(&mut app::App) -> R) -> R {
        assert!(self.identity.is_live_for(self.hwnd));
        let raw = unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(self.hwnd, windows_sys::Win32::UI::WindowsAndMessaging::GWLP_USERDATA)
        } as *mut app::App;
        assert!(!raw.is_null());
        run(unsafe { &mut *raw })
    }

    fn command(&self, command: CommandId) {
        unsafe { SendMessageW(self.hwnd, WM_COMMAND, command as usize, 0) };
    }

    fn set_text(&self, text: &str) {
        let bytes = std::ffi::CString::new(text).unwrap();
        unsafe { SendMessageW(self.editor, crate::editor::scintilla_constants::SCI_SETTEXT, 0, bytes.as_ptr() as isize) };
    }

    /// Makes the active tab Markdown without loading Lexilla, then lets the host react.
    fn make_markdown(&self, text: &str) {
        self.set_text(text);
        self.with_app(|app| app.tabs.set_active_language(Language::Markdown));
        window::preview_host::sync_visibility(self.hwnd);
    }

    fn mode(&self) -> PreviewMode {
        self.with_app(|app| app.preview.mode)
    }

    fn view(&self) -> Option<preview::view::PreviewView> {
        self.with_app(|app| app.preview.view)
    }

    fn notices(&self) -> Vec<String> {
        self.with_app(|app| app.notifications.pending().iter().map(|notice| notice.message.clone()).collect())
    }
}

impl Drop for TestMain {
    fn drop(&mut self) {
        if self.identity.is_live_for(self.hwnd) {
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(self.hwnd) };
        }
    }
}

fn pump_until(what: &str, timeout: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    loop {
        let mut msg = MSG::default();
        while unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if condition() {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn pump_for(duration: Duration) {
    let deadline = Instant::now() + duration;
    pump_until("pump_for", duration + Duration::from_secs(1), || Instant::now() >= deadline);
}

fn visible(hwnd: HWND) -> bool {
    unsafe { IsWindowVisible(hwnd) != 0 }
}

#[test]
fn side_full_and_pressed_full_move_through_split_full_and_off() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    main.make_markdown("# Title\n\nBody\n");
    assert!(window::preview_host::buttons_visible(main.hwnd));

    window::preview_host::click_button(main.hwnd, window::titlebar::HitTarget::PreviewSide);
    assert_eq!(main.mode(), PreviewMode::Split);
    let view = main.view().expect("preview window");
    assert!(visible(view.hwnd()) && visible(main.editor));
    pump_until("first preview frame", Duration::from_secs(3), || view.stats().block_count == 2);

    window::preview_host::click_button(main.hwnd, window::titlebar::HitTarget::PreviewFull);
    assert_eq!(main.mode(), PreviewMode::Full);
    assert!(!visible(main.editor));
    assert_eq!(unsafe { GetFocus() }, view.hwnd());

    window::preview_host::click_button(main.hwnd, window::titlebar::HitTarget::PreviewFull);
    assert_eq!(main.mode(), PreviewMode::Off);
    assert!(main.view().is_none());
    assert!(visible(main.editor));
    assert_eq!(unsafe { GetFocus() }, main.editor);
    assert_eq!(support::win32::scintilla_text(main.editor).unwrap(), "# Title\n\nBody\n");
}

#[test]
fn ctrl_shift_v_cycles_off_split_full_off() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    main.make_markdown("text\n");
    for expected in [PreviewMode::Split, PreviewMode::Full, PreviewMode::Off] {
        main.command(CommandId::MarkdownPreviewCycle);
        assert_eq!(main.mode(), expected);
    }
}

#[test]
fn a_plain_text_tab_gets_a_notice_instead_of_a_preview() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    main.command(CommandId::MarkdownPreviewSide);
    assert_eq!(main.mode(), PreviewMode::Off);
    assert!(main.notices().iter().any(|notice| notice.contains("Markdown preview is available")));
}

#[test]
fn a_non_markdown_tab_hides_the_preview_and_keeps_the_mode() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    main.make_markdown("# One\n");
    main.command(CommandId::MarkdownPreviewSide);
    let view = main.view().unwrap();
    main.command(CommandId::New);
    assert_eq!(main.mode(), PreviewMode::Split);
    assert!(!visible(view.hwnd()));
    assert!(!main.with_app(|app| app.tabs.view().snapshot().preview_buttons));
    main.command(CommandId::SelectTab1);
    assert!(visible(view.hwnd()));
    assert!(main.with_app(|app| app.tabs.view().snapshot().preview_buttons));
}

#[test]
fn escape_in_full_mode_returns_to_split() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    main.make_markdown("text\n");
    main.command(CommandId::MarkdownPreviewFull);
    let view = main.view().unwrap();
    unsafe { SendMessageW(view.hwnd(), WM_KEYDOWN, VK_ESCAPE as usize, 0) };
    pump_until("Esc handling", Duration::from_secs(2), || main.mode() == PreviewMode::Split);
}
```

If `WindowIdentity::is_live_for` or other harness methods have different names, copy them exactly from `json_commands.rs`.

- [ ] **Step 6: Run tests, Clippy, commit**

Run: `cargo test --lib window::preview_host` then `cargo test --test markdown_preview -- --test-threads=1` then `cargo clippy --all-targets`
Expected: PASS.

```bash
git add src/window/preview_host.rs src/window/mod.rs src/app.rs src/window/main_window.rs src/window/titlebar.rs tests/windows/markdown_preview.rs
git commit -m "feat: Split and Full Markdown preview modes with divider and title buttons"
```

---

### Task 15: Live updates, large documents, links, hover, and diagnostics

**Files:**
- Modify: `src/window/preview_host.rs`
- Modify: `src/preview/view.rs` (accessors)
- Modify: `src/window/main_window.rs` (hooks)
- Modify: `tests/windows/markdown_preview.rs`

**Interfaces:**
- Consumes: Task 14 host; `ScintillaNotification` (Task 2); `classify_link` (Task 5).
- Produces:
  - `PreviewView::is_paused(&self) -> bool`, `PreviewView::colors(&self) -> PreviewColors`
  - `pub(crate) struct ParsedPreview { document: DocumentId, generation: u64, parsed: PreviewDocument, started: Instant }`
  - host fns: `record_edit(hwnd, &ScintillaNotification)`, `flush(hwnd)`, `parsed(hwnd, LPARAM)`, `follow_link(hwnd, LPARAM)`, `hover_link(hwnd, LPARAM)`, `refresh(hwnd)`, `document_reloaded(hwnd)`, `diagnostic(hwnd, selector: usize) -> isize`
  - Diagnostic selectors for `WM_FASTPAD_DIAGNOSTIC_PREVIEW`: `0` mode (Off 0, Split 1, Full 2), `1` block count, `2` revision, `3` first-frame µs, `4` last-update µs, `5` preview top line, `6` sync count, `7` preview shown (0/1).

- [ ] **Step 1: Write failing integration tests**

Append to `tests/windows/markdown_preview.rs`:

```rust
fn type_text(hwnd: HWND, text: &str) {
    for unit in text.encode_utf16() {
        unsafe { SendMessageW(hwnd, windows_sys::Win32::UI::WindowsAndMessaging::WM_CHAR, unit as usize, 0) };
    }
}

#[test]
fn typing_updates_the_preview_only_after_the_pause() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    main.make_markdown("# A\n");
    main.command(CommandId::MarkdownPreviewSide);
    let view = main.view().unwrap();
    pump_until("initial render", Duration::from_secs(3), || view.stats().block_count == 1);
    unsafe {
        SendMessageW(main.editor, crate::editor::scintilla_constants::SCI_DOCUMENTEND, 0, 0);
    }
    type_text(main.editor, "\n\npara");
    assert_eq!(view.stats().block_count, 1, "SCN_MODIFIED must not parse");
    pump_until("debounced update", Duration::from_secs(3), || view.stats().block_count == 2);
}

#[test]
fn theme_changes_recolor_the_preview() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    main.make_markdown("# A\n");
    main.command(CommandId::MarkdownPreviewSide);
    main.command(CommandId::ThemeCatppuccinMocha);
    let view = main.view().unwrap();
    assert_eq!(view.colors().link, crate::catppuccin::MOCHA.blue);
}

#[test]
fn relative_markdown_links_open_in_a_tab() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let dir = std::env::temp_dir().join(format!("fastpad-preview-links-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.md"), "[next](b.md)\n").unwrap();
    std::fs::write(dir.join("b.md"), "# B\n").unwrap();
    let main = TestMain::new();
    window::open_path(main.hwnd, &dir.join("a.md")).unwrap();
    pump_until("a.md loaded", Duration::from_secs(3), || !main.with_app(|app| app.populating_file));
    main.with_app(|app| app.tabs.set_active_language(Language::Markdown));
    main.command(CommandId::MarkdownPreviewSide);
    let payload = Box::into_raw(Box::new(String::from("b.md")));
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(main.hwnd, window::WM_FASTPAD_PREVIEW_LINK, 0, payload as isize);
    }
    pump_until("b.md tab", Duration::from_secs(3), || {
        main.with_app(|app| app.tabs.active().and_then(|document| document.path.clone()))
            == Some(dir.join("b.md"))
    });
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unsupported_links_explain_themselves() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    main.make_markdown("[x](ftp://x.dev)\n");
    main.command(CommandId::MarkdownPreviewSide);
    let payload = Box::into_raw(Box::new(String::from("ftp://x.dev")));
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(main.hwnd, window::WM_FASTPAD_PREVIEW_LINK, 0, payload as isize);
    }
    pump_until("link notice", Duration::from_secs(2), || {
        main.notices().iter().any(|notice| notice.contains("does not open this kind of link"))
    });
}

#[test]
fn large_documents_parse_on_a_worker_and_huge_ones_pause() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    let paragraph = "Paragraph text for the worker parse.\n\n";
    main.make_markdown(&paragraph.repeat(2_000_000 / paragraph.len()));
    main.command(CommandId::MarkdownPreviewSide);
    let view = main.view().unwrap();
    pump_until("worker parse", Duration::from_secs(10), || view.stats().block_count > 1000);
    assert!(!view.is_paused());

    main.command(CommandId::MarkdownPreviewClose);
    main.make_markdown(&paragraph.repeat(11_000_000 / paragraph.len()));
    main.command(CommandId::MarkdownPreviewSide);
    let view = main.view().unwrap();
    assert!(view.is_paused());
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(main.hwnd, window::WM_FASTPAD_PREVIEW_REFRESH, 0, 0);
    }
    pump_until("refresh parse", Duration::from_secs(20), || view.stats().block_count > 1000);
}
```

If `app.populating_file` or `window::open_path` differ in name, use the names from `main_window.rs` (`set_file_population`, `open_path`).

Run: `cargo test --test markdown_preview -- --test-threads=1`
Expected: FAIL (`view.colors`, `is_paused` missing; updates never arrive).

- [ ] **Step 2: View accessors**

In `src/preview/view.rs` `impl PreviewView`:

```rust
    pub fn is_paused(&self) -> bool {
        self.with(|state| state.paused).unwrap_or(false)
    }

    pub fn colors(&self) -> PreviewColors {
        self.with(|state| state.colors).unwrap_or_else(|| crate::preview::colors::preview_colors(crate::platform::theme::Theme::Light, false))
    }
```

- [ ] **Step 3: Host update pipeline**

In `src/window/preview_host.rs`, add imports (`crate::editor::ScintillaNotification`, `crate::editor::scintilla_constants::SC_MOD_INSERTTEXT`, `crate::preview::incremental::{Edit, Pending}`, `crate::preview::links::{LinkAction, classify_link}`, `crate::preview::PREVIEW_UPDATE_DELAY_MS`, `crate::platform::wide_null`, `crate::window::messages::WM_FASTPAD_PREVIEW_PARSED`, `windows_sys::Win32::Foundation::LPARAM`, `windows_sys::Win32::UI::Shell::ShellExecuteW`, `windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, SW_SHOWNORMAL, SetTimer}`), replace the placeholder `spawn_parse` with the worker version, and add:

```rust
pub(crate) struct ParsedPreview {
    document: DocumentId,
    generation: u64,
    parsed: PreviewDocument,
    started: Instant,
}

fn spawn_parse(hwnd: HWND, editor: &Editor, document: DocumentId, started: Instant) {
    let Ok(text) = editor.text() else {
        return;
    };
    let generation = with_host(hwnd, |host| {
        host.parse_generation += 1;
        host.parse_generation
    })
    .unwrap_or(0);
    let target = hwnd as isize;
    std::thread::spawn(move || {
        let parsed = PreviewDocument::parse(&text);
        let payload = Box::into_raw(Box::new(ParsedPreview { document, generation, parsed, started }));
        if unsafe { PostMessageW(target as HWND, WM_FASTPAD_PREVIEW_PARSED, 0, payload as isize) } == 0 {
            drop(unsafe { Box::from_raw(payload) });
        }
    });
}

pub(crate) fn parsed(hwnd: HWND, lparam: LPARAM) {
    if lparam == 0 {
        return;
    }
    let payload = unsafe { Box::from_raw(lparam as *mut ParsedPreview) };
    let current = with_host(hwnd, |host| {
        host.document == Some(payload.document) && host.parse_generation == payload.generation
    })
    .unwrap_or(false);
    let (Some(view), true) = (view(hwnd), current) else {
        return;
    };
    let folder = active_document(hwnd).and_then(|(_, _, folder)| folder);
    let ParsedPreview { parsed, started, .. } = *payload;
    view.replace_document(parsed, folder, started);
    if let Some(editor) = editor(hwnd)
        && let Ok(line) = editor.first_visible_line().and_then(|line| editor.doc_line_from_visible(line))
    {
        view.scroll_to_line(line);
    }
}

/// `SCN_MODIFIED`: O(1) bookkeeping only; the timer does the work.
pub(crate) fn record_edit(hwnd: HWND, notification: &ScintillaNotification) {
    let inserted = notification.modification_type as u32 & SC_MOD_INSERTTEXT != 0;
    let length = notification.length.max(0) as usize;
    let edit = Edit {
        position: notification.position.max(0) as usize,
        removed: if inserted { 0 } else { length },
        inserted: if inserted { length } else { 0 },
        lines_delta: notification.lines_added,
    };
    let recorded = with_host(hwnd, |host| {
        if host.view.is_none() || host.document.is_none() {
            return false;
        }
        host.edits.record(edit);
        true
    })
    .unwrap_or(false);
    if recorded {
        unsafe { SetTimer(hwnd, PREVIEW_TIMER_ID, PREVIEW_UPDATE_DELAY_MS, None) };
    }
}

pub(crate) fn flush(hwnd: HWND) {
    unsafe { KillTimer(hwnd, PREVIEW_TIMER_ID) };
    if host_window::input_pending() {
        unsafe { SetTimer(hwnd, PREVIEW_TIMER_ID, PREVIEW_UPDATE_DELAY_MS, None) };
        return;
    }
    let (Some(view), Some(editor), Some((id, ..))) = (view(hwnd), editor(hwnd), active_document(hwnd)) else {
        return;
    };
    if with_host(hwnd, |host| host.document).flatten() != Some(id) {
        return;
    }
    let pending = with_host(hwnd, |host| host.edits.take()).unwrap_or(Pending::Nothing);
    let length = editor.length().unwrap_or(0);
    if length > LIVE_UPDATE_LIMIT {
        view.set_paused(true);
        return;
    }
    let started = Instant::now();
    let pending = if view.is_paused() {
        view.set_paused(false);
        Pending::Full
    } else {
        pending
    };
    let large = length > WORKER_PARSE_THRESHOLD;
    let source = ScintillaSource(&editor);
    match pending {
        Pending::Nothing => {}
        Pending::Full if large => spawn_parse(hwnd, &editor, id, started),
        Pending::Full => {
            view.reparse(&source, started);
        }
        Pending::Edits(edits) => {
            if view.apply_edits(&source, &edits, started, !large).is_none() {
                spawn_parse(hwnd, &editor, id, started);
            }
        }
    }
}

pub(crate) fn refresh(hwnd: HWND) {
    load_active_document(hwnd, true);
}

/// A file finished loading into the active tab: its text replaced whatever the preview showed.
pub(crate) fn document_reloaded(hwnd: HWND) {
    with_host(hwnd, |host| host.document = None);
    sync_visibility(hwnd);
}

pub(crate) fn follow_link(hwnd: HWND, lparam: LPARAM) {
    if lparam == 0 {
        return;
    }
    let dest = *unsafe { Box::from_raw(lparam as *mut String) };
    let folder = active_document(hwnd).and_then(|(_, _, folder)| folder);
    match classify_link(&dest, folder.as_deref()) {
        LinkAction::External(url) => {
            let operation = wide_null("open");
            let target = wide_null(&url);
            let result = unsafe {
                ShellExecuteW(hwnd, operation.as_ptr(), target.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL)
            };
            if result as isize <= 32 {
                host_window::push_notice(hwnd, format!("FastPad could not open {url}."));
            }
        }
        LinkAction::Anchor(anchor) => {
            if !view(hwnd).is_some_and(|view| view.scroll_to_anchor(&anchor)) {
                host_window::push_notice(hwnd, format!("No heading in this document matches #{anchor}."));
            }
        }
        LinkAction::LocalFile(path) => {
            if let Err(error) = crate::window::open_path(hwnd, &path) {
                host_window::report_open_failure(hwnd, &path, &error);
            }
        }
        LinkAction::Ignored => {
            host_window::push_notice(hwnd, format!("FastPad does not open this kind of link: {dest}"));
        }
    }
}

pub(crate) fn hover_link(hwnd: HWND, lparam: LPARAM) {
    if lparam == 0 {
        return;
    }
    let dest = *unsafe { Box::from_raw(lparam as *mut Option<String>) };
    with_host(hwnd, |host| host.hover_text = dest);
    host_window::invalidate_status_bar(hwnd);
}

pub(crate) fn diagnostic(hwnd: HWND, selector: usize) -> isize {
    let view = view(hwnd);
    let stats = view.map(|view| view.stats()).unwrap_or_default();
    match selector {
        0 => match mode(hwnd) {
            PreviewMode::Off => 0,
            PreviewMode::Split => 1,
            PreviewMode::Full => 2,
        },
        1 => stats.block_count as isize,
        2 => stats.revision as isize,
        3 => stats.first_frame_micros as isize,
        4 => stats.last_update_micros as isize,
        5 => view.map_or(0, |view| view.top_line() as isize),
        6 => with_host(hwnd, |host| host.sync_count as isize).unwrap_or(0),
        7 => isize::from(preview_shown(hwnd)),
        _ => -1,
    }
}
```

- [ ] **Step 4: Main-window hooks**

In `src/window/main_window.rs`:

1. `handle_editor_notification`, `SCN_MODIFIED` branch: after the existing `app.tabs.note_active_text_change()` block closes (outside the `App` borrow), when the modification is a text change, call `crate::window::preview_host::record_edit(hwnd, modification);`.
2. Add a timer arm next to the recovery timer:

```rust
        WM_TIMER if wparam == crate::window::preview_host::PREVIEW_TIMER_ID => {
            crate::window::preview_host::flush(hwnd);
            0
        }
```

3. `WM_DESTROY`: add `KillTimer(hwnd, crate::window::preview_host::PREVIEW_TIMER_ID);`.
4. Default arm, next to the Task 14 Esc check:

```rust
            match message {
                crate::window::WM_FASTPAD_PREVIEW_PARSED => {
                    crate::window::preview_host::parsed(hwnd, lparam);
                    return 0;
                }
                crate::window::WM_FASTPAD_PREVIEW_LINK => {
                    crate::window::preview_host::follow_link(hwnd, lparam);
                    return 0;
                }
                crate::window::WM_FASTPAD_PREVIEW_HOVER => {
                    crate::window::preview_host::hover_link(hwnd, lparam);
                    return 0;
                }
                crate::window::WM_FASTPAD_PREVIEW_REFRESH => {
                    crate::window::preview_host::refresh(hwnd);
                    return 0;
                }
                _ => {}
            }
            if message == crate::window::WM_FASTPAD_DIAGNOSTIC_PREVIEW
                && unsafe { app_ptr(hwnd) }.is_some_and(|app| unsafe { app.as_ref() }.launch.diagnostic)
            {
                return crate::window::preview_host::diagnostic(hwnd, wparam);
            }
```

5. End of `apply_theme` and end of `apply_editor_settings`: `crate::window::preview_host::refresh_appearance(hwnd);`
6. `set_file_population(hwnd, active)`: when `active` becomes `false`, call `crate::window::preview_host::document_reloaded(hwnd);` after the flag is cleared.

- [ ] **Step 5: Run tests, Clippy, commit**

Run: `cargo test --test markdown_preview -- --test-threads=1` then `cargo clippy --all-targets`
Expected: PASS.

```bash
git add src/window/preview_host.rs src/preview/view.rs src/window/main_window.rs tests/windows/markdown_preview.rs
git commit -m "feat: debounced live preview updates, worker parses, links, and hover text"
```

---

### Task 16: Scroll sync

**Files:**
- Modify: `src/window/preview_host.rs`
- Modify: `src/window/main_window.rs`
- Modify: `tests/windows/markdown_preview.rs`

**Interfaces:**
- Consumes: `ScrollOrigin`, `sync_count` (Task 14); `PreviewView::{scroll_to_line, top_line}` (Task 11); `WM_FASTPAD_PREVIEW_SCROLLED` (Task 11); `SC_UPDATE_V_SCROLL` (Task 2).
- Produces: `preview_host::editor_scrolled(hwnd)`, `preview_host::preview_scrolled(hwnd, line: usize)`.

- [ ] **Step 1: Write failing tests**

```rust
fn long_markdown() -> String {
    (0..400).map(|index| format!("Paragraph {index}\n\n")).collect()
}

#[test]
fn scrolling_the_editor_scrolls_the_preview() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    main.make_markdown(&long_markdown());
    main.command(CommandId::MarkdownPreviewSide);
    let view = main.view().unwrap();
    pump_until("render", Duration::from_secs(3), || view.stats().block_count == 400);
    unsafe {
        SendMessageW(main.editor, crate::editor::scintilla_constants::SCI_SETFIRSTVISIBLELINE, 300, 0);
        windows_sys::Win32::Graphics::Gdi::UpdateWindow(main.editor);
    }
    pump_until("preview follows", Duration::from_secs(3), || view.top_line() >= 280);
}

#[test]
fn scrolling_the_preview_scrolls_the_editor_without_echo() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    main.make_markdown(&long_markdown());
    main.command(CommandId::MarkdownPreviewSide);
    let view = main.view().unwrap();
    pump_until("render", Duration::from_secs(3), || view.stats().block_count == 400);
    let before = main.with_app(|app| app.preview.sync_count);
    unsafe {
        SendMessageW(view.hwnd(), WM_KEYDOWN, windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_NEXT as usize, 0);
    }
    pump_until("editor follows", Duration::from_secs(3), || {
        unsafe { SendMessageW(main.editor, crate::editor::scintilla_constants::SCI_GETFIRSTVISIBLELINE, 0, 0) } > 0
    });
    pump_for(Duration::from_millis(250));
    let syncs = main.with_app(|app| app.preview.sync_count) - before;
    assert!(syncs <= 2, "scroll sync echoed {syncs} times");
}

#[test]
fn full_mode_keeps_the_editor_position() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    main.make_markdown(&long_markdown());
    unsafe { SendMessageW(main.editor, crate::editor::scintilla_constants::SCI_SETFIRSTVISIBLELINE, 100, 0) };
    main.command(CommandId::MarkdownPreviewFull);
    let view = main.view().unwrap();
    unsafe { SendMessageW(view.hwnd(), WM_KEYDOWN, windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_END as usize, 0) };
    pump_for(Duration::from_millis(300));
    main.command(CommandId::MarkdownPreviewSide);
    assert_eq!(unsafe { SendMessageW(main.editor, crate::editor::scintilla_constants::SCI_GETFIRSTVISIBLELINE, 0, 0) }, 100);
}
```

Run: `cargo test --test markdown_preview scroll -- --test-threads=1`
Expected: FAIL (no sync).

- [ ] **Step 2: Implement**

In `src/window/preview_host.rs`:

```rust
/// `SCN_UPDATEUI` with a vertical scroll: move the preview to the editor's top line, unless this
/// scroll is the echo of a preview-initiated one.
pub(crate) fn editor_scrolled(hwnd: HWND) {
    if mode(hwnd) != PreviewMode::Split || !preview_shown(hwnd) {
        return;
    }
    let echo = with_host(hwnd, |host| std::mem::take(&mut host.scroll_origin) == ScrollOrigin::Preview).unwrap_or(false);
    if echo {
        return;
    }
    let (Some(view), Some(editor)) = (view(hwnd), editor(hwnd)) else {
        return;
    };
    if let Ok(line) = editor.first_visible_line().and_then(|line| editor.doc_line_from_visible(line)) {
        view.scroll_to_line(line);
        with_host(hwnd, |host| host.sync_count += 1);
    }
}

/// `WM_FASTPAD_PREVIEW_SCROLLED`: the user scrolled the preview; move the editor.
pub(crate) fn preview_scrolled(hwnd: HWND, line: usize) {
    if mode(hwnd) != PreviewMode::Split || !preview_shown(hwnd) {
        return;
    }
    let Some(editor) = editor(hwnd) else {
        return;
    };
    let Ok(target) = editor.visible_from_doc_line(line) else {
        return;
    };
    if editor.first_visible_line().ok() == Some(target) {
        return;
    }
    // Only set the guard when Scintilla will actually scroll and so send the echo that clears it.
    with_host(hwnd, |host| {
        host.scroll_origin = ScrollOrigin::Preview;
        host.sync_count += 1;
    });
    let _ = editor.set_first_visible_line(target);
}
```

`std::mem::take` on `ScrollOrigin` requires `Default` (already derived, default `None`).

In `src/window/main_window.rs`:
1. `handle_editor_notification`, `SCN_UPDATEUI` branch: before `return`, read the notification as `ScintillaNotification` and, when `updated as u32 & SC_UPDATE_V_SCROLL != 0`, call `crate::window::preview_host::editor_scrolled(hwnd)`.
2. Default arm: `crate::window::WM_FASTPAD_PREVIEW_SCROLLED => { crate::window::preview_host::preview_scrolled(hwnd, wparam); return 0; }`.

- [ ] **Step 3: Run tests, Clippy, commit**

Run: `cargo test --test markdown_preview -- --test-threads=1` then `cargo clippy --all-targets`
Expected: PASS.

```bash
git add src/window/preview_host.rs src/window/main_window.rs tests/windows/markdown_preview.rs
git commit -m "feat: two-way scroll sync between editor and Markdown preview"
```

---

### Task 17: Preview accessibility

**Files:**
- Create: `src/preview/accessible.rs`
- Modify: `src/preview/mod.rs` (add `pub mod accessible;`)
- Modify: `src/window/accessibility.rs` (share the vtable type and helpers)
- Modify: `src/preview/view.rs` (`WM_GETOBJECT`)

**Interfaces:**
- Consumes: `VisibleLink`, `WM_FASTPAD_PREVIEW_ACTIVATE`, `PreviewView::accessible_links` (Task 11).
- Produces:
  - In `accessibility.rs`, made `pub(crate)`: `AccessibleVtable` (and all its fields), `RawVariant`, trait `VariantValue` (and its methods), `allocate_bstr`, `guid_eq`, `IID_IUNKNOWN`, `IID_IDISPATCH`, `IID_IACCESSIBLE`, and the provider-independent functions `accessible_get_type_info_count`, `accessible_get_type_info`, `accessible_get_ids_of_names`, `accessible_invoke`, `accessible_get_parent`, `accessible_get_help_topic`.
  - In `preview/accessible.rs`: `pub fn object_result(hwnd: HWND, links: Arc<RwLock<Vec<VisibleLink>>>, wparam: WPARAM) -> LRESULT`; `#[cfg(test)] fn create_provider(hwnd: HWND, links: Arc<RwLock<Vec<VisibleLink>>>) -> *mut c_void`.

- [ ] **Step 1: Write failing tests**

Create `src/preview/accessible.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Foundation::{SysFreeString, SysStringLen};

    fn links() -> Arc<RwLock<Vec<VisibleLink>>> {
        let rect = RECT { left: 10, top: 10, right: 60, bottom: 30 };
        Arc::new(RwLock::new(vec![
            VisibleLink { text: "site".into(), dest: "https://x.dev".into(), rect },
            VisibleLink { text: "notes".into(), dest: "notes.md".into(), rect: RECT { top: 40, bottom: 60, ..rect } },
        ]))
    }

    fn read_bstr(value: BSTR) -> String {
        let text = unsafe { std::slice::from_raw_parts(value, SysStringLen(value) as usize) };
        let result = String::from_utf16_lossy(text);
        unsafe { SysFreeString(value) };
        result
    }

    #[test]
    fn the_preview_is_a_document_whose_children_are_its_links() {
        let provider = create_provider(std::ptr::null_mut(), links());
        let table = &PREVIEW_VTABLE;
        unsafe {
            let mut count = 0;
            assert_eq!((table.get_acc_child_count)(provider, &mut count), S_OK);
            assert_eq!(count, 2);

            let mut name = std::ptr::null_mut();
            assert_eq!((table.get_acc_name)(provider, RawVariant::integer(0), &mut name), S_OK);
            assert_eq!(read_bstr(name), "Markdown preview");
            assert_eq!((table.get_acc_name)(provider, RawVariant::integer(2), &mut name), S_OK);
            assert_eq!(read_bstr(name), "notes");

            let mut value = std::ptr::null_mut();
            assert_eq!((table.get_acc_value)(provider, RawVariant::integer(1), &mut value), S_OK);
            assert_eq!(read_bstr(value), "https://x.dev");

            let mut role = RawVariant::empty();
            assert_eq!((table.get_acc_role)(provider, RawVariant::integer(0), &mut role), S_OK);
            assert_eq!(role.child_id(), Some(ROLE_SYSTEM_DOCUMENT as i32));
            assert_eq!((table.get_acc_role)(provider, RawVariant::integer(1), &mut role), S_OK);
            assert_eq!(role.child_id(), Some(ROLE_SYSTEM_LINK as i32));

            let mut action = std::ptr::null_mut();
            assert_eq!((table.get_acc_default_action)(provider, RawVariant::integer(1), &mut action), S_OK);
            assert_eq!(read_bstr(action), "Jump");

            assert_eq!((table.get_acc_name)(provider, RawVariant::integer(3), &mut name), E_INVALIDARG);
            (table.release)(provider);
        }
    }
}
```

Add `pub mod accessible;` to `src/preview/mod.rs`.

Run: `cargo test --lib preview::accessible`
Expected: FAIL to compile.

- [ ] **Step 2: Share the building blocks**

In `src/window/accessibility.rs`, change the listed items to `pub(crate)` (the vtable struct's fields too). Do not change their behavior.

- [ ] **Step 3: Implement the provider**

Above the tests:

```rust
//! MSAA for the preview: a document object named "Markdown preview" whose children are the links
//! currently on screen. It reads the view's link snapshot (safe from any thread) and activates a
//! link by posting to the preview window, which follows it on the UI thread.

use crate::preview::view::VisibleLink;
use crate::window::accessibility::{
    AccessibleVtable, IID_IACCESSIBLE, IID_IDISPATCH, IID_IUNKNOWN, RawVariant, VariantValue,
    accessible_get_help_topic, accessible_get_ids_of_names, accessible_get_parent,
    accessible_get_type_info, accessible_get_type_info_count, accessible_invoke, allocate_bstr,
    guid_eq,
};
use crate::window::messages::WM_FASTPAD_PREVIEW_ACTIVATE;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, RwLock};
use windows_sys::Win32::Foundation::{
    E_INVALIDARG, E_NOINTERFACE, E_NOTIMPL, HWND, LRESULT, RECT, S_FALSE, S_OK, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{ClientToScreen, ScreenToClient};
use windows_sys::Win32::UI::Accessibility::{LresultFromObject, ROLE_SYSTEM_DOCUMENT, ROLE_SYSTEM_LINK};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowRect, PostMessageW};
use windows_sys::core::{BSTR, GUID, HRESULT};

const STATE_SYSTEM_FOCUSED: u32 = 0x0000_0004;
const STATE_SYSTEM_READONLY: u32 = 0x0000_0040;
const STATE_SYSTEM_FOCUSABLE: u32 = 0x0010_0000;
const STATE_SYSTEM_LINKED: u32 = 0x0040_0000;

#[repr(C)]
struct PreviewAccessible {
    vtable: &'static AccessibleVtable,
    references: AtomicU32,
    hwnd: HWND,
    links: Arc<RwLock<Vec<VisibleLink>>>,
}

pub(crate) static PREVIEW_VTABLE: AccessibleVtable = AccessibleVtable {
    query_interface,
    add_ref,
    release,
    get_type_info_count: accessible_get_type_info_count,
    get_type_info: accessible_get_type_info,
    get_ids_of_names: accessible_get_ids_of_names,
    invoke: accessible_invoke,
    get_acc_parent: accessible_get_parent,
    get_acc_child_count: child_count,
    get_acc_child: child,
    get_acc_name: name,
    get_acc_value: value,
    get_acc_description: empty_text,
    get_acc_role: role,
    get_acc_state: state,
    get_acc_help: empty_text,
    get_acc_help_topic: accessible_get_help_topic,
    get_acc_keyboard_shortcut: empty_text,
    get_acc_focus: focus,
    get_acc_selection: selection,
    get_acc_default_action: default_action,
    acc_select: select,
    acc_location: location,
    acc_navigate: navigate,
    acc_hit_test: hit_test,
    acc_do_default_action: do_default_action,
    put_acc_name: put_text,
    put_acc_value: put_text,
};

fn create_provider(hwnd: HWND, links: Arc<RwLock<Vec<VisibleLink>>>) -> *mut c_void {
    Box::into_raw(Box::new(PreviewAccessible {
        vtable: &PREVIEW_VTABLE,
        references: AtomicU32::new(1),
        hwnd,
        links,
    }))
    .cast()
}

pub fn object_result(hwnd: HWND, links: Arc<RwLock<Vec<VisibleLink>>>, wparam: WPARAM) -> LRESULT {
    let provider = create_provider(hwnd, links);
    let result = unsafe { LresultFromObject(&IID_IACCESSIBLE, wparam, provider) };
    unsafe { release(provider) };
    result
}

unsafe fn item<'a>(this: *mut c_void) -> &'a PreviewAccessible {
    unsafe { &*this.cast::<PreviewAccessible>() }
}

fn snapshot(item: &PreviewAccessible) -> Vec<VisibleLink> {
    item.links.read().unwrap_or_else(|error| error.into_inner()).clone()
}

/// `Some(None)` for the document itself, `Some(Some(link))` for a child, `None` for a bad id.
fn target(item: &PreviewAccessible, child: &RawVariant) -> Option<Option<VisibleLink>> {
    match child.child_id()? {
        0 => Some(None),
        id if id > 0 => snapshot(item).get(id as usize - 1).cloned().map(Some),
        _ => None,
    }
}

unsafe extern "system" fn query_interface(this: *mut c_void, iid: *const GUID, output: *mut *mut c_void) -> HRESULT {
    if iid.is_null() || output.is_null() {
        return E_INVALIDARG;
    }
    let requested = unsafe { *iid };
    if guid_eq(&requested, &IID_IUNKNOWN) || guid_eq(&requested, &IID_IDISPATCH) || guid_eq(&requested, &IID_IACCESSIBLE) {
        unsafe {
            *output = this;
            add_ref(this);
        }
        S_OK
    } else {
        unsafe { *output = std::ptr::null_mut() };
        E_NOINTERFACE
    }
}

unsafe extern "system" fn add_ref(this: *mut c_void) -> u32 {
    unsafe { item(this) }.references.fetch_add(1, Ordering::Relaxed) + 1
}

unsafe extern "system" fn release(this: *mut c_void) -> u32 {
    let remaining = unsafe { item(this) }.references.fetch_sub(1, Ordering::Release) - 1;
    if remaining == 0 {
        drop(unsafe { Box::from_raw(this.cast::<PreviewAccessible>()) });
    }
    remaining
}

unsafe extern "system" fn child_count(this: *mut c_void, count: *mut i32) -> HRESULT {
    if count.is_null() {
        return E_INVALIDARG;
    }
    unsafe { *count = snapshot(item(this)).len() as i32 };
    S_OK
}

unsafe extern "system" fn child(this: *mut c_void, child: RawVariant, output: *mut *mut c_void) -> HRESULT {
    if output.is_null() {
        return E_INVALIDARG;
    }
    unsafe { *output = std::ptr::null_mut() };
    match target(unsafe { item(this) }, &child) {
        Some(Some(_)) => S_FALSE,
        _ => E_INVALIDARG,
    }
}

unsafe extern "system" fn name(this: *mut c_void, child: RawVariant, output: *mut BSTR) -> HRESULT {
    match target(unsafe { item(this) }, &child) {
        Some(None) => unsafe { allocate_bstr("Markdown preview", output) },
        Some(Some(link)) => unsafe { allocate_bstr(&link.text, output) },
        None => E_INVALIDARG,
    }
}

unsafe extern "system" fn value(this: *mut c_void, child: RawVariant, output: *mut BSTR) -> HRESULT {
    match target(unsafe { item(this) }, &child) {
        Some(None) => unsafe { allocate_bstr("", output) },
        Some(Some(link)) => unsafe { allocate_bstr(&link.dest, output) },
        None => E_INVALIDARG,
    }
}

unsafe extern "system" fn empty_text(this: *mut c_void, child: RawVariant, output: *mut BSTR) -> HRESULT {
    match target(unsafe { item(this) }, &child) {
        Some(_) => unsafe { allocate_bstr("", output) },
        None => E_INVALIDARG,
    }
}

unsafe extern "system" fn role(this: *mut c_void, child: RawVariant, output: *mut RawVariant) -> HRESULT {
    if output.is_null() {
        return E_INVALIDARG;
    }
    let role = match target(unsafe { item(this) }, &child) {
        Some(None) => ROLE_SYSTEM_DOCUMENT,
        Some(Some(_)) => ROLE_SYSTEM_LINK,
        None => return E_INVALIDARG,
    };
    unsafe { *output = RawVariant::integer(role as i32) };
    S_OK
}

unsafe extern "system" fn state(this: *mut c_void, child: RawVariant, output: *mut RawVariant) -> HRESULT {
    if output.is_null() {
        return E_INVALIDARG;
    }
    let item = unsafe { item(this) };
    let state = match target(item, &child) {
        Some(None) => {
            let focused = if !item.hwnd.is_null() && unsafe { GetFocus() } == item.hwnd { STATE_SYSTEM_FOCUSED } else { 0 };
            STATE_SYSTEM_READONLY | STATE_SYSTEM_FOCUSABLE | focused
        }
        Some(Some(_)) => STATE_SYSTEM_LINKED | STATE_SYSTEM_FOCUSABLE,
        None => return E_INVALIDARG,
    };
    unsafe { *output = RawVariant::integer(state as i32) };
    S_OK
}

unsafe extern "system" fn focus(this: *mut c_void, output: *mut RawVariant) -> HRESULT {
    if output.is_null() {
        return E_INVALIDARG;
    }
    let item = unsafe { item(this) };
    if !item.hwnd.is_null() && unsafe { GetFocus() } == item.hwnd {
        unsafe { *output = RawVariant::integer(0) };
        S_OK
    } else {
        unsafe { *output = RawVariant::empty() };
        S_FALSE
    }
}

unsafe extern "system" fn selection(_this: *mut c_void, output: *mut RawVariant) -> HRESULT {
    if output.is_null() {
        return E_INVALIDARG;
    }
    unsafe { *output = RawVariant::empty() };
    S_FALSE
}

unsafe extern "system" fn default_action(this: *mut c_void, child: RawVariant, output: *mut BSTR) -> HRESULT {
    match target(unsafe { item(this) }, &child) {
        Some(Some(_)) => unsafe { allocate_bstr("Jump", output) },
        Some(None) => unsafe { allocate_bstr("", output) },
        None => E_INVALIDARG,
    }
}

unsafe extern "system" fn select(_this: *mut c_void, _flags: i32, _child: RawVariant) -> HRESULT {
    E_NOTIMPL
}

unsafe extern "system" fn location(this: *mut c_void, left: *mut i32, top: *mut i32, width: *mut i32, height: *mut i32, child: RawVariant) -> HRESULT {
    if left.is_null() || top.is_null() || width.is_null() || height.is_null() {
        return E_INVALIDARG;
    }
    let item = unsafe { item(this) };
    let rect = match target(item, &child) {
        Some(None) => {
            let mut window = RECT::default();
            if item.hwnd.is_null() || unsafe { GetWindowRect(item.hwnd, &mut window) } == 0 {
                return S_FALSE;
            }
            window
        }
        Some(Some(link)) => {
            let mut origin = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
            if item.hwnd.is_null() || unsafe { ClientToScreen(item.hwnd, &mut origin) } == 0 {
                return S_FALSE;
            }
            RECT { left: link.rect.left + origin.x, top: link.rect.top + origin.y, right: link.rect.right + origin.x, bottom: link.rect.bottom + origin.y }
        }
        None => return E_INVALIDARG,
    };
    unsafe {
        *left = rect.left;
        *top = rect.top;
        *width = rect.right - rect.left;
        *height = rect.bottom - rect.top;
    }
    S_OK
}

unsafe extern "system" fn navigate(_this: *mut c_void, _direction: i32, _start: RawVariant, output: *mut RawVariant) -> HRESULT {
    if !output.is_null() {
        unsafe { *output = RawVariant::empty() };
    }
    E_NOTIMPL
}

unsafe extern "system" fn hit_test(this: *mut c_void, x: i32, y: i32, output: *mut RawVariant) -> HRESULT {
    if output.is_null() {
        return E_INVALIDARG;
    }
    let item = unsafe { item(this) };
    let mut point = windows_sys::Win32::Foundation::POINT { x, y };
    if item.hwnd.is_null() || unsafe { ScreenToClient(item.hwnd, &mut point) } == 0 {
        unsafe { *output = RawVariant::empty() };
        return S_FALSE;
    }
    let id = snapshot(item)
        .iter()
        .position(|link| point.x >= link.rect.left && point.x < link.rect.right && point.y >= link.rect.top && point.y < link.rect.bottom)
        .map_or(0, |index| index as i32 + 1);
    unsafe { *output = RawVariant::integer(id) };
    S_OK
}

unsafe extern "system" fn do_default_action(this: *mut c_void, child: RawVariant) -> HRESULT {
    let item = unsafe { item(this) };
    match (child.child_id(), target(item, &child)) {
        (Some(id), Some(Some(_))) => {
            unsafe { PostMessageW(item.hwnd, WM_FASTPAD_PREVIEW_ACTIVATE, id as usize - 1, 0) };
            S_OK
        }
        _ => E_INVALIDARG,
    }
}

unsafe extern "system" fn put_text(_this: *mut c_void, _child: RawVariant, _value: BSTR) -> HRESULT {
    E_NOTIMPL
}
```

If `accessible_get_ids_of_names` or another shared function reads provider state (inspect its body), write a local version that returns `E_NOTIMPL` instead of sharing it.

- [ ] **Step 4: Answer `WM_GETOBJECT` in the view**

In `preview_proc` in `src/preview/view.rs`, add:

```rust
        windows_sys::Win32::UI::WindowsAndMessaging::WM_GETOBJECT
            if lparam as i32 == windows_sys::Win32::UI::WindowsAndMessaging::OBJID_CLIENT =>
        {
            match with_state(hwnd, |state| Arc::clone(&state.accessible)) {
                Some(links) => crate::preview::accessible::object_result(hwnd, links, wparam),
                None => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
            }
        }
```

- [ ] **Step 5: Run tests, Clippy, commit**

Run: `cargo test --lib preview::accessible window::accessibility` then `cargo clippy --all-targets`
Expected: PASS.

```bash
git add src/preview/mod.rs src/preview/accessible.rs src/preview/view.rs src/window/accessibility.rs
git commit -m "feat: MSAA document and link objects for the Markdown preview"
```

---

### Task 18: Measurements, documentation, and final verification

**Files:**
- Modify: `tests/windows/markdown_preview.rs` (ignored performance tests)
- Modify: `src/bin/fastpad-bench.rs`, `tools/benchmark.ps1` (`--launch-file`)
- Modify: `README.md`, `benchmarks/README.md`

**Interfaces:**
- Consumes: everything above.
- Produces: `fastpad-bench --launch-file PATH`; `tools/benchmark.ps1 -LaunchFile PATH`.

- [ ] **Step 1: Performance tests (ignored by default)**

Append to `tests/windows/markdown_preview.rs`:

```rust
fn sample_markdown(bytes: usize) -> String {
    let section = "## Heading\n\nParagraph with **bold**, *emphasis*, `code`, and a [link](https://x.dev).\n\n- item one\n- item two\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n```rust\nfn f() {}\n```\n\n";
    section.repeat(bytes / section.len() + 1)[..bytes].rsplit_once("\n\n").map_or_else(String::new, |(text, _)| format!("{text}\n"))
}

fn p95(mut samples: Vec<u64>) -> u64 {
    samples.sort_unstable();
    samples[(samples.len() * 95 / 100).min(samples.len() - 1)]
}

#[test]
#[ignore = "performance measurement: cargo test --release --test markdown_preview -- --ignored --test-threads=1"]
fn opening_a_100_kb_preview_renders_within_50_ms_p95() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    main.make_markdown(&sample_markdown(100_000));
    let mut samples = Vec::new();
    for _ in 0..30 {
        main.command(CommandId::MarkdownPreviewSide);
        let view = main.view().unwrap();
        pump_until("first frame", Duration::from_secs(5), || view.stats().first_frame_micros > 0);
        samples.push(view.stats().first_frame_micros);
        main.command(CommandId::MarkdownPreviewClose);
    }
    let p95 = p95(samples);
    println!("preview open p95: {p95} us");
    assert!(p95 < 50_000);
}

#[test]
#[ignore = "performance measurement: cargo test --release --test markdown_preview -- --ignored --test-threads=1"]
fn one_paragraph_updates_in_a_1_mb_document_within_2_ms_p95() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    main.make_markdown(&sample_markdown(1_000_000));
    main.command(CommandId::MarkdownPreviewSide);
    let view = main.view().unwrap();
    pump_until("initial render", Duration::from_secs(10), || view.stats().block_count > 0);
    unsafe { SendMessageW(main.editor, crate::editor::scintilla_constants::SCI_GOTOPOS, 500_000, 0) };
    let mut samples = Vec::new();
    for _ in 0..50 {
        let revision = view.stats().revision;
        type_text(main.editor, "x");
        pump_until("update", Duration::from_secs(5), || view.stats().revision > revision && view.stats().last_update_micros > 0);
        pump_for(Duration::from_millis(20));
        samples.push(view.stats().last_update_micros);
    }
    let p95 = p95(samples);
    println!("incremental update p95: {p95} us");
    assert!(p95 < 2_000);
}

#[test]
#[ignore = "performance measurement: cargo test --release --test markdown_preview -- --ignored --test-threads=1"]
fn typing_with_split_open_costs_the_same_as_without_a_preview() {
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    main.make_markdown(&sample_markdown(1_000_000));
    unsafe { SendMessageW(main.editor, crate::editor::scintilla_constants::SCI_GOTOPOS, 500_000, 0) };
    let measure = |main: &TestMain| {
        let mut samples = Vec::new();
        for _ in 0..200 {
            let started = Instant::now();
            type_text(main.editor, "x");
            unsafe { windows_sys::Win32::Graphics::Gdi::UpdateWindow(main.editor) };
            samples.push(started.elapsed().as_micros() as u64);
        }
        p95(samples)
    };
    let baseline = measure(&main);
    main.command(CommandId::MarkdownPreviewSide);
    pump_for(Duration::from_millis(500));
    let with_preview = measure(&main);
    println!("keystroke p95: off {baseline} us, split {with_preview} us");
    assert!(with_preview <= baseline + baseline / 10 + 100);
}

#[test]
#[ignore = "performance measurement: cargo test --release --test markdown_preview -- --ignored --test-threads=1"]
fn closing_the_preview_returns_memory() {
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    fn private_bytes() -> u64 {
        let mut counters = PROCESS_MEMORY_COUNTERS_EX { cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32, ..Default::default() };
        unsafe { GetProcessMemoryInfo(GetCurrentProcess(), (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX).cast(), counters.cb) };
        counters.PrivateUsage as u64
    }
    let _scintilla = support::win32::WindowHarness::new().unwrap();
    let main = TestMain::new();
    main.make_markdown(&sample_markdown(1_000_000));
    pump_for(Duration::from_millis(300));
    let never_opened = private_bytes();
    main.command(CommandId::MarkdownPreviewSide);
    let view = main.view().unwrap();
    pump_until("render", Duration::from_secs(10), || view.stats().block_count > 0);
    pump_for(Duration::from_millis(300));
    let open = private_bytes();
    main.command(CommandId::MarkdownPreviewClose);
    pump_for(Duration::from_millis(500));
    let closed = private_bytes();
    println!("private bytes: never {never_opened}, open {open}, closed {closed}");
    assert!(closed.saturating_sub(never_opened) < 2 * 1024 * 1024);
}
```

Also append this out-of-process check (not ignored; it guards the "zero startup cost" rule at runtime, complementing the import-table guard):

```rust
#[test]
fn launching_with_a_markdown_file_loads_no_preview_graphics_library() {
    use support::process::{FastPadProcess, process_has_module_loaded};
    let path = std::env::temp_dir().join(format!("fastpad-startup-{}.md", std::process::id()));
    std::fs::write(&path, "# Title\n\nBody with a [link](https://x.dev).\n").unwrap();
    let mut process = FastPadProcess::spawn([std::ffi::OsStr::new("--new-window"), path.as_os_str()]).unwrap();
    process.wait_for_main_window(Duration::from_secs(2)).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    for module in ["d2d1.dll", "dwrite.dll", "windowscodecs.dll"] {
        assert!(
            !process_has_module_loaded(process.id(), module).unwrap(),
            "{module} was loaded before any preview was opened"
        );
    }
    process.close().unwrap();
    let _ = std::fs::remove_file(path);
}
```

Scintilla loads `D2D1.DLL`/`DWRITE.DLL` only when DirectWrite technology is requested, which FastPad never does. If this test fails, find what loaded the module (for example with Process Monitor) and report it; do not remove the module from the list.

Run: `cargo build --release` then `cargo test --test markdown_preview launching_with_a_markdown_file -- --test-threads=1`
Expected: PASS.

- [ ] **Step 2: Run the measurements**

Run: `cargo test --release --test markdown_preview -- --ignored --test-threads=1 --nocapture`
Expected: all four PASS; record the printed numbers for the final report. If one fails, report the numbers and the failing target instead of loosening the assertion.

- [ ] **Step 3: Startup benchmark with a Markdown launch file**

In `src/bin/fastpad-bench.rs`:
1. Add `launch_file: Option<PathBuf>` to `Action::Run`; in `parse_args`, accept `"--launch-file"` with a value (`launch_file = Some(PathBuf::from(value))`), and extend `command_line_supports_run_and_compare_modes` with an assertion that `--launch-file notes.md` parses into `Some("notes.md")`.
2. Thread `launch_file: Option<&Path>` through `run_distribution` into both `run_once` definitions.
3. Where the child command line is built (`format!("\"{}\" --diagnostic", executable.display())`), append `launch_file.map(|path| format!(" \"{}\"", path.display())).unwrap_or_default()`.

In `tools/benchmark.ps1`: add `[string]$LaunchFile` to `param(...)` and, after the `EnforceReference` line, `if ($LaunchFile) { $BenchmarkArguments += @("--launch-file", $LaunchFile) }`.

Run: `cargo test --bin fastpad-bench command_line_supports_run_and_compare_modes`
Expected: PASS.

Then measure (reference machine, release): create `benchmarks/fixtures/sample.md` containing the Step 1 `sample_markdown(20_000)` text (commit it), and run on the pre-preview baseline commit (create it with `git worktree add ..\fastpad-baseline 01b2548`) and on this branch:

```powershell
./tools/benchmark.ps1 -Runs 100 -Warmup 10 -LaunchFile benchmarks/fixtures/sample.md -Output benchmarks/preview-baseline.jsonl   # in the baseline worktree
./tools/benchmark.ps1 -Runs 100 -Warmup 10 -LaunchFile benchmarks/fixtures/sample.md -Output benchmarks/preview-candidate.jsonl  # on this branch
cargo run --release --bin fastpad-bench -- compare benchmarks/preview-baseline.jsonl benchmarks/preview-candidate.jsonl
./tools/benchmark.ps1 -Runs 100 -Warmup 10 -Output benchmarks/preview-empty-candidate.jsonl
```

Expected: `compare` reports no regression for any milestone. The baseline worktree needs the Step 3 `--launch-file` change too; cherry-pick that commit into it before measuring. Do not commit the `.jsonl` outputs.

- [ ] **Step 4: Documentation**

In `README.md`, add a section after `## JSON tools`:

```markdown
## Markdown preview

Markdown tabs show two buttons at the right of the title strip: **Open Preview to the Side** and
**Open Preview**. Ctrl+Shift+V cycles between no preview, side by side, and full width; the View
menu and command palette have the same commands. Esc in the full-width preview returns to side by
side. Drag the divider to resize the panes (double-click resets it).

The preview renders GitHub-flavored Markdown natively (tables, task lists, strikethrough, code
blocks, images) and updates shortly after you stop typing. Scrolling either pane scrolls the other.
Only local images are shown. Links open when clicked: web and mail links in your default browser,
`#anchors` inside the preview, and local files in a FastPad tab. Nothing is loaded from the
network. Files larger than 10 MB pause live updates; click the bar at the top of the preview to
refresh it.

The preview's graphics libraries load only when a preview is first opened, so startup is unchanged.
```

In `benchmarks/README.md`, append:

~~~markdown
## Markdown preview

Preview costs are measured in-process with ignored tests:

```powershell
cargo test --release --test markdown_preview -- --ignored --test-threads=1 --nocapture
```

Targets: preview open on 100 KB < 50 ms p95; one-paragraph update in a 1 MB document < 2 ms p95;
keystroke cost with the side-by-side preview open within 10% (+100 µs) of no preview; private
memory within 2 MB of never having opened the preview after closing it.

Startup with a Markdown file is compared against the pre-preview baseline with
`./tools/benchmark.ps1 -LaunchFile benchmarks/fixtures/sample.md` on both builds and
`fastpad-bench compare`.
~~~

- [ ] **Step 5: Final verification (full suite, once)**

Back up `%LocalAppData%\FastPad\fastpad.ini` first (copy it to the job temp directory) and restore it afterwards. Run, in order:

```powershell
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test -- --test-threads=1
pwsh -File tools/audit-dependencies.ps1
cargo build --release
cargo test --release --test markdown_preview binary_does_not_statically_import -- --test-threads=1
pwsh -File tools/package.ps1
pwsh -File tools/verify-package.ps1
```

Expected: every command succeeds. Then run the app manually (`target\release\fastpad.exe --new-window README.md`, choose View > Markdown if needed) and check: both buttons appear; Split, Full, Esc, and close work; typing updates the preview; scrolling syncs; a link opens; switching to a `.txt` tab hides the preview and buttons; theme changes recolor it.

- [ ] **Step 6: Commit**

```bash
git add tests/windows/markdown_preview.rs src/bin/fastpad-bench.rs tools/benchmark.ps1 README.md benchmarks/README.md benchmarks/fixtures/sample.md
git commit -m "docs: Markdown preview usage and measurements"
```
