# Task 9 implementation report

## Scope and base

- Started from the required clean Task 8 base
  `f3ba84429694dee30b9e78b9591f309d714a4999`.
- Implemented only native document-backed tab ownership and its Task 9 command/message lifecycle.
  No open-file, save-file, or language-selection lifecycle was added.
- Preserved the existing `WindowIdentity` checks around reentrant editor creation and added the
  same re-fetch/check discipline around document creation, switching, close review, and shutdown.

## Design and interfaces

- `document.rs` defines the required `DocumentId`, `RecoveryId`, `Language`, `CloseDecision`,
  `CloseCancelled`, and `Document` metadata. `Document` owns exactly one `EditorDocument`; text
  remains wholly inside Scintilla. `Document::test_fixture` and the counted fake handle exist only
  under `cfg(test)` and are absent from normal Windows builds.
- The initial Scintilla document is adopted with `Editor::current_document` during editor
  installation. New documents come from `Editor::create_document`; IDs and recovery IDs are
  monotonically allocated by `App`.
- `Tabs` owns `Vec<Document>` and the Task 8 shared atomic `TabSelection`. It supports lookup,
  activation, canonical-path duplicate rejection, dirty transitions, close decisions, last-tab
  replacement, and explicit shutdown draining. It does not expose mutable native-handle ownership
  separately from document metadata.
- Canonical-path equality uses `Path::canonicalize`; Windows comparison uses a lower-cased
  canonical key so spelling/case aliases cannot establish competing ownership.
- Runtime tab activation updates the shared selection and calls `Editor::use_document`. Title-bar
  tab clicks, close glyphs, and MSAA `accSelect` all converge on that route. The accessibility
  provider is invalidated when tab names/counts change while the shared selection object remains
  authoritative.
- `WM_NOTIFY` accepts save-point notifications only from the installed Scintilla HWND.
  `SCN_SAVEPOINTLEFT` and `SCN_SAVEPOINTREACHED` update only the active document, increment its
  metadata generation on a real transition, invalidate the title strip, and refresh accessibility
  names. Dirty titles use the visible ` *` suffix.
- `CommandId::New` and the plus button create and activate a native document. `CloseTab` and a tab
  close glyph review dirty state, honor Save/Discard/Cancel, select the successor (or preceding tab
  at the end), switch Scintilla, and release the removed tab's one owned reference. Closing the
  final tab creates a fresh native replacement.
- `WM_CLOSE` snapshots dirty documents in tab order and reviews each. Cancel aborts shutdown. Once
  review succeeds, `Tabs::clear_for_shutdown` releases every owned document reference while the
  Scintilla HWND is still live, before `DestroyWindow`.
- The Task 9 close-review Save result represents a confirmed save decision. The file persistence
  operation itself remains outside this task's deliberately excluded save lifecycle.

## TDD evidence

The first required focused run was red:

```text
cargo test document::tests --lib
error[E0599]: Document::test_fixture, Tabs::with_document, Tabs::from_documents,
Tabs::close_active, and related document-tab APIs were absent
test build failed as expected
```

After the minimal metadata/tab implementation, the focused suite passed 7/7. A counted native
reference test was separately introduced red:

```text
cargo test document::tests::closing_a_tab_releases_its_single_owned_native_reference --lib -- --exact
error[E0599]: EditorDocument::test_fixture_with_release_counter was absent
```

The counted test passed after adding the test-only endpoint counter. The final focused model suite
contains nine tests, including shutdown drain and confirmed-save close behavior.

The first native suite compilation caught a test-only `BOOL` import from the wrong module. After
that harness correction, its initial behavioral run passed the New/switch/close, save-point review,
and ordered window-close cases. No behavioral-red claim is made for those already-wired cases.

A later requirements audit found a real selection integration defect and produced a behavioral
red test before its fix:

```text
cargo test --test tabs accessibility_selection_switches_the_native_editor_document -- --exact --nocapture --test-threads=1
timed out waiting for editor text "first"
0 passed; 1 failed
```

Root cause: Task 8's MSAA selection updated the shared index but did not route through the native
editor switch. `accSelect` now posts the selected title-tab click after the bounded shared-state
update. The same command passed 1/1 afterward.

The full suite also exposed an old smoke-test assumption: after typing, `FastPadProcess::close`
could no longer bypass dirty review. The smoke now establishes a save point before cleanup. A
second native teardown failure was traced to calling `FastPadProcess::close` after the reviewed
window had already exited; the helper now returns the already-observed successful exit before
enumerating windows.

## Native and Application Verifier coverage

`tests/windows/tabs.rs` contains an in-binary mutex around every GUI test and is also run with
`--test-threads=1`. It verifies:

- the actual plus-button path creates a native document;
- two documents retain independent Scintilla text across title-tab switching;
- clean close switches to the remaining native document;
- MSAA tab selection switches the native editor, not just the title selection;
- leaving a save point causes dirty close review, Cancel preserves text/window state, and reaching
  the save point permits clean final-tab replacement;
- dirty documents are reviewed in tab order and Cancel aborts the whole window close.

Application Verifier 10.0.26100 was available. `Handles` and `Leak` were enabled specifically for
`fastpad.exe`, the serialized native tab suite was run, the most recent verifier XML session was
exported to `target/task9-appverif.xml`, and all settings were disabled afterward. The run passed
without a verifier stop; the exported session contained no error entries. Unit counters separately
assert one release for a closed owned document and one release per owned document during shutdown
drain.

## Verification

Focused commands used by the brief:

```text
cargo test document::tests --lib
9 passed; 0 failed

cargo test --test tabs -- --test-threads=1
8 passed; 0 failed
```

Complete fresh verification results:

```text
cargo test --all-targets --all-features -- --test-threads=1
62 library + 13 benchmark-harness + 5 editor-control + 6 startup-smoke +
8 tabs + 9 titlebar passed; 0 failed

cargo clippy --all-targets --all-features -- -D warnings
exit 0

cargo fmt --all -- --check
exit 0

git diff --check
exit 0
```

The final Application Verifier rerun included all eight tab-target tests, passed 8/8, exported an
XML session with no error entries, and the subsequent query confirmed every verifier test was
disabled for `fastpad.exe`.

## Commits

- `276d266` - `wip: checkpoint task 9 document model`
- Final implementation commit message: `feat: add Scintilla-backed document tabs`; its ID is
  reported in the handoff because a commit cannot contain its own hash.

## Manual limitations

- The tests exercised real native HWNDs, Scintilla documents, MSAA selection, modal close review,
  and Application Verifier, but no hands-on pointer/keyboard walkthrough or visual inspection was
  performed. Automated button activation is not claimed as manual interaction.
- Application Verifier coverage is the exported latest `fastpad.exe` Handles/Leak session plus
  process exit behavior; Scintilla document references are not kernel handles, so exact document
  reference counts are additionally asserted through the test-only direct endpoint counter.

## Self-review

- Re-read every Task 9 checklist item against the final routes and tests.
- Confirmed text is never copied into the document metadata model.
- Confirmed normal `WM_CLOSE` drains tabs before `DestroyWindow`, while pre-existing emergency
  teardown remains guarded by the Task 6/7 endpoint/window identity state.
- Confirmed tab clicks, close glyphs, accelerators/commands, accessibility selection, title paint,
  and accessibility selected state share the Task 8 models rather than maintaining a second tab
  selection.
- Confirmed test-only fixtures and counters are excluded from non-test builds.
- Remaining scope limitation is intentional: this task models a successful Save close decision but
  does not implement the later file-save operation or save dialog.
