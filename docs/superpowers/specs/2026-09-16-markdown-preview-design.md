# FastPad Markdown Preview Design

Status: Approved design (pending written-spec review)  
Date: 16 September 2026  
Amends: `2026-08-31-fastpad-mvp-design.md` §2 "Explicitly deferred" (Markdown preview) and §6 dependency policy

## 1. Purpose

Add a live, GitHub-like Markdown preview to FastPad without costing anything on the startup path or while typing. The preview renders proportional text, real tables, and local images natively with Direct2D and DirectWrite. It can sit beside the editor (Split) or replace it (Full).

The MVP architectural rule still governs:

> Nothing not required for the first editable frame may block the first editable frame.

Every preview cost is paid only after the user asks for a preview, and typing never waits on preview work.

## 2. Decisions

| Topic | Decision |
|---|---|
| Fidelity | GitHub-like: proportional text, GFM tables, task lists, strikethrough, local images |
| Parser | `pulldown-cmark`, exact version pin, `default-features = false` |
| Renderer | Direct2D + DirectWrite + WIC through the `windows` crate, interface types only |
| DLL loading | `d2d1.dll`, `dwrite.dll`, WIC loaded on first preview open via `LoadLibraryExW(LOAD_LIBRARY_SEARCH_SYSTEM32)` + `GetProcAddress` / `CoCreateInstance`; never in the import table |
| Layout | Split (side by side, draggable divider) plus Full (preview replaces editor) |
| Network | None. Remote images show a placeholder; links open externally only on click |

Rejected alternatives: WebView2 (browser runtime, memory, first-open latency, audit ban), a read-only Scintilla preview (cannot meet table/image fidelity), pure-Rust text stacks such as `parley`/`cosmic-text` + `tiny-skia` (no ClearType, larger dependency graph, font database scan), and hand-written COM bindings (more custom code for the same result).

## 3. Scope

### In scope (v1)

- Block kinds: ATX/Setext headings, paragraphs, bullet/ordered/task lists (nested), block quotes (nested), fenced and indented code, GFM tables with column alignment, thematic breaks, raw HTML shown as literal text.
- Inline runs: text, strong, emphasis, strikethrough, inline code, links, autolinks, images, hard/soft breaks.
- Live update in Split mode, scroll sync in both directions, theme/DPI/font awareness.
- Title-strip preview buttons, Ctrl+Shift+V, palette and View-menu commands.
- Minimal MSAA accessibility for the preview surface, its links, and the new buttons.

### Out of scope (v1)

Text selection and copy inside the preview, syntax highlighting in code blocks, remote images, HTML rendering, math, Mermaid, footnote rendering beyond literal text, clickable task-list checkboxes, persisting the divider ratio or preview mode, UI Automation text pattern for the rendered content.

## 4. Architecture

New module `src/preview/`. Units are separated so the model and incremental logic are testable without Win32.

| File | Responsibility | Win32 |
|---|---|---|
| `model.rs` | Source text → `Vec<Block>` via `pulldown_cmark::Parser::new_ext(..).into_offset_iter()`. Each `Block` holds its kind, inline runs, source byte range, and source line range. | No |
| `incremental.rs` | Pending-edit log, dirty block span computation, slice reparse, range shifting, full-reparse fallback decisions. | No |
| `links.rs` | Link classification (external, anchor, local file, ignored) and GitHub heading slugs. | No |
| `dwrite.rs` | Lazy loading of D2D/DWrite/WIC, factory ownership. | Yes |
| `layout.rs` | Per-block `IDWriteTextLayout` cache, height estimates, height prefix sums, table column measurement. | Yes |
| `images.rs` | Worker-thread WIC decode to downscaled BGRA buffers, cache keyed by path + modification time, UI-thread `ID2D1Bitmap` creation. | Yes |
| `view.rs` | `FastPadPreview` child window: `ID2D1HwndRenderTarget`, painting visible blocks, scrolling, keyboard, link hit-testing and focus, device-loss recovery. | Yes |
| `colors.rs` | `PreviewColors` per `Theme`, beside `SyntaxColors`. | No |

Changes to existing code:

