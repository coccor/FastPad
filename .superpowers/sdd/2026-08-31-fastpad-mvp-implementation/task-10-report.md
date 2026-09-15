# Task 10 implementation / investigation report

## Current status: complete with documented verification limitations

Task 10 started from clean required base `6e34ca93348b735e7786ea2de0e04eaa514e2413`.
The loader/deferred core, native Open command, cancellation, and actual selected-file/canonical-
duplicate integration are now green. The controller-resolution appendix below records the final
implementation and complete serial runtime verification. The intervening investigation sections
preserve the earlier paused evidence; their pending/unverified statements are historical and are
superseded by that appendix. No production dialog workaround was introduced.

## TDD evidence

1. Required `cargo test file::loader::tests --lib` was red with E0432: `super::load` absent.
   After implementing `LoadedFile`/buffered `std::fs::read`/`decode`, it passed 3/3: exact BOM
   path/encoding/text, unsupported encoding with no partial text, and missing-file IO error.
2. Native core tests were introduced red with E0599: `App::open_path` absent. After the guarded
   orchestration, serial bootstrap tests passed. The current native tests inspect actual window-
   owned App metadata and Scintilla text, not a mock.
3. `native_open_command_cancellation_preserves_dirty_document` was behaviorally red: no native
   dialog appeared because Open was not handled. After IFileOpenDialog command wiring and a
   bounded outgoing-window wait, `cargo test --test open_file -- --test-threads=1` passed 6/6
   (two application scenarios plus four support tests), before adding selected-file coverage.
4. Self-review introduced `successful_launch_open_posts_language_continuation_only_once` red:
   two APPLY_LANGUAGE messages instead of one. The handler now advances New/error branches,
   while a successful open owns its language post. The same exact regression passed afterward.
5. Selected-file integration was behaviorally red with an unexpected process exit `0xc000041d`.
   This is unresolved successful-selection coverage, not claimed as a passing regression.

## Loader and native core design

- `LoadedFile` preserves the requested PathBuf, decoded String, and detected Encoding.
- `App::open_path` is an internal HWND-based facade. Native orchestration lives beside the
  window-owned raw pointer boundary; no mutable App borrow crosses modal COM Show or population.
- Canonical duplicate paths look up and activate the existing document without decoding or
  overwriting its current dirty text. Comparison uses the existing Task 9 canonical key.
- Disk read, encoding decode, and Scintilla NUL validation occur before native document switching.
  IO/decode/NUL failure leaves active ID, generation, dirty state, path, tab count, and native text
  unchanged; the real-native regression verifies these properties.
- Population is staged in a fresh Scintilla document. Undo collection is disabled, text set,
  undo emptied, undo re-enabled, and save point established. An empty clean untitled tab can reuse
  its metadata identity by replacing its handle at commit; nonempty/dirty input remains in its
  own tab. Notifications and tab/close commands are gated during native population so they cannot
  mutate the previously active document or invalidate a close review against the staged text.
- The transaction retains the previous native handle for rollback and checks the original
  WindowIdentity after reentrant native boundaries. Retired owned handles drop outside App borrows.
- Actual successful loading records FileLoaded and posts deferred language activation only;
  no Save, language implementation, recovery/persistence, or COM work was added to bootstrap.
- A lightweight owned child subclass recognizes the first accepted printable/tab/newline WM_CHAR
  after default processing. A launch Open request without first input parks on a one-shot latch
  instead of busy-reposting. That first input resumes the request with one posted message.
- Existing Task 6/7 pending-input queue prioritization, first-paint latch, Task 8 WindowIdentity,
  and Task 9 tab ownership/close-review guards remain in place.

## Ordering and startup evidence

The real-native bootstrap regression asserts empty editor and absent FileLoaded before the first
WM_CHAR, then exact JSON/path/Utf8Bom/clean state afterward and strict
`FirstInputAccepted < FileLoaded`. It also verifies the original accepted-input tab is retained,
load undo history is empty, and subsequent input can collect undo. The process launch integration
asserts empty text before input and clean decoded JSON afterward. Full metadata/order assertions
currently live in bootstrap library tests because the process UI exposes neither full path nor
encoding; that separation is explicitly noted rather than suggesting a process test read App.

