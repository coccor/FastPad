# Task 8 implementation report

## Inherited state and checkpoint

- Base before Task 8: `327d9fd3f3cce2a75419596e3ec6f37768c9bef1`.
- The inherited dirty worktree contained Task 8 changes in `Cargo.toml`, `src/app.rs`,
  `src/bootstrap.rs`, `src/window/main_window.rs`, and `src/window/mod.rs`, plus new
  accessibility, command, menu, tab, title-bar, and native title-bar test modules.
- The first exact test run did not compile: `InvalidateRect`, `VK_F10`, and `VK_MENU` were
  imported from the wrong Win32 modules, and `SetBkMode` received the unsigned
  `TRANSPARENT` constant where its ABI expects `i32`.
- After the minimal compile repair, all three inherited geometry tests passed. The native
  test exposed two test/runtime boundary issues. Its accessibility client had not initialized
  COM, so `AccessibleObjectFromWindow` returned the default object; explicit client COM
  initialization proved FastPad's lazy `WM_GETOBJECT` provider token was valid. Its test thread
  was also DPI-unaware, so cross-process client coordinates were virtualized to 96 DPI while
  `GetDpiForWindow` reported 144 DPI. A per-monitor-v2 test-thread context removed that double
  scaling and aligned the calculated maximize center with DWM.
- Caption-button layout now uses the DPI-aware `SM_CXSIZE` system metric rather than forcing a
  46-logical-pixel minimum.
- Coherent WIP checkpoint: `704bd19` (`wip: checkpoint task 8 native title shell`). At that
  checkpoint, `cargo test window::titlebar::tests --lib` passed 3/3 and
  `cargo test --test titlebar -- --test-threads=1` passed 6/6.

## NCCALCSIZE resize-border red/green cycle

The production break named by the added native assertion is: handling either form of
`WM_NCCALCSIZE` as an all-client window removes the standard resize frame, causing the custom
title strip to report a left-edge resize point as `HTCLIENT` instead of `HTLEFT`.

Reproduction command:

```text
cargo test --test titlebar custom_titlebar_preserves_snap_hit_target_and_accessible_children -- --exact --nocapture --test-threads=1
```

Observed red evidence:

```text
custom title strip swallowed the left resize border:
window=(100, 100, 1380, 820), client_origin=(100, 100), client=(0, 0, 1280, 720)
left: 1 (HTCLIENT), right: 10 (HTLEFT)
```

Root cause: `reclaim_caption` handled only the `wParam != 0` `NCCALCSIZE_PARAMS` form. For
`wParam == 0`, it returned zero without calling `DefWindowProcW` or insetting the supplied
`RECT`, so the client rectangle occupied the whole window and no side/bottom resize border
remained.

Minimal hypothesis/fix: for both non-null lParam representations, retain the proposed top,
let `DefWindowProcW` calculate standard resize borders, then replace only the caption inset
with the DPI-aware frame/padded-border thickness. Delegate a null lParam to `DefWindowProcW`.
The focused verification result is recorded below after it is run.

Focused green evidence:

```text
cargo test --test titlebar custom_titlebar_preserves_snap_hit_target_and_accessible_children -- --exact --nocapture --test-threads=1
1 passed; 0 failed

cargo test window::titlebar::tests --lib
3 passed; 0 failed

cargo test --test titlebar -- --test-threads=1
6 passed; 0 failed
```

## Design and wiring

- `TitleBarLayout` is the deterministic geometry model for tabs, close-tab targets, New,
  Overflow, caption drag space, and the three standard caption buttons. Runtime geometry reads
  `GetDpiForWindow` and DPI-aware system caption metrics. The tab/action region is clamped before
  the caption buttons, including at narrow widths.
- `WM_NCCALCSIZE` lets `DefWindowProcW` calculate standard resize borders for both lParam forms,
  then reclaims only the caption while retaining the DPI-aware frame/padded-border top inset.
- `WM_GETMINMAXINFO` maps maximum position and size to the nearest monitor's work area.
- `WM_NCHITTEST` calls `DwmDefWindowProc` first and returns its handled result. Otherwise it
  delegates points outside the retained client frame to `DefWindowProcW`, then maps FastPad
  title targets to `HTCAPTION`, `HTMINBUTTON`, `HTMAXBUTTON`, `HTCLOSE`, or `HTCLIENT`.
- `WM_PAINT` uses `BeginPaint`, `FillRect`, `DrawTextW`, and `EndPaint` for one integrated strip.
  High contrast selects system window/highlight/text colors. The production paint route was
  reconnected to Task 7's post-paint window-identity guard, so a reentrant HWND replacement
  cannot receive the original window's first-paint completion.