- `window/main_window.rs`: `PreviewMode { Off, Split, Full }` on the window; layout splits the content area and hosts the divider; `SCN_MODIFIED` appends to the edit log; `SCN_UPDATEUI` drives scroll sync; `PREVIEW_TIMER` handling; tab switch and theme hooks.
- `window/titlebar.rs`: preview buttons and hit targets (§7.2).
- `window/commands.rs`, `window/menus.rs`, `window/command_palette.rs`: three commands and the Ctrl+Shift+V accelerator.
- `window/accessibility.rs`: preview document object, link children, button children.
- `editor/scintilla.rs`: range-pointer, first-visible-line, and visible-line mapping accessors.
- `Cargo.toml`, `tools/audit-dependencies.ps1`, `LICENSES.md`, `licenses/`, `tools/verify-package.ps1`.

Worker threads are per-operation (image decode, large full reparse) and exit when done; there is no permanent pool, consistent with the MVP constraint.

## 5. Performance contract

1. **Zero startup cost.** No preview code runs and no D2D/DWrite/WIC module loads before a preview command. Detecting a `.md` tab only toggles a layout boolean.
2. **Typing never waits.** `SCN_MODIFIED` performs O(1) work: append an edit and re-arm the timer.
3. **Bounded incremental work.** A one-paragraph edit reparses one block, re-lays out one block, and repaints only the visible area.
4. **Visible-only layout.** Opening a preview costs one full parse plus one viewport of layout.
5. **Images never block the UI thread.**
6. **Large documents.** Above 1 MB, full reparses run on a worker with a generation counter that discards stale results. Scintilla's buffer is not shared across threads: the UI thread copies the text once and moves the copy to the worker. Above 10 MB, live updates pause and a "Refresh preview" bar appears.
7. **Closing frees memory.** Hiding the preview releases the render target, layout cache, image cache, and block model; only the factories remain.

Targets are listed in §9.3.

## 6. Data flow

### 6.1 State ownership

- `PreviewMode` is per window. Activating a non-Markdown tab hides the preview and keeps the mode; activating a Markdown tab restores it.
- The block model and layout cache exist only for the active document. A tab switch discards them and schedules a full parse (immediately on the UI thread at or below 1 MB, on a worker above it).

### 6.2 Edit capture

On `SCN_MODIFIED` with `SC_MOD_INSERTTEXT` or `SC_MOD_DELETETEXT` while a preview is visible:

1. Append `(position, removed_len, inserted_len)` to the pending-edit log. If the log already holds 64 entries, set `full_reparse = true` and stop appending.
2. Re-arm `PREVIEW_TIMER` at 120 ms.

Edits during file population are ignored, matching existing notification handling; population completion schedules a full parse.

### 6.3 Update on timer

