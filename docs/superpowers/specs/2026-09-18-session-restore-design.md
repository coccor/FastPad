# Session Restore Design

Status: Approved design  
Date: 18 September 2026

## 1. Purpose

When FastPad closes, the tabs that were open reopen on the next launch, the way Notepad++ does it. This includes unsaved edits and untitled tabs. Closing with the feature on never prompts. A setting turns the feature off, and then FastPad behaves exactly as it does today.

The MVP design (`2026-08-31-fastpad-mvp-design.md`) left ordinary session restoration out of scope. This spec adds it without breaking the MVP's architectural rule: nothing may block the first editable frame.

## 2. Goals and non-goals

### Goals

- Reopen the previous session's tabs in order, with the active tab and its caret, selection and first visible line. FastPad keeps no per-tab view state (switching tabs already resets it), so only the active tab's view state is captured. The manifest still stores the fields on every entry, with zeros for the inactive ones.
- Keep unsaved text across a normal close, for both file-backed and untitled tabs.
- Leave startup performance unchanged: the first paint and first input happen before any session file is read.
- Never lose data. This covers a crash after restore, a missing or corrupt manifest, and turning the setting off while snapshots still exist.
- Provide a persisted setting to turn the feature off.

### Non-goals

- Named, multiple, or user-saved sessions.
- Handling Windows logoff or shutdown (`WM_QUERYENDSESSION` / `WM_ENDSESSION`). FastPad does not handle these messages today; that is a separate follow-up.
- Session participation for independent instances started with `--new-window` or after an IPC failure.
- Lazy (unloaded) placeholder tabs.
- Detecting that a file-backed tab's file changed on disk between sessions.

## 3. Setting

- New key in `fastpad.ini`: `restore_session`. It accepts the existing boolean spellings (`parse_bool`) and defaults to `true`.
- It gets a field on `Settings` and `SettingsDelta`, handling in `apply_delta` and `apply_line`, a `DEFAULT_RESTORE_SESSION` constant, and an entry in the `parse` doc comment.
- New command `CommandId::ToggleRestoreSession`:
  - Command palette label: "File: Toggle session restore".
  - File menu item: "Restore session on startup".
  - It does not require a document.
- The command goes through `change_setting`, which persists only the `restore_session` line. It then posts a notice with the new state, "Session restore is on" or "Session restore is off", because menus do not show checkmarks yet.
- The setting is read when the window closes. Turning it off during a session makes that close prompt as it does today.

## 4. Participation

Only the primary instance takes part: the one that holds the single-instance mutex (`InstanceClaim::Primary`). An independent instance neither restores nor writes a session, and its close flow is unchanged. That stops concurrent windows from overwriting each other's manifest.

## 5. Session manifest

### Location and format

- The manifest is `%LOCALAPPDATA%\FastPad\session.ini`, next to `fastpad.ini`.
- It is UTF-8, one entry per line, and always written atomically with the same temp-and-rename helper the settings use.

```
version=1
active=2
file=<caret>|<anchor>|<first_visible_line>|C:\path\to\file.txt
snapshot=<caret>|<anchor>|<first_visible_line>|<recovery id, 32 hex digits>
```

### Parsing rules

- `file` and `snapshot` lines appear in tab order.
- The path or ID is always the last field. Windows paths cannot contain `|`, so splitting on the first three `|` characters is unambiguous.
- `active` is a zero-based index into the entry list. If it is out of range, index 0 is used.
- Unknown keys are ignored, and so are malformed entry lines. Any other `version` value makes the whole manifest invalid, and it is ignored.
- Numbers are byte positions and line numbers as Scintilla reports them. Positions past the end of the restored text are clamped.

### Module

The manifest code lives in a new module, `src/session.rs`, which has no window dependencies:

- `Session { active: usize, entries: Vec<SessionEntry> }`
- `SessionEntry { source: SessionSource, caret: usize, anchor: usize, first_line: usize }`
- `SessionSource::File(PathBuf)` or `SessionSource::Snapshot(RecoveryId)`
- `session_file_path() -> Result<PathBuf>`
- `Session::encode(&self) -> String`
- `Session::parse(&str) -> Option<Session>`
- `write(path, &Session) -> Result<()>` (atomic)
- `read(path) -> Option<Session>`
- `remove(path)`

