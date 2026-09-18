# Session Restore Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reopen the tabs that were open when FastPad last closed, including unsaved text, the way Notepad++ does. Closing never prompts, and a `restore_session` setting turns the feature off.

**Architecture:**
- A window-agnostic `src/session.rs` owns the `session.ini` manifest format and the restore progress type.
- Closing the primary window with the feature on flushes recovery snapshots for unsaved tabs, writes the manifest, and skips the Save/Discard review.
- A new deferred startup unit, `WM_FASTPAD_RESTORE_SESSION`, sits between `LOAD_SETTINGS` and `OPEN_REQUEST`. It reopens one manifest entry per message, so queued input always runs first.
- Unsaved text never lives in the manifest. It stays in the existing Recovery snapshots, so crash recovery is the fallback for every failure.

**Tech Stack:** Rust 2024, `windows-sys` (existing), Scintilla 5.6.6 (existing). No new dependencies.

**Spec:** `docs/superpowers/specs/2026-09-18-session-restore-design.md`

## Global Constraints

- Nothing not required for the first editable frame may block the first editable frame. No session work runs before first paint.
- Only the primary instance (`App::instance_mutex.is_some()`) saves or restores a session. `--new-window` and independent instances behave exactly as before.
- `restore_session` defaults to `true`. With it `false`, closing and startup behave exactly as before.
- The manifest is `%LOCALAPPDATA%\FastPad\session.ini`, is written atomically with `crate::file::saver::save_atomic`, and has `version=1`.
- No in-process test may touch the real `%LOCALAPPDATA%`. `App::session_path` is only ever pre-seeded under `cfg(test)`.
- Code conventions (from the repo):
  - Tests carry a `// Break caught: ...` comment.
  - Doc comments explain why, not what.
  - App state is reached through short `app_ptr(hwnd)` borrows, re-checking `identity.is_live_for(hwnd)` after anything that can re-enter.
- Verification while working:
  - Compile with `cargo clippy --all-targets --all-features -- -D warnings`.
  - Run only the targeted tests (`cargo test --lib <filter> -- --test-threads=1`).
  - Run the full suite once, in Task 9.
- In a git worktree, copy `native/out` (Scintilla/Lexilla DLLs) from the main checkout before running tests. Window tests need `--test-threads=1`.

## Execution Batches

The 9 tasks run as **3 batches**. Each batch has one test run, one review and one commit. Within a batch, follow each task's steps in order, but **skip the per-task "Run" and "Commit" steps**. The batch checkpoint replaces them.

Checkpoint procedure for every batch:
1. `cargo clippy --all-targets --all-features -- -D warnings` (this is the compile check).
2. Run the batch's test command once.
3. Review the whole batch diff (one reviewer pass for spec compliance and code quality).
4. Commit once with the batch's message.

If step 2 fails, fix the failure and re-run only the failing filter, not the whole command.

### Batch A: Foundations (Tasks 1-5)

Pure or small, independent changes: the setting, the manifest module, the deferred unit (pass-through), the toggle command, and the recovery helpers, titles and claimed-snapshot check. Nothing here changes close or startup behaviour yet.

- **Test command** (`cargo test` takes one name filter per run, so loop over them):
  ```bash
  for f in config:: session:: window::messages window::commands window::command_palette window::menus bootstrap:: recovery:: document:: session_toggle recovery_never_reopens; do cargo test --lib "$f" -- --test-threads=1 -q || break; done
  ```
- **Commit:** `feat(session): setting, manifest, startup unit, toggle and recovery helpers`

### Batch B: Behaviour (Tasks 6-7)

Save on close and restore at startup. They share helpers (`session_path`, `enable_session`, `write_session`) and only make sense reviewed together.

- **Test command:**
  ```bash
  for f in session_ recover window::main_window::tests::clos; do cargo test --lib "$f" -- --test-threads=1 -q || break; done
  ```
- **Commit:** `feat(window): save the session on close and reopen it after first paint`

### Batch C: End to end and release checks (Tasks 8-9)

The integration test, the primary-instance test audit, the README, and the final full verification. This batch's test run is Task 9's full suite: `cargo fmt --check`, clippy, then `cargo test -- --test-threads=1` once. That run already covers Task 8's `--test session` and the audited binaries, so do not run them separately beforehand. The exception is when fixing an audit failure, which is re-run with its own `--test <name>`.

- **Commit:** `test: session relaunch end to end; docs: session restore`

## File Structure

| File | Change | Responsibility |
|---|---|---|
| `src/config/persisted.rs`, `src/config/defaults.rs` | Modify | `restore_session` setting |
| `src/session.rs` | Create | Manifest types, encode/parse, read/write/remove, `SessionRestore` progress, notice texts |
| `src/lib.rs` | Modify | `pub mod session;` |
| `src/window/messages.rs`, `src/window/mod.rs` | Modify | `WM_FASTPAD_RESTORE_SESSION` in the deferred chain |
| `src/window/commands.rs`, `src/window/command_palette.rs`, `src/window/menus.rs` | Modify | `ToggleRestoreSession` command |
| `src/document.rs` | Modify | `RecoveryOrigin::from_session` and its title |
| `src/recovery/mod.rs`, `src/recovery/snapshot.rs` | Modify | `current_snapshot_file`, `snapshots_removed_on_session_close`, `snapshot_file_id` |
| `src/editor/scintilla_constants.rs` | Modify | `SCI_GETANCHOR` |
| `src/app.rs` | Modify | `session_path`, `session_restore` fields |
| `src/window/main_window.rs` | Modify | Close flow, restore step, snapshot-tab refactor, recovery claim check, tests |
| `src/bootstrap.rs` | Modify | Deferred-order test expectation |
| `tests/windows/session.rs`, `Cargo.toml` | Create/Modify | End-to-end relaunch test |
| `README.md` | Modify | Document the feature and the key |

---

### Task 1: `restore_session` setting

**Files:**
- Modify: `src/config/persisted.rs` (`Settings` ~27-36, `apply_delta` ~42-64, `SettingsDelta` ~80-90, `parse` doc ~92-98, `apply_line` ~121-159, tests after ~440)
- Modify: `src/config/defaults.rs:7-36`

**Interfaces:**
- Produces: `Settings::restore_session: bool`, `SettingsDelta::restore_session: Option<bool>`, `config::defaults::DEFAULT_RESTORE_SESSION: bool = true`.

- [ ] **Step 1: Write the failing test.** Add it in `src/config/persisted.rs` `mod tests`, right after `line_numbers_accepts_the_same_boolean_spellings_as_word_wrap`:

```rust
    #[test]
    fn restore_session_defaults_on_and_accepts_the_boolean_spellings() {
        // Break caught: session restore that is off for a brand-new profile, or that cannot be
        // switched off from fastpad.ini.
        assert!(default_settings().restore_session);
        assert_eq!(parse("restore_session=off").restore_session, Some(false));
        assert_eq!(parse("restore_session=Yes").restore_session, Some(true));
        let delta = parse("restore_session=later");
        assert_eq!(delta.restore_session, None);
        assert_eq!(delta.warnings.len(), 1);

        let mut settings = default_settings();
        settings.apply_delta(&parse("restore_session=0"));
        assert!(!settings.restore_session);
    }
```

- [ ] **Step 2: Run it and confirm it fails to compile.**
  - Run: `cargo test --lib config::persisted::tests::restore_session -- --test-threads=1`
  - Expected: FAIL with ``no field `restore_session` ``.

- [ ] **Step 3: Implement.**
  - In `Settings`, add after `recovery_interval_seconds`:
    ```rust
    /// Whether the primary window reopens the last session's tabs and closes without prompting.
    pub restore_session: bool,
    ```
  - In `SettingsDelta`, add before `warnings`:
    ```rust
    pub restore_session: Option<bool>,
    ```
  - In `apply_delta`, add:
    ```rust
    if let Some(restore_session) = delta.restore_session {
        self.restore_session = restore_session;
    }
    ```
  - In `apply_line`, add an arm before `_ =>`:
    ```rust
        "restore_session" => match parse_bool(value) {
            Some(restore_session) => delta.restore_session = Some(restore_session),
            None => warn(delta, line_number, key, value),
        },
    ```
  - In the `parse` doc comment, change the recognised-key list to: ``exactly `font_face`, `font_size`, `tab_width`, `word_wrap`, `line_numbers`, `theme`, `recovery_interval_seconds`, and `restore_session` are recognized``.
  - In `src/config/defaults.rs`:
    - Add `pub const DEFAULT_RESTORE_SESSION: bool = true;` after `DEFAULT_RECOVERY_INTERVAL_SECONDS`.
    - Add `restore_session: DEFAULT_RESTORE_SESSION,` to `default_settings()`.
    - Change the doc comment to end with `..., a 30-second crash-recovery interval, and session restore on.`
    - Add `assert!(settings.restore_session);` to `default_settings_match_the_compiled_defaults`.

- [ ] **Step 4: Run the tests and confirm they pass.**
  - Run: `cargo test --lib config:: -- --test-threads=1`
  - Expected: PASS.
  - The struct-update literal at `src/window/main_window.rs:~4351` uses `..reloaded`, so it still compiles.

- [ ] **Step 5: Commit.**

```bash
git add src/config/persisted.rs src/config/defaults.rs
git commit -m "feat(config): add restore_session setting"
```

---

### Task 2: Session manifest module

**Files:**
- Create: `src/session.rs`
- Modify: `src/lib.rs` (add `pub mod session;` after `pub mod recovery;`)

**Interfaces:**
- Produces (all `pub` in `crate::session`):
  - `enum SessionSource { File(PathBuf), Snapshot(RecoveryId) }` (Clone, Debug, Eq, PartialEq)
  - `struct SessionEntry { source: SessionSource, caret: usize, anchor: usize, first_line: usize }` with `SessionEntry::new(source) -> Self` (zeros)
  - `struct Session { active: usize, entries: Vec<SessionEntry> }` (Default) with `encode(&self) -> String` and `parse(&str) -> Option<Session>`
  - `fn session_file_path() -> crate::Result<PathBuf>`
  - `fn write(path: &Path, session: &Session) -> crate::Result<()>`
  - `fn read(path: &Path) -> Option<Session>`
  - `fn remove(path: &Path)`
  - `struct SessionRestore { session: Session, next: usize, failed: usize, restored: Vec<Option<DocumentId>>, placeholder: Option<DocumentId> }`, with:
    - `new(session, placeholder) -> Self`
    - `next_entry(&self) -> Option<&SessionEntry>`
    - `record(&mut self, restored: Option<DocumentId>)`
    - `active_tab(&self) -> Option<DocumentId>`
    - `saved_active_restored(&self) -> Option<DocumentId>`
  - `fn restore_failure_notice(count: usize) -> String`
  - `fn toggle_notice(enabled: bool) -> &'static str`