A COM boundary characterization confirms pure App construction and file loading do not change the
thread apartment, and `open_path` does not change the post-native-UI apartment baseline. Native
Windows UI itself can establish an OS-owned implicit apartment, so unchanged apartment state is
not asserted across CreateWindow/Scintilla setup. Source audit finds application CoInitializeEx
and CoCreateInstance only in the Open-command dialog helper; no dialog interfaces are allocated
during App construction or launch-file deferred loading. No timed benchmark/performance envelope
has yet been claimed for this task.

## COM ABI and ownership audit

Authoritative local SDK: Windows Kits 10.0.26100.0 `um/ShObjIdl_core.h`; matching Microsoft
[generated header](https://raw.githubusercontent.com/microsoft/win32metadata/main/generation/WinSDK/RecompiledIdlHeaders/um/ShObjIdl_core.h)
and [common item dialog guidance](https://learn.microsoft.com/en-us/windows/win32/shell/common-file-dialog).

- CLSID_FileOpenDialog comes from windows-sys; IID_IFileOpenDialog is
  `d57c7288-d4ad-4768-be02-9d969532d960`, matching the SDK.
- Hand-written prefixes match IUnknown -> IModalWindow -> IFileDialog: Release slot 2, Show 3,
  SetOptions 9, GetOptions 10, GetResult 20. IShellItem GetDisplayName is slot 5.
- Called methods use `unsafe extern "system"`, HRESULT i32, DWORD u32, HWND pointer, IShellItem
  out interface pointer, and SIGDN i32 / UTF-16 LPWSTR output as prescribed by the SDK.
- Existing flags are obtained before adding `FOS_FORCEFILESYSTEM | FOS_FILEMUSTEXIST`.
- STA CoInitializeEx is scoped to command execution; both S_OK/S_FALSE success are balanced by
  CoUninitialize. Failed initialization returns without uninitializing somebody else's apartment.
- Dialog and selected item are owned interface guards. Output pointers are guarded before their
  HRESULT is checked, so any nonnull owned output also releases on error. Shell path memory is
  guarded with CoTaskMemFree. Reverse drop order frees the string, item, dialog, then COM init.
- Show cancellation `HRESULT_FROM_WIN32(ERROR_CANCELLED)` (`0x800704c7`) returns `Ok(None)`.
  UTF-16 conversion uses OsString::from_wide without lossy path conversion.

## Unexpected selected-file failure: observed root-cause boundary

The original process test used nested filename Edit `WM_SETTEXT`, then dialog Open activation.
Initial timeouts were misleading because the text-reading helper also reports a dead HWND as a
timeout. Explicit GetExitCodeProcess established unexpected `0xc000041d` process exit.

Temporary command-only boundary checkpoints for failing PID 5308 reached:

```
COM init begin -> COM init end -> Show begin
```

No Show end, GetResult, GetDisplayName, RAII release, execute_command return, or App::open_path
checkpoint was reached. The temporary production trace code was removed afterward; evidence
remains in ignored `target/task10-dialog-trace.log`.

Windows Application Error / WER records and fresh user CrashDumps show the original exception was
a NULL read (`0xc0000005`, params 0,0, RCX=0) in `comdlg32.dll + 0x12828`, subsequently wrapped as
`0xc000041d` by the native callback boundary. Matching public Microsoft symbols were downloaded
to `target/task10-symbol-cache`; local DLL PDB GUID/age is
`66A5EE8F2F11757803C8B298707641631`. Noninteractive symbol-stream parsing resolves:

```
comdlg32+0x12828  CFileOpenSave::HandleFileNameDirty +0x68
comdlg32+0x129ba  CFileNameComboBox::_OnCommandMessage +0xda
comdlg32+0x28f37  CComboBoxExBase::OnWinEvent +0x77
comdlg32+0x253f3  CFileOpenSave::s_OpenSaveDlgProc +0x963
```

Changing only the test driver to queued WM_CHAR (with bounded selection/focus and exact filename
acknowledgement) eliminated that crash in subsequent runs, but Open clears the edit and leaves
the expected dialog live. A bounded UI enumeration found only the normal Open dialog, no nested
crash/error/validation UI. The filename button was enabled. This is evidence for a filename
control synchronization/automation issue inside Show; it is an inference, not proof of the exact
shell invariant. Successful selection/path conversion/interface cleanup are still unverified.
No production behavior was changed to suppress the exception or switch dialog APIs.

Proposed next bounded red/green experiment, awaiting direction: configure a selected fixture via
a test-only pre-Show IFileDialog::SetFileName seam, then activate the real dialog. This separates
the shell's edit-control shortcuts from actual Show/GetResult/path ownership. Canonical duplicate
App ownership is already covered by real-native bootstrap tests. A decision is needed whether
that native-library coverage plus process launch/cancellation is acceptable for the integration
checklist, or whether a different physical/UIA process-selection driver is required.

## Verification so far

- Loader focused suite: 3 passed / 0 failed.
- Before first durable checkpoint: serial library suite 75 passed / 0 failed.
- Current focused bootstrap suite: 9 passed / 0 failed (including COM characterization and
  one-shot continuation regression).
- Fresh paused-boundary full serial library suite: 77 passed / 0 failed, after removing temporary
  production trace instrumentation.
- Earlier complete open_file target: 6 passed / 0 failed before adding selected-file experiment.
- Current selected-file experiment: FAIL, expected Open dialog remains live; original driver FAIL
  with shell callback crash. No complete current open_file/all-target runtime passing claim.
- Fresh paused-boundary `cargo test --all-targets --all-features --no-run`: exit 0; all targets
  compile (this is not runtime test coverage).
- Fresh `cargo clippy --all-targets --all-features -- -D warnings`: exit 0.
- Fresh `cargo fmt --all -- --check` and `git diff --check`: exit 0.
- No Application Verifier coverage is claimed for Task 10.

## Commits and manual limitations

- `173f9f3` — `wip: checkpoint task 10 deferred loader core`.
- Investigation checkpoint message: `wip: capture task 10 native dialog investigation`; its hash
  is supplied in handoff. This checkpoint intentionally retains the failing selected-file test
  and the explicit incomplete report; it is not a completed feature commit.
- Required final `feat: defer file opening until after input` is pending actual completion.
- The user confirmed the visible final native dialog was working. That confirms its interaction,
  not a verified successful FastPad load after Show returns. No independent hands-on successful
  selection or visual-layout QA is claimed by this agent.
- Visible launches are paused pending root direction after the unexpected shell failure.
- Test cleanup stopped exact owned sessions; no broad process termination or directory deletion
  was used. Test fixtures remove only their PID/counter-owned temporary directories.

## Self-review / remaining concerns

Scope remains Task 10 only. Preserve native transaction rollback and no App borrow across modal
Show. Do not infer successful COM result release coverage from cancellation. Do not mark this
task complete, add a final feature commit, ignore the selected-file failure silently, or claim
all-target runtime tests passed until successful-selection coverage is resolved and verified.
The temporary trace has been removed from production, but test-driver diagnostic prints and
ignored target dump/symbol tools remain available for the resumed investigation.

## Controller resolution and final verification (supersedes paused status above)

The controller authorized the narrow test-only pre-Show IFileDialog::SetFileName experiment,
explicitly unavailable in production and with production dialog behavior unchanged. The exact
method occupies SDK vtable slot 15 and takes `HRESULT (This, LPCWSTR)` with system calling
convention. Its field remains an unused pointer-sized slot in non-test builds. Test-only filename
configuration and event collection are thread-local, one-shot, and entirely `cfg(test)`.

The open_file integration source-links the exact library sources under cfg(test), rather than
exposing a production configuration API. It creates the real application HWND/Scintilla editor,
executes the actual WM_COMMAND Open handler, invokes real SetFileName before real modal Show,
and activates the native Open button from a bounded worker. It does not write the private shell
edit control. The readiness observer accepts the shell's hidden-known-extension display (this
machine hides `.json`). Initial overly strict readiness assertions were red; observing the actual
extension-hidden filename established that mismatch before narrowing the test observer.

The selected-file regression now passes and observes, after actual calls return:

```
CoInitializeEx -> Show returns -> GetResult -> GetDisplayName
-> CoTaskMemFree -> IShellItem Release -> IFileOpenDialog Release -> CoUninitialize
```

It verifies exact decoded JSON, requested fixture path, Utf8Bom metadata, clean native state,
strict `FirstInputAccepted < FileLoaded`, retained original input tab, and native document-pointer
identity/tab count when opening a canonical `parent/./config.json` alias through a second actual
dialog. Both selection executions assert the complete cleanup sequence. Production-process launch
and command-cancellation coverage remain enabled; cancellation also verifies the original dirty
state. No test is ignored, and all temporary investigation prints/private-edit writes are removed.

This evidences successful Show/result/path/release boundaries with the same production ABI and
rules out those boundaries as necessary causes of the earlier control-shortcut crash. Together
with the symbolized crash inside HandleFileNameDirty before Show returned, it supports the narrow
root-cause conclusion: the original test driver's private filename-control mutation triggered a
shell callback failure. The precise undocumented shell invariant is not claimed to be proven.
Production still uses IFileOpenDialog normally; no API substitution or exception suppression.

The independently discovered one-shot APPLY_LANGUAGE fix and reentrancy hardening are retained:
successful loading owns its continuation, native text queries and retired document drops occur
outside App borrows, modal Show carries only owned WindowIdentity, and population gates close,
tab activation/commands, and edit notifications. Existing startup/deferred-input and Task 9
ownership/review regressions remain green.

Fresh final checks:

- `cargo test file::loader::tests --lib`: 3 passed, 0 failed.
- `cargo test --test open_file -- --test-threads=1`: 84 passed, 0 failed.
- `cargo test --all-targets --all-features -- --test-threads=1`: 206 passed, 0 failed:
  library 77, benchmark 13, editor 5, open_file 84, startup 6, tabs 12, titlebar 9; binary 0.
  The source-linked open_file target includes the 77 library regressions again plus 3 application
  scenarios and 4 support tests; 84 is not claimed as 84 independent application scenarios.
- `cargo clippy --all-targets --all-features -- -D warnings`: exit 0. Two narrow dead-code
  allowances explain integration-only test seam entry points unused by the ordinary lib-test
  target; they do not suppress production warnings or unsafe diagnostics.
- `cargo fmt --all -- --check`, `git diff --check`: exit 0.
- `cargo build --release --bins`: exit 0.

The optional `target/release/fastpad-bench.exe --runs 10 --warmup 2 --output
target/task10-bench.jsonl` smoke sample timed out with Win32 1460 before a record was produced.
The benchmark's owned-child guard cleaned up; a read-only process check found no remaining
FastPad process. No unexpected crash/error dialog appeared during final verification. No numeric
performance envelope or reference-hardware qualification is claimed. Input-before-load timing is
proven by actual QPC milestones in the native regressions, and the source/COM characterization
still demonstrates that application dialog/COM setup is absent from startup.

Durable checkpoints are `173f9f3` (`wip: checkpoint task 10 deferred loader core`) and `d9c54b7`
(`wip: capture task 10 native dialog investigation`). The completed implementation and this report
are committed with the required `feat: defer file opening until after input` message; its final
hash is supplied in handoff because embedding the commit's own hash here would change that hash.

Final self-review: scope is Task 10 only, no Save/language/persistence expansion; all interfaces
and shell allocations have balanced guards; cancellation and error paths preserve prior state;
canonical duplicates preserve ownership and dirty text; startup and stale-HWND defenses remain.
The user confirmed visible dialog interaction, but this agent does not claim independent physical
keyboard/manual selection or visual-layout QA. Successful native selection is automated through
the controller-approved test-only seam and actual modal/result/cleanup calls. Application Verifier
and quantitative performance qualification remain unperformed. Ignored target investigation
artifacts are retained as evidence and are not staged; only this required ignored report is added.