1. Replay pending edits against block ranges to compute the dirty top-level block span. Expand it to the nearest blank lines outside fenced code.
2. Use a full reparse instead when `full_reparse` is set, or when inserted or removed text contains a fence marker (```` ``` ```` or `~~~`), a table delimiter row, or `]:`, or when the span touches a list or quote whose boundary cannot be proven by blank lines.
3. Read the span with `SCI_GETRANGEPOINTER` (the whole document with `SCI_GETCHARACTERPOINTER` for full reparses).
4. Parse the slice, splice the resulting blocks in, shift later ranges by the byte and line deltas.
5. Invalidate layouts of replaced blocks; repaint only if any replaced block intersects the viewport.
6. Between blocks, check `input_pending()`. If input is pending, keep the unprocessed edits and re-arm the timer.

### 6.4 Virtualized layout

- Blocks without a layout use an estimated height: source line count × body line height.
- A block entering the viewport gets its real layout; the prefix sums update.
- When a real height differs from its estimate, scroll offset is corrected so the first visible block keeps its on-screen position.

### 6.5 Scroll sync

- **Editor → preview:** on `SCN_UPDATEUI` with `SC_UPDATE_V_SCROLL`, map the first visible display line to a document line, binary-search the block by line range, interpolate within the block, and set the preview offset.
- **Preview → editor:** inverse mapping, then `SCI_SETFIRSTVISIBLELINE`.
- A scroll-origin guard ignores the echo from the programmatic scroll it just caused.
- Full mode keeps the editor's position; returning to Split restores it.

### 6.6 Invalidation

| Event | Effect |
|---|---|
| Theme, DPI, `font_face`, `font_size` change | Clear all layouts, keep blocks, repaint |
| Width change | Clear all layouts; during divider drag, draw stale layouts clipped and re-lay out on mouse release |
| Height-only change | Repaint only |
| `D2DERR_RECREATE_TARGET` | Recreate render target and image bitmaps from cached BGRA buffers; text layouts survive |

## 7. User interface

### 7.1 Commands and keys

- **Ctrl+Shift+V** cycles `Off → Split → Full → Off`.
- Palette and View menu: *Markdown Preview: Side by Side*, *Markdown Preview: Full*, *Close Markdown Preview*.
- For non-Markdown tabs the menu entries are disabled; invoking the commands shows a non-blocking notice.
- **Esc** in Full mode returns to Split, unless the find bar or command palette is open, in which case Esc closes that first as it does today.

### 7.2 Title-strip preview buttons

Layout of the title strip while the active tab is Markdown:

```
[tab][tab][tab]   <drag gap>   [side][full] [...] [-][□][x]
```

- Two 40 px (DPI-scaled) buttons between the drag gap and the overflow button: **Open Preview to the Side** (Segoe MDL2 `DockRight`, U+E90D) and **Open Preview** (Segoe MDL2 `Preview`, U+E8FF).
- Shown only while the active tab is Markdown; the tab viewport's right edge moves left by their width, and tabs re-lay out as they do when tabs open or close.
- Toggle behavior: the button matching the current mode is drawn pressed; clicking it sets `Off`; clicking the other button switches to its mode.
- Hover and press colors reuse the caption-button palette. Hover writes the command name and shortcut to the status bar.
- `HitTarget::PreviewSide` and `HitTarget::PreviewFull` are client targets (not caption buttons).
- `TitleBarLayout::calculate` gains a `preview_buttons: bool` input and exposes `preview_side` and `preview_full` as `Option<Rect>`.

### 7.3 Divider and focus

- Divider: 4 px (DPI-scaled), draggable, ratio clamped to 20–80%, double-click resets to 50%. Session-only.
- Clicking the preview or entering Full mode focuses the preview; leaving Full mode via Ctrl+Shift+V focuses the editor.
- Preview keys: Up/Down, PgUp/PgDn, Home/End, mouse wheel scroll; Tab/Shift+Tab move a visible focus ring between links; Enter activates the focused link.

### 7.4 Visual style

- Colors come from `PreviewColors { text, muted, heading, link, code_background, border, quote_bar, table_stripe, background }` per theme. Catppuccin: link = blue, border = surface1, muted = subtext0, code background = mantle. Windows high contrast uses system colors.
- Body: Segoe UI at `font_size`. Code (inline and block): `font_face`.
- Headings: H1 2.0× and H2 1.5× with a bottom rule; H3 1.25×; H4 1.0×; H5 and H6 0.875× in muted color. All bold.
- Code blocks: rounded code background, no wrapping, horizontal scroll inside the block.
- Block quotes: 4 px left bar, muted text.
- Lists: bullets • ◦ ▪ by depth; ordered numbers right-aligned; task checkboxes drawn, read-only.
- Tables: bold header row, 1 px borders, striped rows, column alignment from the delimiter row; a table wider than the content area scrolls horizontally within itself.
- Thematic break: 1 px border-colored line.
- Content padding 16 px; content width capped at 980 px and centered in Full mode.

### 7.5 Links

| Target | Action |
|---|---|
| `http:`, `https:`, `mailto:` | `ShellExecuteW` on click or Enter |
| `#anchor` | Scroll preview to the heading with the matching GitHub slug |
| Relative or absolute local path to an existing file | Open in FastPad through `open_path` |
| Anything else | No action; status notice |

Hover draws an underline, sets the hand cursor, and shows the target in the status bar. Nothing is ever fetched.

### 7.6 Images

- Local paths only, resolved relative to the document's folder. Untitled documents and remote URLs show a placeholder.
- Decoded on a worker thread through WIC, scaled down to content width (never up), cached by path and modification time.
- Until decoded, and when missing or undecodable: a bordered placeholder box showing the alt text.

### 7.7 Accessibility

- The preview is an MSAA object with `ROLE_SYSTEM_DOCUMENT`, name "Markdown preview".
- Each link is a `ROLE_SYSTEM_LINK` child with its text as the name and a default action.
- The title-strip buttons are `ROLE_SYSTEM_PUSHBUTTON` children with names and default actions, alongside the caption buttons.
- The editor remains the complete, accessible source of the document.

## 8. Error handling