- [ ] **Step 1: Write the module with its tests** (tests first in the same file, then confirm the implementation below makes them pass). Create `src/session.rs`:

```rust
//! The session manifest: which tabs the primary window had open when it closed, so the next
//! launch can reopen them. Unsaved text is never stored here. It stays in the Recovery snapshots
//! the manifest names, so a lost or corrupt manifest still leaves crash recovery to bring it back.
//!
//! Format (UTF-8, one `key=value` per line): `version=1`, `active=<zero-based entry index>`, then
//! one `file=<caret>|<anchor>|<first visible line>|<path>` or
//! `snapshot=<caret>|<anchor>|<first visible line>|<recovery id as 32 hex digits>` per tab, in tab
//! order. The target is always the last field, so a path containing `|` still parses.

use crate::Result;
use crate::document::{DocumentId, RecoveryId};
use std::path::{Path, PathBuf};

const VERSION: &str = "1";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionSource {
    /// A clean file, reopened from disk.
    File(PathBuf),
    /// An unsaved tab, reopened from the Recovery snapshot with this ID.
    Snapshot(RecoveryId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionEntry {
    pub source: SessionSource,
    pub caret: usize,
    pub anchor: usize,
    pub first_line: usize,
}

impl SessionEntry {
    pub fn new(source: SessionSource) -> Self {
        Self {
            source,
            caret: 0,
            anchor: 0,
            first_line: 0,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Session {
    pub active: usize,
    pub entries: Vec<SessionEntry>,
}

impl Session {
    pub fn encode(&self) -> String {
        let mut output = format!("version={VERSION}\r\nactive={}\r\n", self.active);
        for entry in &self.entries {
            let (key, target) = match &entry.source {
                SessionSource::File(path) => ("file", path.to_string_lossy().into_owned()),
                SessionSource::Snapshot(id) => ("snapshot", format!("{:032x}", id.0)),
            };
            output.push_str(&format!(
                "{key}={}|{}|{}|{target}\r\n",
                entry.caret, entry.anchor, entry.first_line
            ));
        }
        output
    }

    /// `None` for anything but a version-1 manifest. Malformed entry lines and unknown keys are
    /// skipped, and an out-of-range active index falls back to the first entry.
    pub fn parse(source: &str) -> Option<Self> {
        let source = source.strip_prefix('\u{feff}').unwrap_or(source);
        let mut version = None;
        let mut active = 0;
        let mut entries = Vec::new();
        for line in source.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key {
                "version" => version = Some(value),
                "active" => active = value.parse().unwrap_or(0),
                "file" | "snapshot" => entries.extend(parse_entry(key, value)),
                _ => {}
            }
        }
        if version != Some(VERSION) {
            return None;
        }
        if active >= entries.len() {
            active = 0;
        }
        Some(Self { active, entries })
    }
}

fn parse_entry(key: &str, value: &str) -> Option<SessionEntry> {
    let mut fields = value.splitn(4, '|');
    let caret = fields.next()?.parse().ok()?;
    let anchor = fields.next()?.parse().ok()?;
    let first_line = fields.next()?.parse().ok()?;
    let target = fields.next()?;
    let source = if key == "file" {
        if target.is_empty() {
            return None;
        }
        SessionSource::File(PathBuf::from(target))
    } else {
        if target.len() != 32 || !target.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        SessionSource::Snapshot(RecoveryId::from_u128(
            u128::from_str_radix(target, 16).ok()?,
        ))
    };
    Some(SessionEntry {
        source,
        caret,
        anchor,
        first_line,
    })
}

/// `%LocalAppData%\FastPad\session.ini`, next to `fastpad.ini`.
pub fn session_file_path() -> Result<PathBuf> {
    Ok(crate::platform::paths::fastpad_data_dir()?.join("session.ini"))
}

pub fn write(path: &Path, session: &Session) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::file::saver::save_atomic(path, session.encode().as_bytes())
}

pub fn read(path: &Path) -> Option<Session> {
    Session::parse(&std::fs::read_to_string(path).ok()?)
}

pub fn remove(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// Progress through a manifest being reopened, one entry per `WM_FASTPAD_RESTORE_SESSION`.
#[derive(Debug)]
pub struct SessionRestore {
    pub session: Session,
    pub next: usize,
    pub failed: usize,
    /// The tab each processed entry became, by entry index. `None` when the entry failed.
    pub restored: Vec<Option<DocumentId>>,
    /// The empty startup tab, closed at the end once something else was restored.
    pub placeholder: Option<DocumentId>,
}

impl SessionRestore {
    pub fn new(session: Session, placeholder: Option<DocumentId>) -> Self {
        Self {
            session,
            next: 0,
            failed: 0,
            restored: Vec::new(),
            placeholder,
        }
    }

    pub fn next_entry(&self) -> Option<&SessionEntry> {
        self.session.entries.get(self.next)
    }

    pub fn record(&mut self, restored: Option<DocumentId>) {
        if restored.is_none() {
            self.failed += 1;
        }
        self.restored.push(restored);
        self.next += 1;
    }

    /// The tab the saved active entry became, if that entry was restored.
    pub fn saved_active_restored(&self) -> Option<DocumentId> {
        self.restored.get(self.session.active).copied().flatten()
    }

    /// The tab to show at the end: the saved active one, else the last one restored.
    pub fn active_tab(&self) -> Option<DocumentId> {
        self.saved_active_restored()
            .or_else(|| self.restored.iter().rev().find_map(|id| *id))
    }
}

pub fn restore_failure_notice(count: usize) -> String {
    if count == 1 {
        "1 item from the last session could not be reopened.".to_owned()
    } else {
        format!("{count} items from the last session could not be reopened.")
    }
}

pub fn toggle_notice(enabled: bool) -> &'static str {
    if enabled {
        "Session restore is on. Open tabs will reopen on the next launch."
    } else {
        "Session restore is off. Closing FastPad will ask about unsaved changes."
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Session {
        Session {
            active: 1,
            entries: vec![
                SessionEntry::new(SessionSource::File(PathBuf::from(r"C:\notes\a b|c.txt"))),
                SessionEntry {
                    source: SessionSource::Snapshot(RecoveryId::from_u128(0xabc)),
                    caret: 12,
                    anchor: 4,
                    first_line: 3,
                },
            ],
        }
    }

    #[test]
    fn a_manifest_round_trips_through_its_text_form() {
        // Break caught: a path with spaces or `|`, or a snapshot ID, that does not survive a
        // write and read, so the next launch reopens the wrong file or none.
        let session = sample();
        let text = session.encode();
        assert!(text.contains("snapshot=12|4|3|00000000000000000000000000000abc"));
        assert_eq!(Session::parse(&text), Some(session));
    }

    #[test]
    fn only_version_one_manifests_are_accepted() {
        // Break caught: a future or hand-damaged manifest restoring garbage instead of nothing.
        assert_eq!(Session::parse("active=0\r\nfile=0|0|0|C:\\a.txt\r\n"), None);
        assert_eq!(Session::parse("version=2\r\nfile=0|0|0|C:\\a.txt\r\n"), None);
        assert!(Session::parse("\u{feff}version=1\r\n").is_some());
    }

    #[test]
    fn malformed_entries_are_skipped_and_a_bad_active_index_falls_back() {
        // Break caught: one damaged line discarding the whole session, or an active index that
        // points past the entries.
        let parsed = Session::parse(
            "version=1\nactive=9\nfile=x|0|0|C:\\bad.txt\nsnapshot=0|0|0|xyz\nfile=0|0|0|\n\
             bogus=1\nfile=1|2|3|C:\\good.txt\n",
        )
        .unwrap();
        assert_eq!(parsed.active, 0);
        assert_eq!(
            parsed.entries,
            vec![SessionEntry {
                source: SessionSource::File(PathBuf::from(r"C:\good.txt")),
                caret: 1,
                anchor: 2,
                first_line: 3,
            }]
        );
    }

    #[test]
    fn write_then_read_returns_the_session_and_a_missing_file_reads_as_none() {
        let dir = std::env::temp_dir().join(format!("fastpad-session-io-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("FastPad").join("session.ini");
        assert_eq!(read(&path), None);
        write(&path, &sample()).unwrap();
        assert_eq!(read(&path), Some(sample()));
        remove(&path);
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_progress_activates_the_saved_tab_or_the_last_restored_one() {
        // Break caught: a failed active entry leaving no tab activated, or counting successes as
        // failures in the notice.
        let mut restore = SessionRestore::new(sample(), None);
        assert_eq!(restore.next_entry(), Some(&sample().entries[0]));
        restore.record(Some(DocumentId(7)));
        restore.record(None);
        assert_eq!(restore.next_entry(), None);
        assert_eq!(restore.failed, 1);
        assert_eq!(restore.saved_active_restored(), None);
        assert_eq!(restore.active_tab(), Some(DocumentId(7)));

        let mut restore = SessionRestore::new(sample(), None);
        restore.record(Some(DocumentId(7)));
        restore.record(Some(DocumentId(8)));
        assert_eq!(restore.active_tab(), Some(DocumentId(8)));
        assert_eq!(restore_failure_notice(1), "1 item from the last session could not be reopened.");
        assert_eq!(restore_failure_notice(2), "2 items from the last session could not be reopened.");
    }
}
```

- [ ] **Step 2: Register the module.** Add `pub mod session;` to `src/lib.rs` after `pub mod recovery;`.

