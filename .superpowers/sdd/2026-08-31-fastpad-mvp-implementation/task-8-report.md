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

## Independent-review fix round 1 (2026-09-11)

### Findings, reproduction, and root causes

1. **VARIANT ABI overwrite (Critical).** `VariantData` used `[u64; 2]` as an alignment member,
   making `RawVariant` 24 bytes (8-byte header plus a 16-byte union) instead of the Win32
   `VARIANT` ABI's 16 bytes. The native test duplicated the same oversized declaration, so it
   masked the production overwrite. A focused size assertion first failed with `left: 24,
   right: 16` under:

   ```text
   cargo test window::accessibility::tests::raw_variant_matches_the_win32_variant_abi --lib -- --exact
   0 passed; 1 failed
   ```

2. **Focused-editor menu routing (Critical).** F10/Alt handling lived in the root WndProc even
   though Scintilla owns focus, so those key messages never reached it. The initial focused
   native reproduction posted F10 to the real Scintilla HWND and timed out waiting for the
   five-item transient menu:

   ```text
   cargo test --test titlebar f10_from_the_focused_editor_activates_the_transient_menu -- --exact --nocapture --test-threads=1
   transient menu attached=true was not observed
   0 passed; 1 failed
   ```

   A first keydown-level Alt route exposed a second regression: it attached FastPad's transient
   menu before an Alt+Space chord could reach the native system menu. The added preservation
   test failed red under:

   ```text
   cargo test --test titlebar alt_space_does_not_attach_the_transient_menu -- --exact --nocapture --test-threads=1
   assertion failed: GetMenu(hwnd).is_null()
   0 passed; 1 failed
   ```

3. **Advertised accessibility behavior without state/actions (Important).** `get_accFocus`
   fabricated child 1 while the editor owns focus; every tab reported selected;
   `get_accSelection` was fixed at child 1; `accSelect` accepted every positive ID without a
   change; tab default action said `Select` but did nothing; and Overflow posted command ID 0.
   Focused tests reproduced the three principal semantic failures:

   ```text
   cargo test window::accessibility::tests --lib -- --test-threads=1
   focus_is_not_fabricated_when_the_editor_owns_focus: S_OK instead of S_FALSE
   selection_is_bounded_to_tabs_and_updates_the_selected_state: child 1 instead of child 2
   tabs_expose_close_as_their_default_action: "Select" instead of "Close"
   3 passed; 3 failed
   ```

### Fix design and scope ruling

- The production and native-test `VARIANT` payload union now has one 8-byte alignment member,
  yielding a 16-byte, 8-byte-aligned structure. All VARIANT output paths assign a complete
  `RawVariant::empty()` or `RawVariant::integer()` value, including reserved fields.
- Menu-key recognition moved into the top-level message-pump accelerator route, so messages
  targeted at focused Scintilla are observed. F10 activates immediately. Standalone Alt is
  tracked from keydown to keyup; another key cancels it, preserving Alt+Space and other native
  chords. Only `SC_KEYMENU` with a zero lParam attaches the transient menu, leaving character
  and system-menu requests to `DefWindowProcW`.
- The accessibility provider now owns a bounded atomic selected-tab index initialized from
  `Tabs::active_index`. Exactly one tab reports `STATE_SYSTEM_SELECTED`; `get_accSelection`
  reflects it; and `accSelect` accepts only `SELFLAG_TAKESELECTION` for an in-range tab child.
  Root IDs, button IDs, out-of-range IDs, and unsupported/mixed flags return `E_INVALIDARG`.
  Because focus remains in Scintilla and the title strip has no child HWNDs, `get_accFocus`
  truthfully returns an empty VARIANT with `S_FALSE`.
- Narrow Task 8 ruling for selectable/closable tabs: selection is the MSAA `accSelect` action;
  the tab's default action is `Close`, routed through the existing shared `CloseTab` command.
  That command intentionally remains a focus-preserving no-op until the later file/tab lifecycle
  task, as required by the Task 8 brief. No tab creation/removal lifecycle was added.
- Overflow's default action now posts the existing title-strip Overflow click at its calculated
  center, reusing the production popup path instead of emitting invalid `WM_COMMAND(0)`.
  New/CloseTab use shared `CommandId` values and caption buttons use standard system commands.
- Related simple-child entry points now reject invalid child IDs consistently, and navigation
  rejects invalid starts/directions rather than treating malformed input as an edge.

### Green verification

Focused tests after the fixes:

```text
cargo test window::accessibility::tests --lib -- --test-threads=1
7 passed; 0 failed

cargo test --test titlebar alt_space_does_not_attach_the_transient_menu -- --exact --nocapture --test-threads=1
1 passed; 0 failed

cargo test --test titlebar alt_and_f10_from_the_focused_editor_activate_the_transient_menu -- --exact --nocapture --test-threads=1
1 passed; 0 failed
```

Exact Task 8 verification and broader regression checks:

```text
cargo test window::titlebar::tests --lib -- --test-threads=1
3 passed; 0 failed

cargo test --test titlebar -- --test-threads=1
9 passed; 0 failed

cargo test --all-targets --all-features -- --test-threads=1
52 lib + 13 benchmark-harness + 5 editor-control + 6 startup-smoke + 9 titlebar passed;
0 failed

cargo clippy --all-targets --all-features -- -D warnings
exit 0

cargo fmt --all -- --check
exit 0

git diff --check
exit 0
```