- Failure to load D2D, DWrite, or WIC, or to create a factory or render target: the preview stays `Off`, a non-blocking notice names the failure, the editor is unaffected. Later attempts retry.
- Parser: `pulldown-cmark` accepts all input; there is no parse failure path.
- Image errors (missing file, unsupported format, decode failure, oversize above 64 megapixels): placeholder with alt text; no notice.
- `ShellExecuteW` failure: status notice.
- Device loss: recover per §6.6; if recreation fails, treat as a load failure.
- Worker results carry the document id and a generation number; results for another document or an older generation are dropped.

## 9. Testing and benchmarks

### 9.1 Unit tests without Win32

- `model.rs`: one fixture per block kind and inline run kind, asserting kinds, runs, byte ranges, and line ranges.
- `incremental.rs`: seeded edit-sequence property test over fixture documents — after every edit, the incremental block model equals a fresh full parse. Edits include character and line inserts and deletes, fence markers, table delimiter rows, `]:` definitions, and blank lines. The seeded generator lives in the test; no new crate.
- Fallback triggers: fences, delimiter rows, and `]:` force a full reparse; the 65th pending edit sets `full_reparse`.
- Scroll mapping: line → offset → line round-trips within a block; ordering holds with estimated heights.
- `links.rs`: GitHub slug rules (lowercase, punctuation stripped, `-1` suffixes for duplicates) and link classification.

### 9.2 Win32 unit and integration tests

- `TitleBarLayout`: with `preview_buttons = false` all rects equal today's; with `true` the buttons do not overlap each other, the overflow button, or the tab viewport.
- Command palette lists the three preview commands; Ctrl+Shift+V shortcut text.
- New `tests/windows/markdown_preview.rs`:
  - `.md` tab shows the buttons; switching to `.txt` hides them and the preview, and switching back restores the mode.
  - Side → Full → pressed Full transitions Split → Full → Off, preserving editor text, caret, and scroll.
  - Typing in Split updates the preview after the debounce (test hook exposing block count and model revision).
  - Editor scroll moves the preview and vice versa, with a bounded scroll-event count.
  - Theme change repaints with the new `PreviewColors`.
  - Missing image renders a placeholder; clicking a relative `.md` link opens a tab.
  - **Import guard:** the release `FastPad.exe` PE import table contains none of `d2d1.dll`, `dwrite.dll`, `windowscodecs.dll`; a launched instance at input readiness has none of them loaded.

### 9.3 Benchmarks (`fastpad-bench`, documented in `benchmarks/README.md`)

| Measure | Target |
|---|---|
| Warm TTI and first paint, launching with a `.md` path | No statistically material regression versus baseline |
| Preview command → first rendered frame, 100 KB `.md` | < 50 ms p95 |
| One-paragraph incremental update, 1 MB document (timer to paint) | < 2 ms p95 |
| Keystroke → editor paint with Split open, 1 MB document | Equal to preview-off baseline |
| Private working set: never opened / open on 1 MB / closed again | +0 / recorded / within 2 MB of never opened |

### 9.4 Dependency audit and packaging

- `tools/audit-dependencies.ps1` allowed roots gain `pulldown-cmark` (exact version, default features off) and `windows` (exact version). The `windows` name ban is lifted only for that exact root with features limited to `Win32_Graphics_Direct2D`, `Win32_Graphics_Direct2D_Common`, `Win32_Graphics_DirectWrite`, `Win32_Graphics_Imaging`, and `Win32_Graphics_Dxgi_Common` (plus any `Win32_Foundation`/`Win32_System_Com` features those require); any other appearance of `windows` still fails the audit. The network-capable crate scan is unchanged and must pass.
- `LICENSES.md` and `licenses/` include the new crates' license texts; `tools/verify-package.ps1` checks for them.

### 9.5 Run discipline

During implementation: compile through Clippy and run targeted tests only. The full `cargo test -- --test-threads=1` suite and the benchmarks run at final review. `%LocalAppData%\FastPad\fastpad.ini` is backed up and restored around every live-app run.

## 10. Risks

| Risk | Mitigation |
|---|---|
| `windows` crate import stubs pull D2D/DWrite into the import table | Factories created only through `GetProcAddress`/`CoCreateInstance`; import guard test (§9.2) |
| Incremental reparse diverges from full parse | Property test (§9.1); conservative full-reparse triggers |
| Height estimates cause scroll jumps | Anchor-preserving correction (§6.4) |
| Tab strip reflow when switching between Markdown and other tabs is distracting | Accepted for v1; revisit with measured feedback |
| Binary size and compile time growth from `windows` | Narrow feature set; record release binary size before and after |
| Scroll-sync feedback loops | Scroll-origin guard; bounded event-count test |