- [ ] **Step 3: Run the tests.**
  - Run: `cargo test --lib session::tests -- --test-threads=1`
  - Expected: all 5 PASS.
  - If `DocumentId` is not `Debug + Eq + Copy`, check `src/document.rs`. It is used that way in `recovery/mod.rs` tests, so it should be.

- [ ] **Step 4: Commit.**

```bash
git add src/session.rs src/lib.rs
git commit -m "feat(session): add session manifest module"
```

---

### Task 3: `WM_FASTPAD_RESTORE_SESSION` deferred unit

This task only adds the unit to the chain, as a pass-through that posts `OPEN_REQUEST`. Task 7 gives it behaviour.

**Files:**
- Modify: `src/window/messages.rs` (constants, `classify_deferred_message`, `completed_milestone`, tests)
- Modify: `src/window/mod.rs:30-36` (re-export)
- Modify: `src/window/main_window.rs:672-678` (`handle_deferred`)
- Modify: `src/bootstrap.rs:~716-727` (deferred-order test expectation)

**Interfaces:**
- Produces: `crate::window::WM_FASTPAD_RESTORE_SESSION: u32 = WM_APP + 8`.
- Chain: `LOAD_SETTINGS → RESTORE_SESSION → OPEN_REQUEST → ...`
- `load_settings` now runs on `PostNext(WM_FASTPAD_RESTORE_SESSION)`, and `Milestone::SettingsLoaded` is recorded there.

- [ ] **Step 1: Update the failing tests first** in `src/window/messages.rs` `mod tests`.
  - Add `WM_FASTPAD_RESTORE_SESSION` to the `use super::{...}` list.
  - In `deferred_messages_follow_the_required_startup_order`, replace the first assertion with:
    ```rust
        assert_eq!(
            classify_deferred_message(WM_FASTPAD_LOAD_SETTINGS, false),
            Some(DeferredAction::PostNext(WM_FASTPAD_RESTORE_SESSION))
        );
        assert_eq!(
            classify_deferred_message(WM_FASTPAD_RESTORE_SESSION, false),
            Some(DeferredAction::PostNext(WM_FASTPAD_OPEN_REQUEST))
        );
    ```
  - In `deferred_message_reposts_itself_when_input_is_pending`, add:
    ```rust
        assert_eq!(
            classify_deferred_message(WM_FASTPAD_RESTORE_SESSION, true),
            Some(DeferredAction::RepostSelf(WM_FASTPAD_RESTORE_SESSION))
        );
    ```
  - In `deferred_transitions_report_settings_and_file_completion`, replace the `SettingsLoaded` assertion with:
    ```rust
        assert_eq!(
            completed_milestone(DeferredAction::PostNext(WM_FASTPAD_RESTORE_SESSION)),
            Some(Milestone::SettingsLoaded)
        );
        assert_eq!(
            completed_milestone(DeferredAction::PostNext(WM_FASTPAD_OPEN_REQUEST)),
            None
        );
    ```

- [ ] **Step 2: Run the tests and confirm they fail.**
  - Run: `cargo test --lib window::messages -- --test-threads=1`
  - Expected: FAIL to compile (``cannot find value `WM_FASTPAD_RESTORE_SESSION` ``).

- [ ] **Step 3: Implement.**
  - In `src/window/messages.rs`, after `WM_FASTPAD_IPC_REQUEST`, add:
    ```rust
    pub const WM_FASTPAD_RESTORE_SESSION: u32 = WM_APP + 8;
    ```
  - In `classify_deferred_message`, replace the `WM_FASTPAD_LOAD_SETTINGS` arm with:
    ```rust
        WM_FASTPAD_LOAD_SETTINGS => {
            next_action(message, WM_FASTPAD_RESTORE_SESSION, input_pending)
        }
        WM_FASTPAD_RESTORE_SESSION => next_action(message, WM_FASTPAD_OPEN_REQUEST, input_pending),
    ```
  - In `completed_milestone`, change `DeferredAction::PostNext(WM_FASTPAD_OPEN_REQUEST) => Some(Milestone::SettingsLoaded),` to `DeferredAction::PostNext(WM_FASTPAD_RESTORE_SESSION) => Some(Milestone::SettingsLoaded),`.
  - In `src/window/mod.rs`, add `WM_FASTPAD_RESTORE_SESSION` to the `pub use messages::{...}` list, keeping it alphabetical.
  - In `src/window/main_window.rs` `handle_deferred`, replace:
    ```rust
        // Each of these actions is produced only by its own deferred message with no input pending:
        // `PostNext(WM_FASTPAD_OPEN_REQUEST)` by `WM_FASTPAD_LOAD_SETTINGS`, `RecordFullyReady` by
        // `WM_FASTPAD_BUILD_CHROME`. Running them before the milestone keeps the milestone honest.
        if action == DeferredAction::PostNext(crate::window::WM_FASTPAD_OPEN_REQUEST) {
            load_settings(hwnd);
        }
    ```
    with:
    ```rust
        // Each of these actions is produced only by its own deferred message with no input pending:
        // `PostNext(WM_FASTPAD_RESTORE_SESSION)` by `WM_FASTPAD_LOAD_SETTINGS`, `RecordFullyReady` by
        // `WM_FASTPAD_BUILD_CHROME`. Running them before the milestone keeps the milestone honest.
        if action == DeferredAction::PostNext(crate::window::WM_FASTPAD_RESTORE_SESSION) {
            load_settings(hwnd);
        }
    ```
  - In `src/bootstrap.rs`, in the test that ends with `assert_eq!(queued.message, crate::window::WM_FASTPAD_OPEN_REQUEST);` (~line 716-727), replace every `crate::window::WM_FASTPAD_OPEN_REQUEST` in the `open_request_status` `PeekMessageW` call and in that final assertion with `crate::window::WM_FASTPAD_RESTORE_SESSION`. Also rename the local `open_request_status` to `restore_session_status`.

- [ ] **Step 4: Run the tests and confirm they pass.**
  - Run: `cargo test --lib window::messages -- --test-threads=1`, then `cargo test --lib bootstrap:: -- --test-threads=1`.
  - Expected: PASS.
  - Some bootstrap tests post `WM_FASTPAD_LOAD_SETTINGS` and pump until `WM_FASTPAD_OPEN_REQUEST`. They still pass because the pump dispatches `RESTORE_SESSION`, which posts `OPEN_REQUEST`. If any test asserts the message that immediately follows `LOAD_SETTINGS`, update it to `RESTORE_SESSION` the same way.

- [ ] **Step 5: Commit.**

```bash
git add src/window/messages.rs src/window/mod.rs src/window/main_window.rs src/bootstrap.rs
git commit -m "feat(window): add session restore unit to the deferred startup chain"
```

---

### Task 4: `ToggleRestoreSession` command

**Files:**
- Modify: `src/window/commands.rs` (enum, `needs_document`, `TryFrom` table size 56→57, test)
- Modify: `src/window/command_palette.rs:47-52` (`ENTRIES` 45→46)
- Modify: `src/window/menus.rs:~141-148` (File menu)
- Modify: `src/window/main_window.rs:~1594-1601` (`execute_command`), tests

**Interfaces:**
- Consumes: `Settings::restore_session` (Task 1), `crate::session::toggle_notice` (Task 2).
- Produces: `CommandId::ToggleRestoreSession` (value 156).

- [ ] **Step 1: Write the failing tests.**
  - In `src/window/commands.rs` `native_command_values_are_stable_and_round_trip`, add:
    ```rust
            assert_eq!(CommandId::try_from(156), Ok(CommandId::ToggleRestoreSession));
            assert!(!CommandId::ToggleRestoreSession.needs_document());
    ```
  - In `src/window/main_window.rs` `mod tests`, next to `setting_commands_apply_to_the_editor_and_save_only_their_own_ini_lines`, add:
    ```rust
        #[test]
        fn session_toggle_saves_only_its_line_and_says_so() {
            // Break caught: a toggle that flips the flag but is lost on restart, rewrites the user's
            // fastpad.ini, or leaves no sign of which state it chose (menus show no checkmarks).
            let _scintilla = load_native_scintilla();
            let scratch = RecoveryScratch::new("session-toggle");
            let ini = scratch.path().join("fastpad.ini");
            std::fs::write(&ini, "# kept\r\n").unwrap();
            super::save_settings_to(Some(ini.clone()));
            let window = ProductionWindow::new(make_app());
            let _editor = install_test_editor(&window);
            assert!(app_mut(window.hwnd).settings.restore_session);

            execute_command(window.hwnd, CommandId::ToggleRestoreSession);

            assert!(!app_mut(window.hwnd).settings.restore_session);
            assert_eq!(
                std::fs::read_to_string(&ini).unwrap(),
                "# kept\r\nrestore_session=false\r\n"
            );
            assert!(
                app_mut(window.hwnd)
                    .notifications
                    .pending()
                    .iter()
                    .any(|notice| notice.message == crate::session::toggle_notice(false))
            );
            super::save_settings_to(None);
        }
    ```

- [ ] **Step 2: Run the tests and confirm they fail.**
  - Run: `cargo test --lib window::commands -- --test-threads=1`
  - Expected: FAIL to compile (no variant `ToggleRestoreSession`).

- [ ] **Step 3: Implement.**
  - `src/window/commands.rs`:
    - Append `ToggleRestoreSession,` after `MarkdownPreviewClose,` in the enum.
    - Add `| Self::ToggleRestoreSession` to the `needs_document` exemption list after `Self::ThemeCatppuccinMocha`.
    - Change `const COMMANDS: [CommandId; 56]` to `[CommandId; 57]` and append `CommandId::ToggleRestoreSession,`.
  - `src/window/command_palette.rs`:
    - Change `ENTRIES: [PaletteEntry; 45]` to `[PaletteEntry; 46]`.
    - Insert after `entry("File: Close all tabs", CommandId::CloseAllTabs),`:
      ```rust
          entry("File: Toggle session restore", CommandId::ToggleRestoreSession),
      ```
  - `src/window/menus.rs`, File popup: replace
    ```rust
                    MenuEntry::command("&Close tab", CommandId::CloseTab),
                    MenuEntry::Separator,
                    MenuEntry::command("E&xit", CommandId::Exit),
    ```
    with
    ```rust
                    MenuEntry::command("&Close tab", CommandId::CloseTab),
                    MenuEntry::Separator,
                    MenuEntry::command(
                        "&Restore session on startup",
                        CommandId::ToggleRestoreSession,
                    ),
                    MenuEntry::Separator,
                    MenuEntry::command("E&xit", CommandId::Exit),
    ```
  - `src/window/main_window.rs` `execute_command`: after the `CommandId::ToggleLineNumbers` arm, add:
    ```rust
            CommandId::ToggleRestoreSession => {
                change_setting(hwnd, |settings| {
                    settings.restore_session = !settings.restore_session;
                    Some(("restore_session", settings.restore_session.to_string()))
                });
                let enabled = unsafe { app_ptr(hwnd) }
                    .is_some_and(|app| unsafe { app.as_ref() }.settings.restore_session);
                push_notice(hwnd, crate::session::toggle_notice(enabled).to_owned());
            }
    ```
  - If the compiler reports another exhaustive `match` on `CommandId` (for example accessibility names or accelerator specs), add the arm there with the same menu label.