## 6. Close flow (session restore on)

The `WM_CLOSE` handler branches on `restore_session && primary`. Both conditions false or either one false keeps the current flow unchanged: review prompts, `remove_session_snapshots`, and so on.

When both are true:

1. Keep the existing guard (`file_population_active`) and call `drain_ipc_requests`.
2. **Flush snapshots.** For each dirty document where `needs_snapshot` is true, write its snapshot synchronously with the recovery timer's existing snapshot routine. The timer has usually written these already, so this step normally does no work.
3. If any snapshot write fails, fall back to the current prompt flow for the whole close. A dirty tab without a snapshot must not close silently.
4. **Build the manifest** in tab order, skipping entries as follows:
   - A clean document with a path becomes `file=`.
   - A dirty document, with or without a path, becomes `snapshot=` using the ID of the snapshot just flushed or already current. The snapshot file stores the original path.
   - A clean untitled document is skipped.
   - The active index points to the active tab's entry. If the active tab was skipped, it points to the entry before it, or 0.
5. **Write the manifest.** If there are no entries, delete any existing manifest instead. If the write fails, fall back to the prompt flow.
6. **Clean up snapshots.** Call `remove_session_snapshots(hwnd, &[])`. Its existing rules keep every dirty document's own snapshot and its recovery origin, and remove the own snapshot of each clean non-recovered document. A clean recovered document keeps its origin snapshot today. With session restore on, that snapshot is also removed, because the manifest does not reference it.
7. Call `shutdown_ipc`, then `clear_documents_for_shutdown`, then `DestroyWindow`.

Snapshots for dirty tabs stay in the normal `Recovery` folder. If the manifest were lost, crash recovery would still find them once the owner process has exited, so the unsaved text survives.

## 7. Startup flow

### Deferred step

- Add `WM_FASTPAD_RESTORE_SESSION` to the deferred chain between `LOAD_SETTINGS` and `OPEN_REQUEST`:
  `LOAD_SETTINGS → RESTORE_SESSION → OPEN_REQUEST → APPLY_LANGUAGE → RECOVERY → START_IPC → BUILD_CHROME`
- Update `messages.rs` and its order test.
- Restoring more than one tab does not delay first paint or first input. The step comes after first paint, like the whole chain, and it restores **one entry per message**. Between entries it reposts itself, so pending input is handled before the next file loads.

### Behaviour

1. On the first pass, if the setting is off, the process is not primary, or `read` returns `None`, post the next step at once.
2. Otherwise, keep the parsed `Session` and a cursor in `App` (`pending_session: Option<SessionRestore>`), then delete `session.ini`. A crash during or after restore is then handled by crash recovery alone, and the manifest is never used twice.
3. Each pass restores one entry:
   - **`File(path)`** goes through `window::open_path`, so it reuses the loader, encoding detection and replacement of the clean initial untitled tab. A failure is counted and not reported yet, so it does not produce one notice per file.
   - **`Snapshot(id)`** reads `snapshot_path(recovery_root, id)` with the existing decoder. It opens through a variant of `open_recovered_snapshot` that binds the tab to `snapshot.original_path` when that path exists. The tab has `path = Some(original_path)`, is dirty, and has `recovery_origin` set to the session snapshot. That makes Ctrl+S save to the original file and then remove the snapshot, as the existing code already does after a save. Untitled snapshots open as untitled tabs, as they do today. A missing or invalid snapshot counts as a failure.
   - If a file-backed snapshot's path is already open in a tab, it opens as untitled with the recovery origin. `Tabs::push` rejects duplicate paths, so this keeps the unsaved text anyway.
   - After loading an entry, apply its language right away (`apply_detected_language`). A posted language step could otherwise run after the next entry had become active.
4. After the last entry:
   - Close the empty startup tab if it is still clean, untitled and empty, and at least one entry was restored.
   - Activate the tab at the saved `active` index, or the last restored tab if that entry failed. Apply the saved caret, anchor and first visible line only when the saved entry itself was restored.
   - If anything failed, post one notice, for example "2 items from the last session could not be reopened."
   - Clear `pending_session` and post `OPEN_REQUEST`.