- One `CommandId` enum backs accelerator entries, overflow entries, transient
  File/Edit/Search/View/Help menus, and `WM_COMMAND`. The message pump calls the single
  accelerator table before normal translation/dispatch. `App::execute` posts `WM_CLOSE` for
  Exit; later-task commands are intentionally focus-preserving no-ops.
- The transient menu is created only on Alt/F10 menu entry and detached after menu-loop exit.
  Native handles have RAII destruction, while attach/detach uses raw handles copied out of App
  before reentrant Win32 calls.
- `WM_GETOBJECT` creates and caches the title provider only on the first `OBJID_CLIENT` request.
  Its COM vtable and reference count stay in `window/accessibility.rs`. The root reports a page
  tab list; the initial tab reports a page-tab role; New, Overflow, Minimize, Maximize, and Close
  report named push-button roles. A standard MSAA provider is available to UI Automation through
  the Windows legacy accessibility bridge.

## Verification

Focused/native and suite evidence collected before final verification:

```text
cargo test --lib -- --test-threads=1
47 passed; 0 failed

cargo test --test titlebar -- --test-threads=1
7 passed; 0 failed

cargo test --all-targets --all-features -- --test-threads=1
47 lib + 13 benchmark-harness + 5 editor-control + 6 startup-smoke + 7 titlebar passed;
0 failed

cargo clippy --all-targets --all-features -- -D warnings
exit 0

cargo fmt --all -- --check
exit 0 after formatting

git diff --check
exit 0
```

The expanded native test verifies:

- `HTMAXBUTTON` at the calculated maximize center (snap-layout hover contract);
- `HTLEFT` at the retained side resize border;
- maximum position/size equal to the nearest monitor work area;
- a lazily marshaled provider from `WM_GETOBJECT`;
- page-tab-list, page-tab, and five push-button roles plus all six names;
- `WM_COMMAND(CommandId::Exit)` exits through `App::execute`.

## Performance

Command completed with 100 valid measured launches:

```text
cargo run --release --bin fastpad-bench -- --runs 100 --warmup 10 --output benchmarks/titlebar.jsonl
```

Candidate summary:

```text
process_start: p50=9144us p95=10419us
window_created: p50=14102us p95=15184us
editor_created: p50=23118us p95=24666us
first_paint: p50=32575us p95=34882us
first_input_accepted: p50=32257us p95=34268us
first_input_rendered: p50=33505us p95=35937us
settings_loaded: p50=32590us p95=34898us
file_loaded: p50=32599us p95=34908us
fully_ready: p50=32638us p95=34943us
idle_private_working_set_bytes: p50=1675264 p95=1716224
valid_records=100
```

The rendered-input p95 is below the documented 40 ms absolute ceiling, but p50 is 33.505 ms,
above the documented 25 ms reference target. `benchmarks/baseline.jsonl` is absent from the
worktree and repository history, so the required comparison command was run but could not make
a regression decision:

```text
cargo run --release --bin fastpad-bench -- compare benchmarks/baseline.jsonl benchmarks/titlebar.jsonl
fastpad-bench: could not open benchmarks/baseline.jsonl: The system cannot find the file specified. (os error 2)
exit code: 1
```

## Commits

- `704bd19` - `wip: checkpoint task 8 native title shell`
- `bc39d28` - `fix: preserve custom frame resize borders`
- Final implementation commit message: `feat: add editor-first native title shell`; its ID is
  reported in the Task 8 handoff because embedding a commit's own ID in its contents is circular.

## Scope and manual limitations

- No Mica, animations, image decoding, child HWNDs, or later file-lifecycle behavior was added.
- No unrelated user work was reset, stashed, overwritten, or removed.
- This environment allowed real native process/API tests but not trustworthy hands-on GUI or
  assistive-technology observation. Drag, caption double-click maximize, Alt+Space system menu,
  visual output at 100/150/200% DPI, interactive Alt/F10 menu navigation, Narrator focus speech,
  snap-layout hover visuals, and Windows high-contrast visuals were not manually verified.
- Automated coverage exercised native title geometry at the machine's 150% DPI and verified the
  maximize hit target, resize frame, work-area sizing, accessibility roles/names, and Exit route.

## Self-review

- Re-read the binding brief after implementation and checked each required message and command
  route against the final diff.
- Preserved Task 6/7 HWND identity, ownership-transfer, deferred-start, input-priority, and native
  handle lifetime guards. In particular, no App reference is held across menu attach/detach or
  other reentrant Win32 calls.
- Kept accessibility allocation lazy and isolated its COM ABI/refcount implementation.
- Remaining concerns are limited to the missing performance baseline, the absolute TTI p50 miss,
  and the manual GUI/accessibility matrix that cannot be honestly completed in this environment.