- [ ] **Step 4: Run the tests and confirm they pass.**
  - Run: `cargo test --lib window::commands -- --test-threads=1`
  - Run: `cargo test --lib window::command_palette -- --test-threads=1`
  - Run: `cargo test --lib window::menus -- --test-threads=1`
  - Run: `cargo test --lib session_toggle -- --test-threads=1`
  - Expected: PASS.
  - `every_command_except_tab_positions_and_the_palette_is_listed_once` covers the palette entry.

- [ ] **Step 5: Commit.**

```bash
git add src/window/commands.rs src/window/command_palette.rs src/window/menus.rs src/window/main_window.rs
git commit -m "feat(window): add command to toggle session restore"
```

---

### Task 5: Recovery helpers, session titles, and the claimed-snapshot check

**Files:**
- Modify: `src/document.rs:29-38` (`RecoveryOrigin`) and `title()` (~108-118), tests
- Modify: every `RecoveryOrigin {` literal (7 sites; find with `rg -n "RecoveryOrigin \{" src tests`)
- Modify: `src/recovery/snapshot.rs` (add `snapshot_file_id`, test)
- Modify: `src/recovery/mod.rs` (add `current_snapshot_file`, `snapshots_removed_on_session_close`, tests)
- Modify: `src/window/main_window.rs:~2979-2990` (`recover_snapshots` claim check), test

**Interfaces:**
- Produces:
  - `RecoveryOrigin::from_session: bool`
  - `crate::recovery::snapshot::snapshot_file_id(path: &Path) -> Option<RecoveryId>`
  - `crate::recovery::current_snapshot_file(root: &Path, document: &Document) -> Option<PathBuf>`
  - `crate::recovery::snapshots_removed_on_session_close(root: &Path, document: &Document) -> Vec<PathBuf>`

- [ ] **Step 1: Write the failing tests.**
  - In `src/document.rs` tests (create `#[cfg(test)] mod tests { use super::*; ... }` if none exists):
    ```rust
        #[test]
        fn session_restored_untitled_tabs_are_not_called_recovered() {
            // Break caught: every unsaved tab from a normal close reappearing as "Recovered: ...",
            // as if FastPad had crashed.
            let mut document = Document::test_fixture(DocumentId(1), true);
            document.recovery_origin = Some(RecoveryOrigin {
                snapshot_path: PathBuf::from(r"C:\Recovery\a.fps"),
                original_path: None,
                from_session: true,
            });
            assert_eq!(document.title(), "Untitled *");
            document.recovery_origin.as_mut().unwrap().from_session = false;
            assert_eq!(document.title(), "Recovered: Untitled *");
        }
    ```
  - In `src/recovery/snapshot.rs` tests:
    ```rust
        #[test]
        fn snapshot_files_name_their_recovery_id() {
            let root = Path::new(r"C:\Recovery");
            let id = RecoveryId::from_u128(0x1234);
            assert_eq!(snapshot_file_id(&snapshot_path(root, id)), Some(id));
            assert_eq!(snapshot_file_id(Path::new(r"C:\Recovery\1234.fps")), None);
            assert_eq!(snapshot_file_id(&root.join(format!("{:032x}.txt", 0x1234))), None);
        }
    ```
  - In `src/recovery/mod.rs` tests, add `current_snapshot_file` and `snapshots_removed_on_session_close` to the `use super::{...}` list, then add:
    ```rust
        fn scratch(label: &str) -> PathBuf {
            let root = std::env::temp_dir()
                .join(format!("fastpad-recovery-{label}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            root
        }

        #[test]
        fn the_current_snapshot_is_the_own_one_once_written_else_the_source() {
            // Break caught: a session naming a stale or missing snapshot, so the next launch restores
            // old text or nothing at all.
            let root = scratch("current");
            let source = root.join("source.fps");
            std::fs::write(&source, b"x").unwrap();
            let mut document = Document::test_fixture(DocumentId(1), true);
            document.recovery_origin = Some(RecoveryOrigin {
                snapshot_path: source.clone(),
                original_path: None,
                from_session: false,
            });
            assert_eq!(current_snapshot_file(&root, &document), None, "generation unrecorded");
            document.recovery_generation = Some(document.generation);
            assert_eq!(current_snapshot_file(&root, &document), Some(source.clone()));
            let own = crate::recovery::snapshot::snapshot_path(&root, document.recovery_id);
            std::fs::write(&own, b"y").unwrap();
            assert_eq!(current_snapshot_file(&root, &document), Some(own.clone()));

            assert_eq!(snapshots_removed_on_session_close(&root, &document), vec![source]);
            let mut clean = Document::test_fixture(DocumentId(2), false);
            clean.recovery_origin = document.recovery_origin.clone();
            assert_eq!(snapshots_removed_on_session_close(&root, &clean).len(), 2);
            document.recovery_generation = None;
            assert!(snapshots_removed_on_session_close(&root, &document).is_empty());
            let _ = std::fs::remove_dir_all(&root);
        }
    ```
  - In `src/window/main_window.rs` `mod tests`, add:
    ```rust
        #[test]
        fn recovery_never_reopens_a_snapshot_an_open_tab_already_holds() {
            // Break caught: a later Open (every open re-runs the recovery unit) or a session restore
            // opening a second copy of text that is already in a tab.
            let _scintilla = load_native_scintilla();
            let window = ProductionWindow::new(make_app());
            let _editor = install_test_editor(&window);
            let root = RecoveryScratch::new("claimed");
            write_snapshot(
                root.path(),
                &Snapshot::new(RecoveryId::from_u128(0x7171), None, Encoding::Utf8, "held once"),
            )
            .unwrap();
            app_mut(window.hwnd).recovery_root = Some(root.path().to_path_buf());

            unsafe { SendMessageW(window.hwnd, crate::window::WM_FASTPAD_RECOVERY, 0, 0) };
            assert_eq!(app_mut(window.hwnd).tabs.len(), 2);
            unsafe { SendMessageW(window.hwnd, crate::window::WM_FASTPAD_RECOVERY, 0, 0) };
            assert_eq!(app_mut(window.hwnd).tabs.len(), 2);
        }
    ```

- [ ] **Step 2: Run the tests and confirm they fail.**
  - Run: `cargo test --lib recovery:: -- --test-threads=1`
  - Expected: FAIL to compile (no field `from_session`, no `current_snapshot_file`).

- [ ] **Step 3: Implement.**
  - `src/document.rs`:
    - Add a field to `RecoveryOrigin`:
      ```rust
          /// Restored from the last session rather than after a crash; titled like any other tab.
          pub from_session: bool,
      ```
    - In `title()`, change the match arm `(None, Some(origin)) => format!("Recovered: {}", origin.display_name()),` to:
      ```rust
                  (None, Some(origin)) if !origin.from_session => {
                      format!("Recovered: {}", origin.display_name())
                  }
      ```
  - Add `from_session: false,` to every other `RecoveryOrigin {` literal, including the one in `open_recovered_snapshot` in `main_window.rs`. Use `rg -n "RecoveryOrigin \{" src tests` to find them.
  - `src/recovery/snapshot.rs`, after `snapshot_path`:
    ```rust
    /// The recovery ID a snapshot file is named after, or `None` for any other file name.
    pub fn snapshot_file_id(path: &Path) -> Option<RecoveryId> {
        if path.extension()? != EXTENSION {
            return None;
        }
        let stem = path.file_stem()?.to_str()?;
        if stem.len() != 32 || !stem.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        u128::from_str_radix(stem, 16).ok().map(RecoveryId::from_u128)
    }
    ```
  - `src/recovery/mod.rs`, after `snapshots_removed_on_close`:
    ```rust
    /// The snapshot file holding `document`'s latest text: its own once written, otherwise the
    /// snapshot it was recovered or restored from. `None` while the latest edits are in no file.
    pub fn current_snapshot_file(root: &Path, document: &Document) -> Option<PathBuf> {
        if document.recovery_generation != Some(document.generation) {
            return None;
        }
        let own = snapshot::snapshot_path(root, document.recovery_id);
        if own.is_file() {
            return Some(own);
        }
        document
            .recovery_origin
            .as_ref()
            .map(|origin| origin.snapshot_path.clone())
            .filter(|source| source.is_file())
    }

    /// Files a session-saving close may delete. The manifest names only the current snapshot of
    /// each dirty document, so everything else that document owns would resurrect as a duplicate.
    /// Clean documents need none of theirs. A dirty document whose text is in no file keeps all.
    pub fn snapshots_removed_on_session_close(root: &Path, document: &Document) -> Vec<PathBuf> {
        let keep = if document.dirty {
            match current_snapshot_file(root, document) {
                Some(current) => Some(current),
                None => return Vec::new(),
            }
        } else {
            None
        };
        owned_snapshot_files(root, document)
            .into_iter()
            .filter(|file| Some(file) != keep.as_ref())
            .collect()
    }
    ```
  - `src/window/main_window.rs` `recover_snapshots`: replace
    ```rust
            let owned = unsafe { app_ptr(hwnd) }.is_none_or(|app| {
                unsafe { app.as_ref() }.owns_recovery_id(candidate.snapshot.recovery_id)
            });
            if owned {
                continue;
            }
    ```
    with
    ```rust
            // This process's own snapshots, and ones an open tab was already recovered or restored
            // from, are held by a live tab rather than left behind by a crash.
            let claimed = unsafe { app_ptr(hwnd) }.is_none_or(|app| {
                let app = unsafe { app.as_ref() };
                app.owns_recovery_id(candidate.snapshot.recovery_id)
                    || app.tabs.documents().any(|document| {
                        document
                            .recovery_origin
                            .as_ref()
                            .is_some_and(|origin| origin.snapshot_path == candidate.path)
                    })
            });
            if claimed {
                continue;
            }
    ```