The native suite now checks the 16-byte test-side ABI, marshaled selected state, truthful empty
focus, current selection, Close default action, bounded selection, focused-editor Alt/F10 menu
activation, and non-attachment of the transient menu for Alt+Space.

### Commits, performance, and remaining limitations

- Review-fix implementation commit: `1ed04bc2f3c51dcc64a4097f7b9c1a560c936a41`
  (`fix: harden title shell accessibility and menu keys`).
- Follow-up report metadata commit: reported in the handoff because a commit cannot contain its
  own hash.
- The prior 100-run performance result was not repeated because the review explicitly excluded
  unrelated performance work. The missing baseline still prevents a regression comparison, and
  the previously recorded rendered-input p50 remains above the 25 ms reference target while p95
  remains below the 40 ms absolute ceiling.
- The manual DPI/high-contrast/Narrator/snap/drag matrix remains unperformed. Native automation
  is real process/API coverage, but it is not claimed as hands-on visual or assistive-technology
  observation.

### Fix-round self-review

- Re-read the three findings against the final diff and verified each reported false behavior has
  a focused regression assertion.
- Preserved lazy provider creation, COM ownership/reference counting, Task 6/7 HWND identity and
  reentrancy guards, and the no-child-HWND title-strip architecture.
- Kept changes within Task 8: no performance tuning, Mica, animation, image decoding, or later
  document/tab lifecycle was introduced.

## Scoped re-review fix round 2 (2026-09-11)

### Corrected ruling and ABI red/green evidence

The controller rechecked `windows-sys 0.61.2` rather than relying on the round-1 review's
hand-written 16-byte assumption. Its generated `Win32/System/Variant/mod.rs` includes the
`VARIANT_0_0_0_0 { pvRecord, pRecInfo }` record arm. On this x64 target that makes `VARIANT`
24 bytes with 8-byte alignment: an 8-byte header plus a 16-byte payload union.

The focused regression first enabled the generated Variant/Ole bindings and compared the
round-1 declaration directly with `windows_sys::Win32::System::Variant::VARIANT`:

```text
cargo test window::accessibility::tests::raw_variant_matches_the_win32_variant_abi --lib -- --exact --nocapture
assertion failed: left: 16, right: 24
0 passed; 1 failed
```

Production and the native client test now use the generated `VARIANT` itself; both duplicated
manual representations were removed. Empty and `VT_I4` values start from the binding's fully
zeroed default before the tag/value are written. Unit assertions inspect all reserved header
fields and both halves of the 16-byte payload. The native client allocates a platform-sized
generated `VARIANT`, poisons its reserved fields and second payload half before each result call,
and verifies the provider overwrites the complete value. The focused green ABI/accessibility run
passed 7/7 tests.

### Application-owned selection and bounded actions

The root cause of the remaining selection defect was a provider-private `AtomicUsize` copied
from `Tabs::active_index` when MSAA was first requested. `accSelect` changed that copy, while the
App/title paint snapshot continued reading the unchanged `Tabs` value.

`Tabs` now owns a cloneable atomic `TabSelection` handle. The App passes that handle to the lazy
provider, `accSelect` validates both the exact `SELFLAG_TAKESELECTION` flag and an in-range tab
child, and successful selection updates the shared model. The existing title paint path reads
`Tabs::active_index`, so it observes the same value; the provider also invalidates the window
after a successful change. Focus and selection getters remain truthful and bounded.

The tab-model regression test first failed because no shared selection API existed, then passed
after the model was introduced. The provider regression now demonstrates that selecting child 2
changes the shared model, moves `STATE_SYSTEM_SELECTED`, and rejects root, button,
out-of-range, zero-flag, and mixed-flag selections.

The native test additionally calls the tab's `Close` default action and verifies it is accepted,
while root/out-of-range default actions return `E_INVALIDARG`. It verifies the FastPad window
remains live after the action: `CloseTab` still routes through the shared command model to the
Task-8-mandated focus-preserving no-op. No document/tab creation or close lifecycle was added.

### Verification

```text
cargo test window::titlebar::tests --lib -- --test-threads=1
3 passed; 0 failed

cargo test --test titlebar -- --test-threads=1
9 passed; 0 failed

cargo test --all-targets --all-features -- --test-threads=1
53 library + 13 benchmark-harness + 5 editor-control + 6 startup-smoke + 9 titlebar passed;
0 failed

cargo clippy --all-targets --all-features -- -D warnings
exit 0

cargo fmt --all -- --check
exit 0

git diff --check
exit 0
```

The first strict-Clippy run flagged the native test's integer-to-pointer poison sentinel. It was
replaced with `std::ptr::dangling_mut`, after which strict Clippy passed. The complete test suite
was rerun after that test-only correction.

### Scope and retained limitations

- The resolved focused-editor Alt/F10 route and Alt+Space preservation tests remain unchanged
  and green.
- The prior performance run/baseline limitation and manual DPI, high-contrast, Narrator, snap,
  drag, and caption-interaction limitations were not revisited; this round was explicitly
  limited to ABI and accessibility semantics.
- No performance tuning, visual feature work, child HWNDs, or later-task document lifecycle was
  introduced.
