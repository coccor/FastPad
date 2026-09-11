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

## Remaining verification and review

To be completed after the focused fix and final audit.