- [ ] **Step 4: Run the tests and confirm they pass.**
  - Run: `cargo test --lib recovery:: -- --test-threads=1`
  - Run: `cargo test --lib document:: -- --test-threads=1`
  - Run: `cargo test --lib recovery_never_reopens -- --test-threads=1`
  - Expected: PASS.

- [ ] **Step 5: Commit.**

```bash
git add src/document.rs src/recovery src/window/main_window.rs tests
git commit -m "feat(recovery): session-aware snapshot helpers and claimed-snapshot check"
```

---

### Task 6: Save the session on close

**Files:**
- Modify: `src/app.rs` (field `session_path`, init in `App::new`)
- Modify: `src/editor/scintilla_constants.rs` (add `SCI_GETANCHOR`)
- Modify: `src/window/main_window.rs` (`WM_CLOSE` ~205-220; new fns near `remove_session_snapshots` ~3247; tests)

**Interfaces:**
- Consumes:
  - `crate::session::{Session, SessionEntry, SessionSource, write, remove, session_file_path}` (Task 2)
  - `crate::recovery::{current_snapshot_file, snapshots_removed_on_session_close, snapshot::snapshot_file_id}` (Task 5)
- Produces:
  - `App::session_path: Option<PathBuf>`
  - `fn session_path(hwnd: HWND) -> Option<PathBuf>`, where `None` means the window does not take part
  - `fn save_session_for_close(hwnd: HWND) -> bool`
  - Test helpers `enable_session(hwnd, &RecoveryScratch)` and `write_session(&RecoveryScratch, Vec<SessionEntry>, usize)`

- [ ] **Step 1: Write the failing tests** in `src/window/main_window.rs` `mod tests`.
  - Add `use crate::session::{Session, SessionEntry, SessionSource};` to the test imports.
  - Add the helpers and tests:
    ```rust
        /// Makes the window a primary instance saving its session under `scratch`.
        fn enable_session(hwnd: HWND, scratch: &RecoveryScratch) {
            let recovery = scratch.path().join("Recovery");
            std::fs::create_dir_all(&recovery).unwrap();
            let app = app_mut(hwnd);
            app.instance_mutex = Some(unnamed_mutex());
            app.recovery_root = Some(recovery);
            app.session_path = Some(scratch.path().join("session.ini"));
        }

        fn write_session(scratch: &RecoveryScratch, entries: Vec<SessionEntry>, active: usize) {
            crate::session::write(
                &scratch.path().join("session.ini"),
                &Session { active, entries },
            )
            .unwrap();
        }

        #[test]
        fn session_close_records_every_tab_without_prompting() {
            // Break caught: a session close that still asks about unsaved text, drops an unsaved or
            // clean tab from the manifest, loses the active tab, or deletes the snapshot the next
            // launch needs.
            let _scintilla = load_native_scintilla();
            let scratch = RecoveryScratch::new("session-close");
            let file = scratch.path().join("notes.txt");
            std::fs::write(&file, "saved text").unwrap();
            let window = ProductionWindow::new(make_app());
            let editor = install_test_editor(&window);
            enable_session(window.hwnd, &scratch);
            App::open_path(window.hwnd, &file).unwrap();
            execute_command(window.hwnd, CommandId::New);
            editor.set_text("unsaved words").unwrap();
            execute_command(window.hwnd, CommandId::New);
            let prompted = std::rc::Rc::new(std::cell::Cell::new(false));
            let seen = std::rc::Rc::clone(&prompted);
            answer_next_close_prompt(move |_| {
                seen.set(true);
                CloseDecision::Cancel
            });

            unsafe { SendMessageW(window.hwnd, WM_CLOSE, 0, 0) };

            assert!(!prompted.get(), "session restore must not prompt");
            assert_eq!(unsafe { IsWindow(window.hwnd) }, 0);
            let session = crate::session::read(&scratch.path().join("session.ini")).unwrap();
            assert_eq!(session.entries.len(), 2, "the empty untitled tab is skipped");
            assert_eq!(session.entries[0].source, SessionSource::File(file));
            let SessionSource::Snapshot(id) = session.entries[1].source else {
                panic!("the unsaved tab must be recorded as a snapshot");
            };
            assert_eq!(session.active, 1, "the skipped active tab falls back to the one before");
            let snapshot = crate::recovery::snapshot::snapshot_path(
                &scratch.path().join("Recovery"),
                id,
            );
            let snapshot = Snapshot::decode(&std::fs::read(snapshot).unwrap()).unwrap();
            assert_eq!(snapshot.text, "unsaved words");
        }

        #[test]
        fn session_close_still_prompts_when_restore_is_off() {
            // Break caught: the setting being ignored, so unsaved text is kept silently even though
            // the user asked to be prompted.
            let _scintilla = load_native_scintilla();
            let scratch = RecoveryScratch::new("session-off");
            let window = ProductionWindow::new(make_app());
            let editor = install_test_editor(&window);
            enable_session(window.hwnd, &scratch);
            app_mut(window.hwnd).settings.restore_session = false;
            editor.set_text("dirty").unwrap();
            answer_next_close_prompt(|_| CloseDecision::Cancel);

            unsafe { SendMessageW(window.hwnd, WM_CLOSE, 0, 0) };

            assert_ne!(unsafe { IsWindow(window.hwnd) }, 0, "Cancel keeps the window");
            assert!(!scratch.path().join("session.ini").exists());
        }
    ```
  - If `CloseDecision`, `IsWindow` or `Snapshot` are not already imported in `mod tests`, add them. `CloseDecision` is at `crate::document::CloseDecision`. `IsWindow` is already imported at `~3685`.

- [ ] **Step 2: Run the tests and confirm they fail.**
  - Run: `cargo test --lib session_close -- --test-threads=1`
  - Expected: FAIL to compile (no field `session_path`).

- [ ] **Step 3: Implement.**
  - `src/app.rs`:
    - Add after `recovery_owner`:
      ```rust
          /// `session.ini` for a primary window, resolved on first use; tests pre-seed it.
          pub(crate) session_path: Option<std::path::PathBuf>,
      ```
    - Add `session_path: None,` in `App::new`.
  - `src/editor/scintilla_constants.rs`: next to `SCI_GETCURRENTPOS`, add `pub const SCI_GETANCHOR: u32 = 2009;`.
  - `src/window/main_window.rs`, replace the `WM_CLOSE` arm with:
    ```rust
            WM_CLOSE => {
                if file_population_active(hwnd) {
                    return 0;
                }
                // A launch forwarded just before the review must be handled, not lost with the window.
                drain_ipc_requests(hwnd);
                if !save_session_for_close(hwnd) {
                    let Some(discarded) = review_dirty_documents(hwnd) else {
                        return 0;
                    };
                    remove_session_snapshots(hwnd, &discarded);
                }
                shutdown_ipc(hwnd);
                clear_documents_for_shutdown(hwnd);
                unsafe {
                    DestroyWindow(hwnd);
                }
                0
            }
    ```
  - Add after `remove_session_snapshots`:
    ```rust
    /// Where this window keeps its session, or `None` when it does not take part: session restore
    /// is off, or this is not the primary instance. Tests never resolve the real path.
    fn session_path(hwnd: HWND) -> Option<std::path::PathBuf> {
        let mut app = unsafe { app_ptr(hwnd) }?;
        let app = unsafe { app.as_mut() };
        if !app.settings.restore_session || app.instance_mutex.is_none() {
            return None;
        }
        #[cfg(not(test))]
        if app.session_path.is_none() {
            app.session_path = crate::session::session_file_path().ok();
        }
        app.session_path.clone()
    }

    /// With session restore on, records every tab in `session.ini` instead of asking about unsaved
    /// changes. False sends the caller to the review prompts: the feature is off, or some unsaved
    /// text could not be secured in a snapshot and must not close silently.
    fn save_session_for_close(hwnd: HWND) -> bool {
        let Some(identity) = (unsafe { window_identity(hwnd) }) else {
            return false;
        };
        let Some(path) = session_path(hwnd) else {
            return false;
        };
        let Some(root) = recovery_root(hwnd) else {
            return false;
        };
        let pending = unsafe { app_ptr(hwnd) }.map_or(0, |app| {
            unsafe { app.as_ref() }
                .tabs
                .documents()
                .filter(|document| crate::recovery::needs_snapshot(document))
                .count()
        });
        // Each call writes the next document still needing one, so `pending` calls cover them all.
        for _ in 0..pending {
            snapshot_next_document(hwnd);
            if !identity.is_live_for(hwnd) {
                return false;
            }
        }
        let Some(session) = build_session(hwnd, &root) else {
            return false;
        };
        let written = if session.entries.is_empty() {
            crate::session::remove(&path);
            Ok(())
        } else {
            crate::session::write(&path, &session)
        };
        if written.is_err() {
            return false;
        }
        let files = unsafe { app_ptr(hwnd) }.map(|app| {
            unsafe { app.as_ref() }
                .tabs
                .documents()
                .flat_map(|document| {
                    crate::recovery::snapshots_removed_on_session_close(&root, document)
                })
                .collect::<Vec<_>>()
        });
        crate::recovery::remove_snapshot_files(&files.unwrap_or_default());
        true
    }

    /// The manifest for the open tabs, or `None` when a dirty tab's text is in no snapshot file.
    /// Clean untitled tabs are empty and skipped. Only the shown tab has a caret and scroll
    /// position worth keeping, because switching tabs resets the view.
    fn build_session(hwnd: HWND, root: &std::path::Path) -> Option<crate::session::Session> {
        use crate::editor::scintilla_constants::{SCI_GETANCHOR, SCI_GETCURRENTPOS};
        use crate::session::{Session, SessionEntry, SessionSource};
        let app = unsafe { app_ptr(hwnd) }?;
        let app = unsafe { app.as_ref() };
        let active_id = app.tabs.active().map(|document| document.id);
        let mut session = Session::default();
        let mut active_entry = None;
        for document in app.tabs.documents() {
            let is_active = Some(document.id) == active_id;
            let source = if document.dirty {
                let file = crate::recovery::current_snapshot_file(root, document)?;
                SessionSource::Snapshot(crate::recovery::snapshot::snapshot_file_id(&file)?)
            } else if let Some(path) = document.path.as_ref().filter(|path| path.to_str().is_some()) {
                SessionSource::File(path.clone())
            } else {
                if is_active {
                    session.active = session.entries.len().saturating_sub(1);
                }
                continue;
            };
            if is_active {
                session.active = session.entries.len();
                active_entry = Some(session.entries.len());
            }
            session.entries.push(SessionEntry::new(source));
        }
        if let (Some(index), Some(editor)) = (active_entry, app.editor.as_ref()) {
            let read = |message| unsafe { SendMessageW(editor.hwnd(), message, 0, 0) }.max(0) as usize;
            let entry = &mut session.entries[index];
            entry.caret = read(SCI_GETCURRENTPOS);
            entry.anchor = read(SCI_GETANCHOR);
            entry.first_line = editor.first_visible_line().unwrap_or(0);
        }
        Some(session)
    }
    ```