5. `OPEN_REQUEST` then opens any command-line file as it does today, and that file becomes the active tab.

### Titles

`RecoveryOrigin` gets a `from_session: bool`. A session-restored untitled tab is titled "Untitled", not "Recovered: Untitled", because nothing crashed.

### Relation to crash recovery

`recover_snapshots` runs after restore. It must skip any snapshot a restored tab already owns. Add that check where it tests `owns_recovery_id`: skip candidates whose snapshot path equals the `recovery_origin.snapshot_path` of an open document. That snapshot's owner process has exited, so without the check it would be treated as a crash snapshot and opened twice. The check also stops a later Ctrl+O from reopening an already-recovered crash snapshot, because every open re-runs the recovery step.

Every file open posts the language step, which carries on through the recovery step. `recover_snapshots` therefore returns early while a session restore is still in progress. It runs for real after `OPEN_REQUEST`, which always posts the language step again.

### Guards

- Each pass re-checks `identity.is_live_for(hwnd)` after any call that can re-enter, following the existing pattern.
- If the window starts closing while a restore is in progress, the close uses the normal flow. Entries not yet restored lose nothing: their snapshots stay on disk and crash recovery picks them up next launch.
- File population is not re-entrant, so a pass that finds `file_population_active` reposts itself.

### Test seam

The session file path is resolved lazily into `App::session_path`. Under `cfg(test)` it is never resolved, only pre-seeded by a test. That way no in-process test can touch the real `%LOCALAPPDATA%`, even one that sets `instance_mutex`.

## 8. Error handling summary

| Situation | Behaviour |
|---|---|
| Manifest missing, unreadable, or unknown version | No restore. Leftover snapshots come back through crash recovery. |
| File entry missing or fails to load | Skipped and counted in one notice. |
| Snapshot entry missing or invalid | Skipped and counted. The existing discovery code quarantines invalid snapshots. |
| Snapshot write fails at close | Falls back to Save/Discard prompts. |
| Manifest write fails at close | Falls back to Save/Discard prompts. |
| Crash after restore | Restored dirty tabs still own snapshots in `Recovery`, so crash recovery restores them. |
| Setting turned off with snapshots on disk | They come back through crash recovery on the next launch. |

## 9. Performance

- There is no new work before first paint or first input.
- Close is normally one small file write. Snapshots are only written for dirty tabs whose latest edits the timer has not recorded yet.
- Restore loads one entry per message and reposts while input is pending, so the editor stays responsive during a large restore.
- The benchmark harness (`fastpad-bench`) still measures TTI with no session present. A bench run with a 10-tab session checks that TTI does not regress.

## 10. Testing

- **`config/persisted.rs`:** parse `restore_session` with each boolean spelling; the default is `true`.
- **`session.rs`:**
  - encode/parse round-trip;
  - `|` handling in the path field;
  - an out-of-range `active` index;
  - a wrong version, and ignoring malformed lines;
  - atomic write, and read of a missing file.
- **`messages.rs`:** the deferred order includes `RESTORE_SESSION`.
- **In-process window tests** (`main_window.rs`):
  - With restore on, close writes the manifest (clean files as `file=`, dirty tabs as `snapshot=`, clean untitled tabs skipped), shows no prompt, and keeps dirty snapshots.
  - With restore off, close still runs the review prompt.
  - A restored dirty file-backed tab is bound to its path and dirty. Saving it removes its snapshot.
  - Crash recovery does not reopen a snapshot a restored tab owns.
  - A command-line file opens after the restored tabs and becomes active.
  - `ToggleRestoreSession` writes only its own ini line.
- **Integration test** (new `tests/windows/session.rs` with a `[[test]]` entry):
  - Using a scratch `LOCALAPPDATA`: open two files, edit one, add an untitled tab with text, close, and relaunch.
  - Check the tab count, the active tab, and the unsaved text.
  - Relaunch with `restore_session=false` and check the previous behaviour.