- [ ] **Step 4: Run the tests and confirm they pass.**
  - Run: `cargo test --lib session_close -- --test-threads=1`
  - Expected: PASS.
  - Then run the existing close/IPC window tests: `cargo test --lib window::main_window::tests::closing -- --test-threads=1`, plus any test whose name contains `close`.
  - Tests that set `instance_mutex` but not `session_path` must behave exactly as before, because `session_path` returns `None` under `cfg(test)`.

- [ ] **Step 5: Commit.**

```bash
git add src/app.rs src/editor/scintilla_constants.rs src/window/main_window.rs
git commit -m "feat(window): save the session instead of prompting on close"
```

---

### Task 7: Restore the session at startup

**Files:**
- Modify: `src/app.rs` (field `session_restore`, init)
- Modify: `src/window/main_window.rs`:
  - `handle_deferred` ~666-719
  - `recover_snapshots` ~2972
  - rename/refactor `open_recovered_snapshot` ~3156-3225
  - `save_session_for_close` (from Task 6)
  - new restore fns
  - tests

**Interfaces:**
- Consumes:
  - `SessionRestore` and `crate::session::{read, remove, restore_failure_notice}` (Task 2)
  - `WM_FASTPAD_RESTORE_SESSION` (Task 3)
  - `from_session` (Task 5)
  - `session_path`, `enable_session`, `write_session` (Task 6)
- Produces:
  - `App::session_restore: Option<crate::session::SessionRestore>`
  - `enum RestoreStep { Continue, Done }`
  - `fn restore_session_step(hwnd) -> RestoreStep`
  - `enum SnapshotTab { Recovered, Session }`
  - `fn open_snapshot_tab(hwnd, &WindowIdentity, SnapshotCandidate, SnapshotTab) -> Result<()>`

- [ ] **Step 1: Write the failing tests** in `src/window/main_window.rs` `mod tests`:

```rust
    /// Runs only the session unit until it hands over to `WM_FASTPAD_OPEN_REQUEST`, without
    /// pumping the rest of the chain (which would bind the real single-instance pipe).
    fn run_session_restore(hwnd: HWND) {
        use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;
        let restore = crate::window::WM_FASTPAD_RESTORE_SESSION;
        unsafe { PostMessageW(hwnd, restore, 0, 0) };
        let mut message = MSG::default();
        while unsafe { PeekMessageW(&mut message, hwnd, restore, restore, PM_REMOVE) } != 0 {
            unsafe { DispatchMessageW(&message) };
        }
    }

    #[test]
    fn session_restore_reopens_files_and_unsaved_text_in_order() {
        // Break caught: restored tabs out of order, an unsaved file reopening untitled (so Ctrl+S
        // asks for a path), a stray empty startup tab, a lost caret, a manifest that restores
        // twice, or crash recovery opening a restored snapshot again.
        let _scintilla = load_native_scintilla();
        let scratch = RecoveryScratch::new("session-restore");
        let recovery = scratch.path().join("Recovery");
        let notes = scratch.path().join("notes.txt");
        std::fs::write(&notes, "saved text").unwrap();
        let draft = scratch.path().join("draft.txt");
        std::fs::write(&draft, "on disk").unwrap();
        let draft_id = RecoveryId::from_u128(0x5e55);
        write_snapshot(
            &recovery,
            &Snapshot::new(draft_id, Some(draft.clone()), Encoding::Utf8, "unsaved draft"),
        )
        .unwrap();
        let scratch_id = RecoveryId::from_u128(0x5e56);
        write_snapshot(
            &recovery,
            &Snapshot::new(scratch_id, None, Encoding::Utf8, "scratch words"),
        )
        .unwrap();
        write_session(
            &scratch,
            vec![
                SessionEntry {
                    source: SessionSource::Snapshot(draft_id),
                    caret: 3,
                    anchor: 1,
                    first_line: 0,
                },
                SessionEntry::new(SessionSource::File(notes.clone())),
                SessionEntry::new(SessionSource::Snapshot(scratch_id)),
            ],
            0,
        );
        let window = ProductionWindow::new(make_app());
        let editor = install_test_editor(&window);
        enable_session(window.hwnd, &scratch);

        run_session_restore(window.hwnd);

        {
            let app = app_mut(window.hwnd);
            let documents = app.tabs.documents().collect::<Vec<_>>();
            assert_eq!(documents.len(), 3, "the empty startup tab is closed");
            assert_eq!(documents[0].path.as_deref(), Some(draft.as_path()));
            assert!(documents[0].dirty);
            assert_eq!(documents[0].title(), "draft.txt *");
            assert_eq!(documents[1].path.as_deref(), Some(notes.as_path()));
            assert!(!documents[1].dirty);
            assert_eq!(documents[2].path, None);
            assert_eq!(documents[2].title(), "Untitled *");
            assert_eq!(app.tabs.active_index(), 0);
        }
        assert_eq!(editor.text().unwrap(), "unsaved draft");
        assert_eq!(editor.selection().unwrap(), 1..3);
        assert!(!scratch.path().join("session.ini").exists(), "the manifest is consumed");

        unsafe { SendMessageW(window.hwnd, crate::window::WM_FASTPAD_RECOVERY, 0, 0) };
        assert_eq!(app_mut(window.hwnd).tabs.len(), 3, "recovery must not duplicate a tab");

        execute_command(window.hwnd, CommandId::Save);
        assert_eq!(std::fs::read_to_string(&draft).unwrap(), "unsaved draft");
        assert!(
            !crate::recovery::snapshot::snapshot_path(&recovery, draft_id).exists(),
            "saving a restored tab removes its snapshot"
        );
    }

    #[test]
    fn session_restore_skips_unreopenable_entries_with_one_notice() {
        // Break caught: one missing file aborting the rest of the restore, a notice per file, or
        // no tab activated when the saved active entry is the one that failed.
        let _scintilla = load_native_scintilla();
        let scratch = RecoveryScratch::new("session-missing");
        let kept = scratch.path().join("kept.txt");
        std::fs::write(&kept, "still here").unwrap();
        write_session(
            &scratch,
            vec![
                SessionEntry::new(SessionSource::File(scratch.path().join("gone.txt"))),
                SessionEntry::new(SessionSource::File(kept.clone())),
                SessionEntry::new(SessionSource::Snapshot(RecoveryId::from_u128(0xdead))),
            ],
            0,
        );
        let window = ProductionWindow::new(make_app());
        let editor = install_test_editor(&window);
        enable_session(window.hwnd, &scratch);

        run_session_restore(window.hwnd);

        let app = app_mut(window.hwnd);
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tabs.active().unwrap().path.as_deref(), Some(kept.as_path()));
        assert_eq!(editor.text().unwrap(), "still here");
        let notices = app
            .notifications
            .pending()
            .iter()
            .filter(|notice| notice.message.contains("last session"))
            .map(|notice| notice.message.clone())
            .collect::<Vec<_>>();
        assert_eq!(notices, vec![crate::session::restore_failure_notice(2)]);
    }

    #[test]
    fn session_restore_ignores_a_window_outside_the_session() {
        // Break caught: a --new-window instance or a disabled setting consuming the primary
        // window's session.
        let _scintilla = load_native_scintilla();
        let scratch = RecoveryScratch::new("session-outside");
        let notes = scratch.path().join("notes.txt");
        std::fs::write(&notes, "saved text").unwrap();
        write_session(&scratch, vec![SessionEntry::new(SessionSource::File(notes))], 0);
        let window = ProductionWindow::new(make_app());
        let _editor = install_test_editor(&window);
        enable_session(window.hwnd, &scratch);

        app_mut(window.hwnd).settings.restore_session = false;
        run_session_restore(window.hwnd);
        app_mut(window.hwnd).settings.restore_session = true;
        app_mut(window.hwnd).instance_mutex = None;
        run_session_restore(window.hwnd);

        assert_eq!(app_mut(window.hwnd).tabs.len(), 1);
        assert_eq!(app_mut(window.hwnd).tabs.active().unwrap().path, None);
        assert!(scratch.path().join("session.ini").exists());
    }

    #[test]
    fn session_restore_opens_the_launch_file_last() {
        // Break caught: the command-line file opening before the restored tabs, so a restored tab
        // ends up active instead of the file the user just asked for.
        let _scintilla = load_native_scintilla();
        let scratch = RecoveryScratch::new("session-launch");
        let restored = scratch.path().join("restored.txt");
        std::fs::write(&restored, "from last time").unwrap();
        let launched = scratch.path().join("launched.txt");
        std::fs::write(&launched, "asked for now").unwrap();
        write_session(&scratch, vec![SessionEntry::new(SessionSource::File(restored))], 0);
        let mut app = make_app();
        app.launch.request = crate::launch::LaunchRequest::Open(launched.into_os_string());
        let window = ProductionWindow::new(app);
        let editor = install_test_editor(&window);
        enable_session(window.hwnd, &scratch);

        run_session_restore(window.hwnd);
        unsafe { SendMessageW(window.hwnd, crate::window::WM_FASTPAD_OPEN_REQUEST, 0, 0) };

        assert_eq!(app_mut(window.hwnd).tabs.len(), 2);
        assert_eq!(app_mut(window.hwnd).tabs.active_index(), 1);
        assert_eq!(editor.text().unwrap(), "asked for now");
    }
```

Add `DispatchMessageW, MSG, PM_REMOVE, PeekMessageW` to the test `use` if they are not already imported (they are at ~3685).

- [ ] **Step 2: Run the tests and confirm they fail.**
  - Run: `cargo test --lib session_restore -- --test-threads=1`
  - Expected: FAIL. The unit is still a pass-through, so tab counts and paths do not match.

- [ ] **Step 3: Implement.**
  - `src/app.rs`:
    - Add after `session_path`:
      ```rust
          /// Present while the last session's entries are still being reopened.
          pub(crate) session_restore: Option<crate::session::SessionRestore>,
      ```
    - Add `session_restore: None,` in `App::new`.
  - `handle_deferred`: insert directly after the `load_settings` block from Task 3:
    ```rust
        // Only `WM_FASTPAD_RESTORE_SESSION` processed with no input pending produces this action.
        // Each pass reopens at most one session entry and reposts the unit until none remain.
        if action == DeferredAction::PostNext(crate::window::WM_FASTPAD_OPEN_REQUEST)
            && restore_session_step(hwnd) == RestoreStep::Continue
        {
            unsafe {
                PostMessageW(hwnd, crate::window::WM_FASTPAD_RESTORE_SESSION, 0, 0);
            }
            return 0;
        }
    ```
  - `recover_snapshots`: after the `let Some(root) = recovery_root(hwnd) else { return; };` line, add:
    ```rust
        // Every open posts the language unit, which continues into this one. While a session
        // restore is still reopening entries it owns their snapshots. Recovery runs again after
        // `WM_FASTPAD_OPEN_REQUEST`, which always posts the language unit.
        if unsafe { app_ptr(hwnd) }
            .is_some_and(|app| unsafe { app.as_ref() }.session_restore.is_some())
        {
            return;
        }
    ```
  - `save_session_for_close`: after the `session_path` early return, add:
    ```rust
        // Mid-restore, the manifest is gone and unrestored entries are in no tab. The review flow
        // keeps their snapshots on disk for recovery instead.
        if unsafe { app_ptr(hwnd) }
            .is_some_and(|app| unsafe { app.as_ref() }.session_restore.is_some())
        {
            return false;
        }
    ```
  - Rename `open_recovered_snapshot` to `open_snapshot_tab`, add a `kind: SnapshotTab` parameter, and bind session tabs to their files.
    - Add above it:
      ```rust
      /// How a snapshot comes back as a tab: after a crash (untitled, titled "Recovered: ...") or from
      /// the last session (bound to its file again, so Ctrl+S saves where it came from).
      #[derive(Clone, Copy, Debug, Eq, PartialEq)]
      enum SnapshotTab {
          Recovered,
          Session,
      }
      ```
    - Change the signature to:
      ```rust
      fn open_snapshot_tab(
          hwnd: HWND,
          identity: &WindowIdentity,
          candidate: crate::recovery::SnapshotCandidate,
          kind: SnapshotTab,
      ) -> Result<()> {
      ```
    - After `let crate::recovery::SnapshotCandidate { path, snapshot } = candidate;`, add:
      ```rust
          let from_session = kind == SnapshotTab::Session;
          // A session tab edits its file again, unless another tab already has that file open.
          let bound_path = snapshot.original_path.clone().filter(|original| {
              from_session
                  && unsafe { app_ptr(hwnd) }
                      .is_some_and(|app| unsafe { app.as_ref() }.tabs.find_path(original).is_none())
          });
      ```
    - Replace the document setup lines with:
      ```rust
          let mut document = Document::untitled(id, recovery_id, editor.create_document()?);
          document.path = bound_path;
          document.encoding = snapshot.encoding;
          document.dirty = true;
          document.recovery_generation = Some(document.generation);
          document.recovery_origin = Some(crate::document::RecoveryOrigin {
              snapshot_path: path,
              original_path: snapshot.original_path,
              from_session,
          });
      ```
    - The rest of the function is unchanged.
    - In `recover_snapshots`, change the call to `open_snapshot_tab(hwnd, &identity, candidate, SnapshotTab::Recovered)`.
  - Add the restore step functions near `recover_snapshots`:
    ```rust
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum RestoreStep {
        Continue,
        Done,
    }

    /// One `WM_FASTPAD_RESTORE_SESSION` pass. The first pass takes the manifest. Each pass reopens
    /// at most one entry, and the pass that finds none left finishes the restore.
    fn restore_session_step(hwnd: HWND) -> RestoreStep {
        if file_population_active(hwnd) {
            return RestoreStep::Continue;
        }
        let started = unsafe { app_ptr(hwnd) }
            .is_some_and(|app| unsafe { app.as_ref() }.session_restore.is_some());
        if !started && !begin_session_restore(hwnd) {
            return RestoreStep::Done;
        }
        let entry = unsafe { app_ptr(hwnd) }.and_then(|app| {
            unsafe { app.as_ref() }
                .session_restore
                .as_ref()?
                .next_entry()
                .cloned()
        });
        let Some(entry) = entry else {
            finish_session_restore(hwnd);
            return RestoreStep::Done;
        };
        let restored = restore_session_entry(hwnd, &entry);
        if let Some(mut app) = unsafe { app_ptr(hwnd) }
            && let Some(restore) = unsafe { app.as_mut() }.session_restore.as_mut()
        {
            restore.record(restored);
        }
        RestoreStep::Continue
    }

    /// Takes the manifest, deleting it so a crash from here on is recovery's alone, and remembers
    /// the empty startup tab so it can be closed. False when there is nothing to restore.
    fn begin_session_restore(hwnd: HWND) -> bool {
        let Some(path) = session_path(hwnd) else {
            return false;
        };
        let Some(session) = crate::session::read(&path) else {
            return false;
        };
        crate::session::remove(&path);
        if session.entries.is_empty() {
            return false;
        }
        let placeholder = empty_startup_tab(hwnd);
        let Some(mut app) = (unsafe { app_ptr(hwnd) }) else {
            return false;
        };
        unsafe { app.as_mut() }.session_restore =
            Some(crate::session::SessionRestore::new(session, placeholder));
        true
    }

    /// The active tab when it is still the empty, untouched untitled tab every launch starts with.
    fn empty_startup_tab(hwnd: HWND) -> Option<DocumentId> {
        use crate::editor::scintilla_constants::SCI_GETLENGTH;
        let app = unsafe { app_ptr(hwnd) }?;
        let app = unsafe { app.as_ref() };
        let active = app.tabs.active().filter(|document| {
            !document.dirty && document.path.is_none() && document.recovery_origin.is_none()
        })?;
        let editor = app.editor.as_ref()?;
        let empty = unsafe { SendMessageW(editor.hwnd(), SCI_GETLENGTH, 0, 0) } == 0;
        empty.then_some(active.id)
    }

    /// Reopens one manifest entry and applies its language while it is the active tab. Returns
    /// its tab, or `None` when the entry could not be reopened.
    fn restore_session_entry(
        hwnd: HWND,
        entry: &crate::session::SessionEntry,
    ) -> Option<DocumentId> {
        match &entry.source {
            crate::session::SessionSource::File(path) => open_path(hwnd, path).ok()?,
            crate::session::SessionSource::Snapshot(id) => {
                let identity = unsafe { window_identity(hwnd) }?;
                let root = recovery_root(hwnd)?;
                let path = crate::recovery::snapshot::snapshot_path(&root, *id);
                let snapshot =
                    crate::recovery::Snapshot::decode(&std::fs::read(&path).ok()?).ok()?;
                let candidate = crate::recovery::SnapshotCandidate { path, snapshot };
                open_snapshot_tab(hwnd, &identity, candidate, SnapshotTab::Session).ok()?;
            }
        }
        apply_detected_language(hwnd);
        unsafe { app_ptr(hwnd) }
            .and_then(|app| Some(unsafe { app.as_ref() }.tabs.active()?.id))
    }

    /// Closes the empty startup tab once something replaced it, shows the saved active tab with its
    /// caret and scroll position, and reports every entry that failed in one notice.
    fn finish_session_restore(hwnd: HWND) {
        let Some(restore) = unsafe { app_ptr(hwnd) }
            .and_then(|mut app| unsafe { app.as_mut() }.session_restore.take())
        else {
            return;
        };
        let restored_any = restore.restored.iter().any(Option::is_some);
        if let Some(placeholder) = restore.placeholder
            && restored_any
            && still_empty_untitled(hwnd, placeholder)
            && activate_document_by_id(hwnd, placeholder)
        {
            close_active_document(hwnd);
        }
        if let Some(active) = restore.active_tab()
            && activate_document_by_id(hwnd, active)
            && restore.saved_active_restored() == Some(active)
        {
            apply_view_state(hwnd, &restore.session.entries[restore.session.active]);
        }
        if restore.failed > 0 {
            push_notice(hwnd, crate::session::restore_failure_notice(restore.failed));
        }
    }

    /// A reused startup tab now has a path, and a typed-in one is dirty. Neither may be closed.
    fn still_empty_untitled(hwnd: HWND, id: DocumentId) -> bool {
        unsafe { app_ptr(hwnd) }.is_some_and(|app| {
            unsafe { app.as_ref() }.tabs.document(id).is_some_and(|document| {
                !document.dirty && document.path.is_none() && document.recovery_origin.is_none()
            })
        })
    }

    fn apply_view_state(hwnd: HWND, entry: &crate::session::SessionEntry) {
        use crate::editor::scintilla_constants::SCI_GETLENGTH;
        let Some(editor) =
            (unsafe { app_ptr(hwnd) }).and_then(|app| unsafe { app.as_ref() }.editor.clone())
        else {
            return;
        };
        // The file may have shrunk since the session was saved.
        let length = unsafe { SendMessageW(editor.hwnd(), SCI_GETLENGTH, 0, 0) }.max(0) as usize;
        let _ = editor.set_selection(entry.anchor.min(length)..entry.caret.min(length));
        let _ = editor.set_first_visible_line(entry.first_line);
    }
    ```
  - Note: `restore_session_entry`'s `File` arm has type `()` after `.ok()?`, which is what the `match` needs.
  - Note: `close_active_document` does not prompt for a clean tab. `finish_session_restore` only calls it when `restored_any`, so the window never ends up with zero tabs.

- [ ] **Step 4: Run the tests and confirm they pass.**
  - Run: `cargo test --lib session_ -- --test-threads=1`
  - Expected: every `session_*` test PASSes (Tasks 4, 6 and 7).
  - Also run `cargo test --lib recover -- --test-threads=1` for the renamed snapshot-tab path.
  - Then compile everything: `cargo clippy --all-targets --all-features -- -D warnings`.

- [ ] **Step 5: Commit.**

```bash
git add src/app.rs src/window/main_window.rs
git commit -m "feat(window): reopen the last session's tabs after first paint"
```

---

### Task 8: End-to-end relaunch test and primary-instance test audit

**Files:**
- Create: `tests/windows/session.rs`
- Modify: `Cargo.toml` (add a `[[test]]` entry next to `single_instance`)
- Possibly modify: `tests/windows/single_instance.rs`, `tests/windows/support/acceptance.rs` (see Step 4)

- [ ] **Step 1: Write the test file** `tests/windows/session.rs`:

```rust
#![cfg(windows)]
// Requires that no other FastPad runs in this session: only the primary instance keeps a session.

mod support;

use fastpad::window::commands::CommandId;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use support::process::{FastPadProcess, wait_and_dismiss_dialog, wait_for_process_exit};
use support::win32::{Deadline, find_child_by_class, scintilla_text, send_text};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE, WM_COMMAND};

static SESSION_TEST_LOCK: Mutex<()> = Mutex::new(());
const WAIT: Duration = Duration::from_secs(5);

#[test]
fn a_closed_session_reopens_its_tabs_and_unsaved_text_on_the_next_launch() {
    // Break caught: a session close that still prompts, a manifest never written, a restore that
    // loses the unsaved tab or the active tab, or crash recovery adding a duplicate tab.
    let _serial = SESSION_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let data = Scratch::new("reopen");
    let file = data.file("notes.txt", "saved text");

    let mut first = FastPadProcess::spawn_with_local_app_data([&file], &data.root).unwrap();
    let hwnd = first
        .wait_for_main_window(WAIT)
        .expect("no main window: is another FastPad running in this session?");
    let editor = find_child_by_class(hwnd, "Scintilla").unwrap();
    wait_for_text(editor, "saved text");
    command(hwnd, CommandId::New);
    wait_for_text(editor, "");
    send_text(editor, "unsaved words").unwrap();
    wait_for_text(editor, "unsaved words");
    unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
    wait_for_process_exit(first.id(), WAIT).expect("closing with session restore on must not prompt");
    first.close().unwrap();
    assert!(data.session().exists());

    let mut second =
        FastPadProcess::spawn_with_local_app_data(std::iter::empty::<&str>(), &data.root).unwrap();
    let hwnd = second.wait_for_main_window(WAIT).unwrap();
    let editor = find_child_by_class(hwnd, "Scintilla").unwrap();
    wait_for_text(editor, "unsaved words");
    command(hwnd, CommandId::SelectTab1);
    wait_for_text(editor, "saved text");
    // Give the recovery unit time to run; a duplicate would show up as a third tab.
    std::thread::sleep(Duration::from_millis(500));
    command(hwnd, CommandId::SelectTab3);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(scintilla_text(editor).unwrap(), "saved text", "a third tab exists");
    assert!(!data.session().exists(), "the restore consumes the manifest");

    unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
    wait_for_process_exit(second.id(), WAIT).unwrap();
    second.close().unwrap();
}

#[test]
fn with_session_restore_off_closing_asks_and_nothing_is_kept() {
    // Break caught: the setting being ignored, so FastPad silently keeps text the user expects
    // to be asked about.
    let _serial = SESSION_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let data = Scratch::new("off");
    std::fs::write(data.root.join("FastPad").join("fastpad.ini"), "restore_session=false\n").unwrap();

    let mut process =
        FastPadProcess::spawn_with_local_app_data(std::iter::empty::<&str>(), &data.root).unwrap();
    let hwnd = process
        .wait_for_main_window(WAIT)
        .expect("no main window: is another FastPad running in this session?");
    let editor = find_child_by_class(hwnd, "Scintilla").unwrap();
    send_text(editor, "throwaway").unwrap();
    wait_for_text(editor, "throwaway");
    unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
    wait_and_dismiss_dialog(process.id(), WAIT).expect("closing with session restore off must prompt");
    wait_for_process_exit(process.id(), WAIT).unwrap();
    process.close().unwrap();
    assert!(!data.session().exists());
}

struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "fastpad-session-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("FastPad")).unwrap();
        Self { root }
    }

    fn file(&self, name: &str, text: &str) -> PathBuf {
        let path = self.root.join(name);
        std::fs::write(&path, text).unwrap();
        path
    }

    fn session(&self) -> PathBuf {
        self.root.join("FastPad").join("session.ini")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn command(hwnd: HWND, command: CommandId) {
    unsafe {
        PostMessageW(hwnd, WM_COMMAND, command as usize, 0);
    }
}

fn wait_for_text(editor: HWND, expected: &str) {
    let deadline = Deadline::after(WAIT);
    while !scintilla_text(editor).is_ok_and(|text| text == expected) {
        assert!(!deadline.expired(), "timed out waiting for editor text {expected:?}");
        deadline.sleep_step();
    }
}
```

- [ ] **Step 2: Register it** in `Cargo.toml`, after the `single_instance` entry:

```toml
[[test]]
name = "session"
path = "tests/windows/session.rs"
```

- [ ] **Step 3: Run it.**
  - Run: `cargo test --test session -- --test-threads=1`
  - Expected: both PASS. Make sure no FastPad window is open in this Windows session first.
  - If `wait_and_dismiss_dialog` answers anything other than "No"/Discard, read `tests/windows/support/process.rs:239` and match what `tests/windows/recovery.rs::discard_on_window_close` relies on.

- [ ] **Step 4: Audit the other tests that launch as the primary instance.**
  - Some tests launch without `--new-window` and now close with session restore on, so they no longer prompt:
    - `tests/windows/single_instance.rs`
    - `tests/windows/support/acceptance.rs` (`assert_no_resident_process`, the `spawn_with_local_app_data(std::iter::empty…)` launches)
    - `tests/windows/startup_smoke.rs:48`
    - `tests/windows/markdown_preview.rs:892`
  - Run: `cargo test --test single_instance --test acceptance --test startup_smoke --test markdown_preview -- --test-threads=1`
  - For any failure caused by a missing prompt, or by a session reopening tabs in a later launch that shares the same scratch `LOCALAPPDATA`, add this line after the scratch `FastPad` directory is created:
    ```rust
    std::fs::write(root.join("FastPad").join("fastpad.ini"), "restore_session=false\n").unwrap();
    ```
  - Only add it to the scratch set-up that test uses, and add a comment `// This test is about <X>, not session restore.`
  - Do not change assertions.

- [ ] **Step 5: Commit.**

```bash
git add tests/windows/session.rs Cargo.toml tests/windows
git commit -m "test: relaunch restores the session end to end"
```

---

### Task 9: Docs and full verification

**Files:**
- Modify: `README.md` (features section ~129-134, settings table ~163-172)

- [ ] **Step 1: Document the feature.**
  - In `README.md`, after the "Crash recovery that just works" section, add:
    ```markdown
    ### Pick up where you left off

    Close FastPad and the next launch reopens every tab, including unsaved edits and untitled notes,
    with the tab you were on active. Closing never nags about unsaved changes while this is on. Turn
    it off with **File > Restore session on startup** or `restore_session=false`, and FastPad asks
    before closing again.
    ```
  - Add a row to the settings table after `recovery_interval_seconds`:
    ```markdown
    | `restore_session` | `true`/`false`, `1`/`0`, `yes`/`no`, `on`/`off` | `true` |
    ```

- [ ] **Step 2: Format and lint.**
  - Run: `cargo fmt --check`. If it reports diffs, run `cargo fmt` and re-check.
  - Run: `cargo clippy --all-targets --all-features -- -D warnings`
  - Expected: no output from fmt, and clippy finishes with no warnings.

- [ ] **Step 3: Run the full suite once.**
  - Run: `cargo test -- --test-threads=1`
  - Expected: all tests PASS. Report any failure with its output. Do not skip or `#[ignore]` a test to get green.

- [ ] **Step 4: Check startup performance.**
  - Run `cargo run --release --bin fastpad-bench` the way the project normally runs it (see `src/bin/fastpad-bench.rs` usage/help).
  - Compare warm TTI p50/p95 against a build of `main` (build it in a separate worktree).
  - Expected: no regression beyond noise. With no `session.ini` present, the new unit only reads settings and posts the next message.

- [ ] **Step 5: Commit.**

```bash
git add README.md
git commit -m "docs: describe session restore and restore_session"
```

## Spec Coverage Check

| Spec section | Task |
|---|---|
| §3 Setting, toggle, notice | 1, 4 |
| §4 Primary-only participation | 6 (`session_path`), 7 (`session_restore_ignores_a_window_outside_the_session`) |
| §5 Manifest format and module | 2 |
| §6 Close flow: flush, fallback, manifest, snapshot cleanup | 5, 6 |
| §7 Deferred unit, one entry per message, path-bound snapshots, launch file last, notice, placeholder, view state, titles, recovery interplay, test seam | 3, 5, 7 |
| §8 Error handling | 6 (fallbacks), 7 (skips, notice), 5 (claimed snapshots) |
| §9 Performance | 3, 7 (one entry per message), 9 (bench) |
| §10 Testing | 1-8 |
